//! Private Unix daemon lifecycle and the bounded, transparent SSH proxy.

use std::{
    collections::hash_map::DefaultHasher,
    fs::{File, OpenOptions, TryLockError},
    hash::{Hash, Hasher},
    io,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use serde_json::Value;
use tandem_protocol::{PROTOCOL_VERSION, RequestEnvelope, ServerFrame, encode_frame, read_frame};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::{Child, Command},
    signal::unix::{SignalKind, signal},
    task::JoinSet,
    time::{Instant, sleep, sleep_until, timeout},
};

use crate::{error::ServerError, stdio::AgentHost};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CONNECTIONS: usize = 64;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DaemonError {
    #[error("HOME is required to select the shared remote runtime")]
    MissingHome,
    #[error("Daemon directory must be a real directory, not a symbolic link: {}", path.display())]
    InvalidDirectory { path: PathBuf },
    #[error("Daemon path belongs to another user: {}", path.display())]
    WrongOwner { path: PathBuf },
    #[error("Daemon path must not grant permissions to other users: {}", path.display())]
    InsecurePermissions { path: PathBuf },
    #[error("Daemon lock must be a regular file with one link: {}", path.display())]
    InvalidLock { path: PathBuf },
    #[error("Daemon socket path contains a different file type: {}", path.display())]
    InvalidSocket { path: PathBuf },
    #[error("Daemon path changed while it was being opened: {}", path.display())]
    PathChanged { path: PathBuf },
    #[error("Daemon socket path exceeds the Unix socket limit: {}", path.display())]
    SocketPathTooLong { path: PathBuf },
    #[error("The shared remote daemon is already running")]
    AlreadyRunning,
    #[error(
        "The shared remote runtime could not safely close a session and has been stopped. Reconnect to resume the conversation."
    )]
    RuntimeCleanupFailed,
    #[error(
        "The shared remote daemon did not start within 20 seconds. Check the remote installation and directory permissions."
    )]
    StartupTimeout,
    #[error(
        "The shared remote daemon exited during startup ({status}). Check the remote installation and directory permissions."
    )]
    StartupFailed { status: std::process::ExitStatus },
    #[error(
        "The shared remote daemon has a different protocol or binary identity. Reconnect after reinstalling the remote server."
    )]
    VersionMismatch,
    #[error("The Unix socket peer belongs to another user")]
    PeerMismatch,
    #[error("The shared remote daemon did not complete its handshake")]
    HandshakeTimeout,
    #[error("The shared remote daemon did not accept the connection within five seconds")]
    ConnectTimeout,
    #[error("Remote runtime installation did not finish within 180 seconds")]
    InstallTimeout,
    #[error(
        "The shared remote daemon did not finish sending responses within 30 seconds of input closing"
    )]
    OutputDrainTimeout,
    #[error("Unexpected response while installing the shared remote runtime")]
    UnexpectedInstallResponse,
    #[error("Remote runtime installation failed: {0}")]
    RuntimeInstall(#[source] tandem_protocol::RpcError),
    #[error("Could not access daemon path {}: {source}", path.display())]
    Filesystem {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Shared remote daemon I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Protocol(#[from] tandem_protocol::ProtocolError),
}

type Result<T> = std::result::Result<T, DaemonError>;

struct DaemonPaths {
    directory: PathBuf,
    socket: PathBuf,
    lock: PathBuf,
    startup_lock: PathBuf,
    executable: PathBuf,
    identity: String,
    uid: u32,
}

impl DaemonPaths {
    fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or(DaemonError::MissingHome)?;
        let home = filesystem(Path::new(&home), std::fs::canonicalize(&home))?;
        #[cfg(target_os = "linux")]
        let executable = linux_executable();
        #[cfg(not(target_os = "linux"))]
        let executable = std::env::current_exe()?;
        let (local, _peer) = UnixStream::pair()?;
        let uid = local.peer_cred()?.uid();
        let root = filesystem(Path::new("/tmp"), std::fs::canonicalize("/tmp"))?;
        Self::new(&root, &home, &executable, uid)
    }

    fn new(root: &Path, home: &Path, executable: &Path, uid: u32) -> Result<Self> {
        let mut home_hash = DefaultHasher::new();
        home.hash(&mut home_hash);
        let directory = root.join(format!("tandem-{uid}-{:016x}", home_hash.finish()));
        private_directory(&directory, uid)?;
        let mut binary_hash = DefaultHasher::new();
        env!("CARGO_PKG_VERSION").hash(&mut binary_hash);
        PROTOCOL_VERSION.hash(&mut binary_hash);
        // Every copy of one build shares a namespace, including concurrent
        // uploads that atomically replace the installed file with a new inode.
        env!("TANDEM_SERVER_BUILD_ID").hash(&mut binary_hash);
        let identity = format!("{PROTOCOL_VERSION}-{:016x}", binary_hash.finish());
        let socket = directory.join(format!("{identity}.sock"));
        // macOS has a smaller sockaddr_un than Linux; leave room for its NUL.
        if socket.as_os_str().as_encoded_bytes().len() >= 104 {
            return Err(DaemonError::SocketPathTooLong { path: socket });
        }
        Ok(Self {
            lock: directory.join(format!("{identity}.lock")),
            startup_lock: directory.join(format!("{identity}.start")),
            directory,
            socket,
            executable: executable.to_owned(),
            identity,
            uid,
        })
    }

    fn handshake(&self) -> Vec<u8> {
        format!("TANDEM {}\n", self.identity).into_bytes()
    }
}

#[cfg(target_os = "linux")]
fn linux_executable() -> PathBuf {
    // Atomic uploads can unlink this process's original executable or replace
    // its pathname with another build. /proc pins the binary already running.
    PathBuf::from("/proc/self/exe")
}

fn filesystem<T>(path: &Path, result: io::Result<T>) -> Result<T> {
    result.map_err(|source| DaemonError::Filesystem {
        path: path.to_owned(),
        source,
    })
}

fn private_directory(path: &Path, uid: u32) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        result => filesystem(path, result)?,
    }
    let metadata = filesystem(path, std::fs::symlink_metadata(path))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DaemonError::InvalidDirectory {
            path: path.to_owned(),
        });
    }
    private_metadata(path, &metadata, uid)
}

fn private_metadata(path: &Path, metadata: &std::fs::Metadata, uid: u32) -> Result<()> {
    if metadata.uid() != uid {
        return Err(DaemonError::WrongOwner {
            path: path.to_owned(),
        });
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(DaemonError::InsecurePermissions {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn lock_file(path: &Path, uid: u32) -> Result<File> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = filesystem(path, std::fs::symlink_metadata(path))?;
            validate_lock(path, &metadata, uid)?;
            let file = filesystem(path, OpenOptions::new().read(true).write(true).open(path))?;
            let opened = filesystem(path, file.metadata())?;
            if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
                return Err(DaemonError::PathChanged {
                    path: path.to_owned(),
                });
            }
            file
        }
        result => filesystem(path, result)?,
    };
    validate_lock(path, &filesystem(path, file.metadata())?, uid)?;
    Ok(file)
}

fn validate_lock(path: &Path, metadata: &std::fs::Metadata, uid: u32) -> Result<()> {
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(DaemonError::InvalidLock {
            path: path.to_owned(),
        });
    }
    private_metadata(path, metadata, uid)
}

fn try_lock(file: &File) -> Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

fn validate_socket(path: &Path, uid: u32) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(DaemonError::InvalidSocket {
                    path: path.to_owned(),
                });
            }
            if metadata.uid() != uid {
                return Err(DaemonError::WrongOwner {
                    path: path.to_owned(),
                });
            }
            // The containing directory is private. A daemon may still be
            // setting socket permissions immediately after bind returns.
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        result => filesystem(path, result).map(|_| false),
    }
}

async fn handshake(stream: &mut UnixStream, expected: &[u8], uid: u32) -> Result<()> {
    if stream.peer_cred()?.uid() != uid {
        return Err(DaemonError::PeerMismatch);
    }
    timeout(HANDSHAKE_TIMEOUT, async {
        stream.write_all(expected).await?;
        stream.flush().await?;
        let mut received = vec![0; expected.len()];
        stream.read_exact(&mut received).await?;
        if received != expected {
            return Err(DaemonError::VersionMismatch);
        }
        Ok(())
    })
    .await
    .map_err(|_| DaemonError::HandshakeTimeout)?
}

async fn try_connect(paths: &DaemonPaths) -> Result<Option<UnixStream>> {
    if !validate_socket(&paths.socket, paths.uid)? {
        return Ok(None);
    }
    match timeout(HANDSHAKE_TIMEOUT, UnixStream::connect(&paths.socket))
        .await
        .map_err(|_| DaemonError::ConnectTimeout)?
    {
        Ok(mut stream) => {
            match handshake(&mut stream, &paths.handshake(), paths.uid).await {
                Ok(()) => Ok(Some(stream)),
                Err(DaemonError::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    // The idle daemon may be completing shutdown as we connect.
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

async fn connect(paths: &DaemonPaths) -> Result<UnixStream> {
    if let Some(stream) = try_connect(paths).await? {
        return Ok(stream);
    }
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let startup_lock = lock_file(&paths.startup_lock, paths.uid)?;
    while !try_lock(&startup_lock)? {
        if let Some(stream) = try_connect(paths).await? {
            return Ok(stream);
        }
        if Instant::now() >= deadline {
            return Err(DaemonError::StartupTimeout);
        }
        sleep(Duration::from_millis(50)).await;
    }
    if let Some(stream) = try_connect(paths).await? {
        return Ok(stream);
    }
    let lifetime_lock = lock_file(&paths.lock, paths.uid)?;
    while !try_lock(&lifetime_lock)? {
        if let Some(stream) = try_connect(paths).await? {
            return Ok(stream);
        }
        if Instant::now() >= deadline {
            return Err(DaemonError::StartupTimeout);
        }
        sleep(Duration::from_millis(25)).await;
    }
    // The startup lock stays held across launch. Release the lifetime lock so
    // the new daemon can acquire it; an exiting daemon has now finished cleanup.
    drop(lifetime_lock);
    let mut command = Command::new(&paths.executable);
    command
        .arg("--daemon")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    command.as_std_mut().process_group(0);
    let mut child = command.spawn()?;
    let result = await_startup(paths, &mut child, deadline).await;
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    } else {
        // Reap the child if it exits while this proxy is alive. Dropping this
        // waiter during proxy shutdown leaves the daemon running independently.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
    result
}

async fn await_startup(
    paths: &DaemonPaths,
    child: &mut Child,
    deadline: Instant,
) -> Result<UnixStream> {
    loop {
        if let Some(stream) = try_connect(paths).await? {
            return Ok(stream);
        }
        if let Some(status) = child.try_wait()? {
            return Err(DaemonError::StartupFailed { status });
        }
        if Instant::now() >= deadline {
            return Err(DaemonError::StartupTimeout);
        }
        sleep(Duration::from_millis(25)).await;
    }
}

pub(crate) async fn proxy() -> Result<()> {
    let paths = DaemonPaths::discover()?;
    let stream = connect(&paths).await?;
    relay(tokio::io::stdin(), tokio::io::stdout(), stream).await
}

async fn relay(
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin,
    remote: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<()> {
    let (incoming, outgoing) = tokio::io::split(remote);
    let mut sending = Box::pin(forward_frames(input, outgoing));
    let mut receiving = Box::pin(forward_frames(incoming, output));
    tokio::select! {
        result = &mut sending => {
            result?;
            timeout(OUTPUT_DRAIN_TIMEOUT, receiving)
                .await
                .map_err(|_| DaemonError::OutputDrainTimeout)?
        },
        result = &mut receiving => result,
    }
}

async fn forward_frames(
    input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> Result<()> {
    let mut input = BufReader::new(input);
    while let Some(frame) = read_frame(&mut input).await? {
        output.write_all(&frame).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
    }
    output.shutdown().await?;
    Ok(())
}

pub(crate) async fn install_runtime() -> Result<()> {
    let paths = DaemonPaths::discover()?;
    let mut stream = connect(&paths).await?;
    stream
        .write_all(&encode_frame(&RequestEnvelope::new(
            1,
            "install_runtime",
            Value::Null,
        )?)?)
        .await?;
    stream.flush().await?;
    let mut incoming = BufReader::new(stream);
    let response = timeout(INSTALL_TIMEOUT, read_frame(&mut incoming))
        .await
        .map_err(|_| DaemonError::InstallTimeout)??
        .ok_or(DaemonError::UnexpectedInstallResponse)?;
    match ServerFrame::decode(&response)? {
        ServerFrame::Response { id: 1, .. } => Ok(()),
        ServerFrame::Failure { id: 1, error } => Err(DaemonError::RuntimeInstall(error)),
        _ => Err(DaemonError::UnexpectedInstallResponse),
    }
}

struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && (metadata.dev(), metadata.ino()) == self.identity
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn bind(paths: &DaemonPaths) -> Result<(UnixListener, SocketGuard)> {
    private_directory(&paths.directory, paths.uid)?;
    if validate_socket(&paths.socket, paths.uid)? {
        // The lifetime lock is held by the caller. Only remove a refused socket
        // of the expected type, never an arbitrary path or a running listener.
        match timeout(HANDSHAKE_TIMEOUT, UnixStream::connect(&paths.socket))
            .await
            .map_err(|_| DaemonError::ConnectTimeout)?
        {
            Ok(_) => return Err(DaemonError::AlreadyRunning),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) =>
            {
                if validate_socket(&paths.socket, paths.uid)? {
                    filesystem(&paths.socket, std::fs::remove_file(&paths.socket))?;
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    let listener = UnixListener::bind(&paths.socket)?;
    filesystem(
        &paths.socket,
        std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600)),
    )?;
    let metadata = filesystem(&paths.socket, std::fs::symlink_metadata(&paths.socket))?;
    Ok((
        listener,
        SocketGuard {
            path: paths.socket.clone(),
            identity: (metadata.dev(), metadata.ino()),
        },
    ))
}

pub(crate) async fn run() -> crate::error::Result<()> {
    let paths = DaemonPaths::discover()?;
    let lifetime_lock = lock_file(&paths.lock, paths.uid)?;
    if !try_lock(&lifetime_lock)? {
        return Err(DaemonError::AlreadyRunning.into());
    }
    let (listener, _socket) = bind(&paths).await?;
    let host = Arc::new(AgentHost::new());
    let mut clients = JoinSet::new();
    let result = serve(&paths, listener, &host, &mut clients).await;
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    let shutdown = host.shutdown().await;
    result.map_err(ServerError::from).and(shutdown)
}

async fn serve(
    paths: &DaemonPaths,
    listener: UnixListener,
    host: &Arc<AgentHost>,
    clients: &mut JoinSet<crate::error::Result<()>>,
) -> Result<()> {
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut idle_deadline = Instant::now() + IDLE_TIMEOUT;
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            () = host.failed() => return Err(DaemonError::RuntimeCleanupFailed),
            () = sleep_until(idle_deadline), if clients.is_empty() => break,
            _ = clients.join_next(), if !clients.is_empty() => {
                if clients.is_empty() {
                    idle_deadline = Instant::now() + IDLE_TIMEOUT;
                }
            }
            connection = listener.accept(), if clients.len() < MAX_CONNECTIONS => {
                let (mut stream, _) = connection?;
                let expected = paths.handshake();
                let uid = paths.uid;
                let host = Arc::clone(host);
                clients.spawn(async move {
                    handshake(&mut stream, &expected, uid).await?;
                    let (input, output) = stream.into_split();
                    crate::stdio::run_connection(host, input, output).await
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::{fs::symlink, net::UnixListener as StdUnixListener},
        sync::atomic::{AtomicU64, Ordering},
    };

    use anyhow::{Context, Result};

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = Path::new("/tmp").join(format!(
                "td-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new().mode(0o700).create(&path)?;
            Ok(Self(path))
        }

        fn uid(&self) -> Result<u32> {
            Ok(std::fs::metadata(&self.0)?.uid())
        }

        fn paths(&self) -> Result<DaemonPaths> {
            Ok(DaemonPaths::new(
                &self.0,
                &self.0,
                &std::env::current_exe()?,
                self.uid()?,
            )?)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn namespaces_separate_homes_and_keep_long_unicode_paths_out_of_socket_names() -> Result<()> {
        let root = TestDirectory::new()?;
        let executable = std::env::current_exe()?;
        let first = DaemonPaths::new(
            &root.0,
            &root.0.join("long home 空间".repeat(50)),
            &executable,
            root.uid()?,
        )?;
        let same = DaemonPaths::new(
            &root.0,
            &root.0.join("long home 空间".repeat(50)),
            &executable,
            root.uid()?,
        )?;
        let other = root.paths()?;
        assert_eq!(first.socket, same.socket);
        assert_ne!(first.socket, other.socket);
        assert!(first.socket.as_os_str().as_encoded_bytes().len() < 104);
        Ok(())
    }

    #[test]
    fn copied_and_atomically_replaced_artifacts_share_the_same_daemon_namespace() -> Result<()> {
        let root = TestDirectory::new()?;
        let original = root.0.join("original-server");
        let installed = root.0.join("installed-server");
        let replacement = root.0.join("replacement-server");
        std::fs::write(&original, b"one build")?;
        std::fs::copy(&original, &installed)?;
        std::fs::copy(&original, &replacement)?;
        let source = DaemonPaths::new(&root.0, &root.0, &original, root.uid()?)?;
        let before = DaemonPaths::new(&root.0, &root.0, &installed, root.uid()?)?;
        let inode = std::fs::metadata(&installed)?.ino();
        std::fs::rename(&replacement, &installed)?;
        assert_ne!(std::fs::metadata(&installed)?.ino(), inode);
        let after = DaemonPaths::new(&root.0, &root.0, &installed, root.uid()?)?;
        assert_eq!(source.socket, before.socket);
        assert_eq!(before.socket, after.socket);
        assert_eq!(source.handshake(), after.handshake());
        assert_eq!(before.lock, after.lock);
        assert_eq!(before.startup_lock, after.startup_lock);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_launch_uses_the_running_binary_even_after_its_installed_file_is_removed() -> Result<()>
    {
        const CHILD_PATH: &str = "TANDEM_TEST_UNLINKED_EXECUTABLE";
        if let Some(expected) = std::env::var_os(CHILD_PATH) {
            let executable = std::env::current_exe()?;
            assert_eq!(executable, PathBuf::from(expected));
            std::fs::remove_file(&executable)?;
            let result = std::process::Command::new(linux_executable())
                .arg("--list")
                .output()?;
            assert!(result.status.success());
            assert!(String::from_utf8(result.stdout)?.contains("daemon::tests::"));
            return Ok(());
        }
        let root = TestDirectory::new()?;
        let executable = root.0.join("installed-server-test");
        std::fs::copy(std::env::current_exe()?, &executable)?;
        let executable = executable.canonicalize()?;
        let result = std::process::Command::new(&executable)
            .args([
                "--exact",
                "daemon::tests::daemon_launch_uses_the_running_binary_even_after_its_installed_file_is_removed",
                "--nocapture",
            ])
            .env(CHILD_PATH, &executable)
            .output()?;
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!executable.exists());
        Ok(())
    }

    #[test]
    fn private_directories_reject_symlinks_shared_permissions_and_wrong_owners() -> Result<()> {
        let root = TestDirectory::new()?;
        let path = root.0.join("directory");
        private_directory(&path, root.uid()?)?;
        assert_eq!(std::fs::metadata(&path)?.mode() & 0o777, 0o700);
        assert!(matches!(
            private_directory(&path, root.uid()?.wrapping_add(1)),
            Err(DaemonError::WrongOwner { .. })
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750))?;
        assert!(matches!(
            private_directory(&path, root.uid()?),
            Err(DaemonError::InsecurePermissions { .. })
        ));
        let link = root.0.join("link");
        symlink(&path, &link)?;
        assert!(matches!(
            private_directory(&link, root.uid()?),
            Err(DaemonError::InvalidDirectory { .. })
        ));
        Ok(())
    }

    #[test]
    fn locks_are_exclusive_and_reject_symlinks_and_multiple_links() -> Result<()> {
        let root = TestDirectory::new()?;
        let path = root.0.join("daemon.lock");
        let first = lock_file(&path, root.uid()?)?;
        let second = lock_file(&path, root.uid()?)?;
        assert!(try_lock(&first)?);
        assert!(!try_lock(&second)?);
        drop(first);
        assert!(try_lock(&second)?);
        drop(second);
        let alias = root.0.join("alias");
        symlink(&path, &alias)?;
        assert!(matches!(
            lock_file(&alias, root.uid()?),
            Err(DaemonError::InvalidLock { .. })
        ));
        let hard_link = root.0.join("hard-link");
        std::fs::hard_link(&path, &hard_link)?;
        assert!(matches!(
            lock_file(&path, root.uid()?),
            Err(DaemonError::InvalidLock { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn stale_sockets_are_replaced_but_files_symlinks_and_live_sockets_are_preserved()
    -> Result<()> {
        let root = TestDirectory::new()?;
        let paths = root.paths()?;
        let lifetime_lock = lock_file(&paths.lock, paths.uid)?;
        assert!(try_lock(&lifetime_lock)?);
        std::fs::write(&paths.socket, b"keep this file")?;
        assert!(matches!(
            bind(&paths).await,
            Err(DaemonError::InvalidSocket { .. })
        ));
        assert_eq!(std::fs::read(&paths.socket)?, b"keep this file");
        std::fs::remove_file(&paths.socket)?;
        symlink(&paths.lock, &paths.socket)?;
        assert!(matches!(
            bind(&paths).await,
            Err(DaemonError::InvalidSocket { .. })
        ));
        assert!(
            std::fs::symlink_metadata(&paths.socket)?
                .file_type()
                .is_symlink()
        );
        std::fs::remove_file(&paths.socket)?;
        let live = StdUnixListener::bind(&paths.socket)?;
        assert!(matches!(
            bind(&paths).await,
            Err(DaemonError::AlreadyRunning)
        ));
        drop(live);
        let (listener, guard) = bind(&paths).await?;
        assert_eq!(std::fs::metadata(&paths.socket)?.mode() & 0o777, 0o600);
        drop(listener);
        drop(guard);
        assert!(!paths.socket.exists());
        assert!(paths.lock.exists());
        Ok(())
    }

    #[tokio::test]
    async fn socket_cleanup_never_removes_a_replacement_file() -> Result<()> {
        let root = TestDirectory::new()?;
        let paths = root.paths()?;
        let (listener, guard) = bind(&paths).await?;
        std::fs::remove_file(&paths.socket)?;
        std::fs::write(&paths.socket, b"replacement")?;
        drop(listener);
        drop(guard);
        assert_eq!(std::fs::read(&paths.socket)?, b"replacement");
        Ok(())
    }

    #[tokio::test]
    async fn handshake_verifies_binary_identity_and_the_peer_user() -> Result<()> {
        for (left_identity, right_identity, matches_identity) in [
            (b"same\n".as_slice(), b"same\n".as_slice(), true),
            (b"left\n".as_slice(), b"else\n".as_slice(), false),
        ] {
            let (mut left, mut right) = UnixStream::pair()?;
            let uid = left.peer_cred()?.uid();
            let (first, second) = tokio::join!(
                handshake(&mut left, left_identity, uid),
                handshake(&mut right, right_identity, uid)
            );
            if matches_identity {
                first?;
                second?;
            } else {
                assert!(matches!(first, Err(DaemonError::VersionMismatch)));
                assert!(matches!(second, Err(DaemonError::VersionMismatch)));
            }
        }
        let (mut left, _right) = UnixStream::pair()?;
        let uid = left.peer_cred()?.uid();
        assert!(matches!(
            handshake(&mut left, b"same\n", uid.wrapping_add(1)).await,
            Err(DaemonError::PeerMismatch)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn proxy_forwards_complete_frames_and_rejects_truncated_frames() -> Result<()> {
        let bytes = b"{\"id\":1,\"value\":null}\n{\"event\":{}}\n";
        let mut forwarded = Vec::new();
        forward_frames(bytes.as_slice(), &mut forwarded).await?;
        assert_eq!(forwarded, bytes);
        let mut forwarded = Vec::new();
        assert!(matches!(
            forward_frames(b"{\"id\":1}".as_slice(), &mut forwarded).await,
            Err(DaemonError::Protocol(
                tandem_protocol::ProtocolError::TruncatedFrame
            ))
        ));
        assert_eq!(forwarded, [] as [u8; 0]);
        Ok(())
    }

    #[tokio::test]
    async fn proxy_drains_responses_after_input_is_half_closed() -> Result<()> {
        let request = b"{\"id\":1,\"command\":\"server_info\"}\n";
        let response = b"{\"id\":1,\"value\":null}\n";
        let (remote, mut server) = tokio::io::duplex(1024);
        let serving = tokio::spawn(async move {
            let mut received = Vec::new();
            server.read_to_end(&mut received).await?;
            assert_eq!(received, request);
            tokio::task::yield_now().await;
            server.write_all(response).await?;
            server.shutdown().await?;
            Ok::<(), io::Error>(())
        });
        let mut received = Vec::new();
        relay(request.as_slice(), &mut received, remote).await?;
        serving.await??;
        assert_eq!(received, response);
        Ok(())
    }

    #[tokio::test]
    async fn closed_remote_output_does_not_wait_for_an_open_input() -> Result<()> {
        let (_client, input) = tokio::io::duplex(1024);
        let (remote, server) = tokio::io::duplex(1024);
        drop(server);
        let mut output = Vec::new();
        timeout(Duration::from_secs(1), relay(input, &mut output, remote)).await??;
        assert_eq!(output, [] as [u8; 0]);
        Ok(())
    }

    #[tokio::test]
    async fn a_listener_handshakes_with_independent_proxy_connections() -> Result<()> {
        let root = TestDirectory::new()?;
        let paths = root.paths()?;
        let (listener, _guard) = bind(&paths).await?;
        let expected = paths.handshake();
        let uid = paths.uid;
        let server = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await?;
                let expected = expected.clone();
                clients.spawn(async move { handshake(&mut stream, &expected, uid).await });
            }
            while let Some(client) = clients.join_next().await {
                client.context("handshake task")??;
            }
            Ok::<(), anyhow::Error>(())
        });
        let (one, two, three, four) = tokio::join!(
            try_connect(&paths),
            try_connect(&paths),
            try_connect(&paths),
            try_connect(&paths)
        );
        for result in [one, two, three, four] {
            assert!(result?.is_some());
        }
        server.await??;
        Ok(())
    }
}
