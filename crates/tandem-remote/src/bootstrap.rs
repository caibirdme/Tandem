use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    DirectoryValidationError, RemoteError, RemoteResponseError, RemoteTransport, SshConnection,
    ssh::{run, shell_quote},
};

const CHECKSUM_FUNCTION: &str = r#"tandem_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum < "$1"
  else
    shasum -a 256 < "$1"
  fi
}
"#;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const VERSION: &str = env!("CARGO_PKG_VERSION");

struct ServerProbe {
    platform: RemotePlatform,
    version: Option<String>,
    digest: Option<String>,
}

impl ServerProbe {
    fn matches(&self, digest: &str) -> bool {
        self.version.as_deref() == Some(&server_signature())
            && self.digest.as_deref() == Some(digest)
    }
}

#[derive(Clone, Copy)]
enum ServerMode {
    Runtime,
    Proxy,
}

impl ServerMode {
    const fn argument(self) -> &'static str {
        match self {
            Self::Runtime => "--install-runtime",
            Self::Proxy => "--proxy",
        }
    }

    const fn marker(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Proxy => "proxy",
        }
    }
}

/// Server artifacts are native executables built for the destination, not archives.
/// Each artifact directory supports `<target>/tandem-server` and `tandem-server-<target>`.
#[derive(Clone, Debug, Default)]
pub struct ArtifactLocator {
    pub directory: Option<PathBuf>,
    pub bundled_directory: Option<PathBuf>,
    /// Used only when the destination matches this desktop's operating system and CPU.
    pub local_server: Option<PathBuf>,
}

impl ArtifactLocator {
    /// Locate a platform-specific executable without falling back to a different CPU.
    ///
    /// # Errors
    /// Returns an error when no artifact exists or the selected file is invalid.
    pub async fn locate(&self, platform: &RemotePlatform) -> Result<PathBuf, RemoteError> {
        let mut candidates = Vec::new();
        let environment_directory = std::env::var_os("TANDEM_REMOTE_ARTIFACTS").map(PathBuf::from);
        for directory in [
            self.directory.as_ref(),
            environment_directory.as_ref(),
            self.bundled_directory.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            candidates.push(directory.join(&platform.target).join("tandem-server"));
            candidates.push(directory.join(format!("tandem-server-{}", platform.target)));
        }
        if platform.matches_local() {
            if let Some(path) = &self.local_server {
                candidates.push(path.clone());
            }
            if let Ok(executable) = std::env::current_exe() {
                candidates.push(executable.with_file_name("tandem-server"));
            }
        }
        for path in candidates {
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_BYTES {
                return Err(RemoteError::InvalidArtifact { path });
            }
            return Ok(path);
        }
        Err(RemoteError::ArtifactMissing {
            target: platform.target.clone(),
        })
    }
}

/// A supported SSH host and the exact release artifact it needs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlatform {
    pub os: String,
    pub architecture: String,
    pub target: String,
}

impl RemotePlatform {
    fn from_uname(os: &str, architecture: &str) -> Result<Self, RemoteError> {
        let target = match (os, architecture) {
            ("Linux", "x86_64" | "amd64") => "x86_64-unknown-linux-gnu",
            ("Linux", "aarch64" | "arm64") => "aarch64-unknown-linux-gnu",
            ("Darwin", "x86_64" | "amd64") => "x86_64-apple-darwin",
            ("Darwin", "aarch64" | "arm64") => "aarch64-apple-darwin",
            _ => {
                return Err(RemoteError::UnsupportedPlatform {
                    os: os.into(),
                    architecture: architecture.into(),
                });
            }
        };
        Ok(Self {
            os: os.into(),
            architecture: architecture.into(),
            target: target.into(),
        })
    }

    fn matches_local(&self) -> bool {
        let local_os = match std::env::consts::OS {
            "linux" => "Linux",
            "macos" => "Darwin",
            _ => return false,
        };
        Self::from_uname(local_os, std::env::consts::ARCH)
            .is_ok_and(|local| local.target == self.target)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapReport {
    pub platform: RemotePlatform,
    pub server_version: String,
    pub deployed: bool,
    pub runtime_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirectory {
    pub name: String,
    pub path: String,
}

/// Opens short-lived SSH channels for discovery and one persistent channel per runtime.
#[derive(Clone, Debug)]
pub struct RemoteClient {
    connection: SshConnection,
    artifacts: ArtifactLocator,
}

impl RemoteClient {
    /// Validate the connection before it can reach an operating-system command.
    ///
    /// # Errors
    /// Returns an error for an invalid SSH destination or identity-file argument.
    pub fn new(connection: SshConnection, artifacts: ArtifactLocator) -> Result<Self, RemoteError> {
        connection.validate()?;
        Ok(Self {
            connection,
            artifacts,
        })
    }

    /// Test SSH authentication and remote command execution without installing anything.
    ///
    /// # Errors
    /// Returns an SSH process, authentication, connectivity, or timeout error.
    pub async fn check_connection(&self) -> Result<(), RemoteError> {
        run(
            &self.connection,
            "connection test",
            "true",
            None,
            PROBE_TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// Verify host access, atomically deploy the matching server, and prepare its runtime.
    ///
    /// # Errors
    /// Returns an SSH, unsupported-platform, missing-artifact, or runtime setup error.
    pub async fn bootstrap(&self) -> Result<BootstrapReport, RemoteError> {
        let probe = self.probe(false).await?;
        let artifact = self.artifacts.locate(&probe.platform).await?;
        let digest = artifact_digest(&artifact).await?;
        let deployed = !probe.matches(&digest);
        if deployed {
            run(
                &self.connection,
                "server deployment",
                &upload_command(&digest),
                Some(&artifact),
                INSTALL_TIMEOUT,
            )
            .await?;
        }
        run(
            &self.connection,
            "runtime initialization",
            &guarded_server_command(&digest, ServerMode::Runtime),
            None,
            INSTALL_TIMEOUT,
        )
        .await?;
        Ok(BootstrapReport {
            platform: probe.platform,
            server_version: VERSION.into(),
            deployed,
            runtime_ready: true,
        })
    }

    /// Bootstrap and attach a proxy to the shared remote daemon over SSH.
    ///
    /// # Errors
    /// Returns a bootstrap or process startup error.
    pub async fn connect(&self) -> Result<RemoteTransport, RemoteError> {
        self.bootstrap().await?;
        self.connect_ready().await
    }

    /// Reattach to a prepared host, validating its server and runtime without installing.
    ///
    /// # Errors
    /// Returns a process, transport, or server-version error if the host is no longer ready.
    pub async fn connect_ready(&self) -> Result<RemoteTransport, RemoteError> {
        let transport = self.open_transport().await?;
        let result = transport
            .request("server_info", serde_json::Value::Null)
            .await;
        match result {
            Ok(value) if valid_server_info(&value) && value["runtimeReady"] == true => {
                Ok(transport)
            }
            Ok(value) if valid_server_info(&value) => {
                transport.shutdown().await;
                Err(RemoteError::RuntimeNotReady)
            }
            result => {
                transport.shutdown().await;
                Err(result.err().unwrap_or(RemoteError::ServerVersionMismatch))
            }
        }
    }

    /// Connect to an installed server for workspace inspection without starting an SDK.
    ///
    /// # Errors
    /// Returns a connection or version error; this never deploys or initializes a runtime.
    pub async fn connect_review(&self) -> Result<RemoteTransport, RemoteError> {
        let transport = self.open_transport().await?;
        let result = transport
            .request("server_info", serde_json::Value::Null)
            .await;
        match result {
            Ok(value) if valid_server_info(&value) => Ok(transport),
            result => {
                transport.shutdown().await;
                Err(result.err().unwrap_or(RemoteError::ServerVersionMismatch))
            }
        }
    }

    /// Attach a proxy only when the installed server matches the local artifact.
    ///
    /// # Errors
    /// Returns an artifact, version, response, or SSH failure without deploying a server.
    pub async fn open_transport(&self) -> Result<RemoteTransport, RemoteError> {
        let probe = self.probe(true).await?;
        let artifact = self.artifacts.locate(&probe.platform).await?;
        let digest = artifact_digest(&artifact).await?;
        if probe.version.as_deref() != Some(&server_signature()) {
            return Err(RemoteError::ServerVersionMismatch);
        }
        if probe.digest.as_deref() != Some(&digest) {
            return Err(RemoteError::ServerArtifactMismatch);
        }
        RemoteTransport::spawn(
            self.connection
                .command(&guarded_server_command(&digest, ServerMode::Proxy)),
        )
    }

    async fn probe(&self, readiness: bool) -> Result<ServerProbe, RemoteError> {
        let operation = if readiness {
            "readiness probe"
        } else {
            "environment probe"
        };
        let output = run(
            &self.connection,
            operation,
            &probe_command(readiness),
            None,
            PROBE_TIMEOUT,
        )
        .await?;
        parse_probe(&output)
    }

    /// Resolve a readable remote directory, accepting `~` for the remote home.
    ///
    /// # Errors
    /// Returns an error for invalid input, a missing directory, or inaccessible SSH host.
    pub async fn inspect_directory(&self, path: &str) -> Result<RemoteDirectory, RemoteError> {
        let command = directory_command(path, false)?;
        let output = run(
            &self.connection,
            "directory validation",
            &command,
            None,
            PROBE_TIMEOUT,
        )
        .await?;
        let mut directories = parse_directories(&output)?;
        let actual = directories.len();
        if actual != 1 {
            return Err(RemoteResponseError::CanonicalDirectoryCount { actual }.into());
        }
        directories
            .pop()
            .ok_or_else(|| RemoteResponseError::CanonicalDirectoryCount { actual }.into())
    }

    /// Enumerate immediate subdirectories, including hidden directories, without a runtime.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be read or output exceeds its size limit.
    pub async fn list_directories(&self, path: &str) -> Result<Vec<RemoteDirectory>, RemoteError> {
        let command = directory_command(path, true)?;
        let output = run(
            &self.connection,
            "directory listing",
            &command,
            None,
            PROBE_TIMEOUT,
        )
        .await?;
        let mut entries = parse_directories(&output)?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }
}

async fn artifact_digest(path: &Path) -> Result<String, RemoteError> {
    let read_error = |source| RemoteError::ArtifactRead {
        path: path.to_owned(),
        source,
    };
    let mut file = tokio::fs::File::open(path).await.map_err(read_error)?;
    let metadata = file.metadata().await.map_err(read_error)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(RemoteError::InvalidArtifact {
            path: path.to_owned(),
        });
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).await.map_err(read_error)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_ARTIFACT_BYTES {
            return Err(RemoteError::InvalidArtifact {
                path: path.to_owned(),
            });
        }
        hasher.update(&buffer[..count]);
    }
    if total != metadata.len() {
        return Err(RemoteError::InvalidArtifact {
            path: path.to_owned(),
        });
    }
    let mut digest = String::with_capacity(64);
    for byte in hasher.finalize() {
        for nibble in [byte >> 4, byte & 0x0f] {
            digest.push(char::from(b"0123456789abcdef"[usize::from(nibble)]));
        }
    }
    Ok(digest)
}

fn probe_command(readiness: bool) -> String {
    let marker = if readiness {
        "readiness"
    } else {
        "environment"
    };
    format!(
        r#"# tandem {marker} probe
set -eu
{CHECKSUM_FUNCTION}
os=$(uname -s)
architecture=$(uname -m)
version=''
digest=''
installed="$HOME/.local/share/tandem/bin/tandem-server"
if [ -x "$installed" ]; then
  version=$("$installed" --version || true)
  digest=$(tandem_sha256 "$installed")
  digest=${{digest%% *}}
fi
printf '%s\000%s\000%s\000%s\000' "$os" "$architecture" "$version" "$digest"
"#,
    )
}

fn parse_probe(output: &[u8]) -> Result<ServerProbe, RemoteError> {
    let response = std::str::from_utf8(output).map_err(RemoteResponseError::ProbeEncoding)?;
    let mut parts = response.split('\0');
    let os = parts.next().ok_or(RemoteResponseError::IncompleteProbe)?;
    let architecture = parts.next().ok_or(RemoteResponseError::IncompleteProbe)?;
    let version = parts.next().ok_or(RemoteResponseError::IncompleteProbe)?;
    let digest = parts.next().ok_or(RemoteResponseError::IncompleteProbe)?;
    if parts.next() != Some("") || parts.next().is_some() {
        return Err(RemoteResponseError::IncompleteProbe.into());
    }
    let digest = if digest.is_empty() {
        None
    } else if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Some(digest.to_owned())
    } else {
        return Err(RemoteResponseError::ArtifactDigest.into());
    };
    let version = version
        .trim()
        .strip_prefix("tandem-server ")
        .map(str::to_owned);
    Ok(ServerProbe {
        platform: RemotePlatform::from_uname(os, architecture)?,
        version,
        digest,
    })
}

fn upload_command(digest: &str) -> String {
    format!(
        r#"set -eu
{CHECKSUM_FUNCTION}
umask 077
destination="$HOME/.local/share/tandem/bin"
mkdir -p "$destination"
temporary=$(mktemp "$destination/.tandem-server.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cat > "$temporary"
chmod 700 "$temporary"
actual_digest=$(tandem_sha256 "$temporary")
actual_digest=${{actual_digest%% *}}
[ "$actual_digest" = {} ] || {{ printf '%s\n' 'tandem-server artifact mismatch' >&2; exit 1; }}
actual_version=$("$temporary" --version)
[ "$actual_version" = {} ] || {{ printf '%s\n' 'tandem-server version mismatch' >&2; exit 1; }}
mv -f "$temporary" "$destination/tandem-server"
"#,
        shell_quote(digest),
        shell_quote(&format!("tandem-server {}", server_signature()))
    )
}

fn guarded_server_command(digest: &str, mode: ServerMode) -> String {
    format!(
        r#"# tandem verified {}
set -eu
{CHECKSUM_FUNCTION}
export PATH="$HOME/.local/share/tandem/bin:$HOME/.local/bin:$PATH"
umask 077
destination="$HOME/.local/share/tandem/bin"
temporary=$(mktemp -d "$destination/.tandem-run.XXXXXX")
child=''
cleanup() {{
  if [ -n "$child" ]; then
    kill "$child" 2>/dev/null || true
    wait "$child" 2>/dev/null || true
  fi
  rm -rf "$temporary"
}}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
ln -L "$destination/tandem-server" "$temporary/tandem-server"
actual_digest=$(tandem_sha256 "$temporary/tandem-server")
actual_digest=${{actual_digest%% *}}
[ "$actual_digest" = {} ] || {{ printf '%s\n' 'tandem-server artifact mismatch' >&2; exit 1; }}
actual_version=$("$temporary/tandem-server" --version)
[ "$actual_version" = {} ] || {{ printf '%s\n' 'tandem-server version mismatch' >&2; exit 1; }}
"$temporary/tandem-server" {} <&0 &
child=$!
status=0
wait "$child" || status=$?
child=''
exit "$status"
"#,
        mode.marker(),
        shell_quote(digest),
        shell_quote(&format!("tandem-server {}", server_signature())),
        mode.argument(),
    )
}

fn server_signature() -> String {
    format!("{VERSION} (protocol {})", tandem_protocol::PROTOCOL_VERSION)
}

fn valid_server_info(value: &serde_json::Value) -> bool {
    value["protocolVersion"].as_u64() == Some(u64::from(tandem_protocol::PROTOCOL_VERSION))
        && value["serverVersion"].as_str() == Some(VERSION)
        && value["processId"].as_u64().is_some_and(|id| id > 0)
        && value["runtimeReady"].is_boolean()
}

fn directory_command(path: &str, list: bool) -> Result<String, RemoteError> {
    if path.is_empty() {
        return Err(DirectoryValidationError::Empty.into());
    }
    if path.len() > 4096 {
        return Err(DirectoryValidationError::TooLong { limit: 4096 }.into());
    }
    if path.contains('\0') {
        return Err(DirectoryValidationError::NullByte.into());
    }
    let mut command = format!(
        r#"set -eu
directory={}
case "$directory" in
  '~') directory="$HOME" ;;
  '~/'*) directory="$HOME/${{directory#\~/}}" ;;
esac
CDPATH='' cd -P -- "$directory"
[ -r "$PWD" ] && [ -x "$PWD" ]
"#,
        shell_quote(path)
    );
    if list {
        command.push_str(
            r#"prefix=${PWD%/}
for entry in "$prefix"/* "$prefix"/.[!.]* "$prefix"/..?*; do
  [ -d "$entry" ] || continue
  printf '%s\000' "$entry"
done
"#,
        );
    } else {
        command.push_str("printf '%s\\000' \"$PWD\"\n");
    }
    Ok(command)
}

fn parse_directories(output: &[u8]) -> Result<Vec<RemoteDirectory>, RemoteError> {
    if !output.is_empty() && output.last() != Some(&0) {
        return Err(RemoteResponseError::IncompleteDirectory.into());
    }
    output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let path = std::str::from_utf8(part).map_err(RemoteResponseError::DirectoryEncoding)?;
            if !path.starts_with('/') {
                return Err(RemoteResponseError::RelativeDirectory.into());
            }
            let path = if path == "/" {
                "/"
            } else {
                path.trim_end_matches('/')
            };
            let name = if path == "/" {
                "/"
            } else {
                path.rsplit('/').next().unwrap_or(path)
            };
            Ok(RemoteDirectory {
                name: name.into(),
                path: path.into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let directory = Self(
                std::env::temp_dir()
                    .join(format!("tandem-{name}-{}-{timestamp}", std::process::id())),
            );
            std::fs::create_dir(&directory.0)?;
            Ok(directory)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn artifact_hash_reads_binary_content_and_enforces_upload_size()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TestDirectory::new("artifact-hash")?;
        let path = root.0.join("tandem-server");
        std::fs::write(&path, b"abc")?;
        std::fs::write(root.0.join("artifact.json"), b"untrusted metadata")?;
        assert_eq!(
            artifact_digest(&path).await?,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::write(&path, b"abcd")?;
        assert_eq!(
            artifact_digest(&path).await?,
            "88d4266fd4e6338d13b845fcf289579d209c897823b9217da3e161936f031589"
        );
        std::fs::write(&path, b"")?;
        assert!(matches!(
            artifact_digest(&path).await,
            Err(RemoteError::InvalidArtifact { .. })
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_len(MAX_ARTIFACT_BYTES + 1)?;
        assert!(matches!(
            artifact_digest(&path).await,
            Err(RemoteError::InvalidArtifact { .. })
        ));
        Ok(())
    }

    #[cfg(unix)]
    fn executable(path: &Path, content: &str) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, content)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }

    #[cfg(unix)]
    fn shell(command: &str, home: &Path, input: &[u8]) -> std::io::Result<std::process::Output> {
        use std::{io::Write, process::Stdio};

        let mut child = std::process::Command::new("sh")
            .args(["-c", command])
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut writer) = child.stdin.take() {
            writer.write_all(input)?;
        }
        child.wait_with_output()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn upload_checks_digest_and_version_before_replacing_installed_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TestDirectory::new("artifact-upload")?;
        let destination = root.0.join(".local/share/tandem/bin");
        std::fs::create_dir_all(&destination)?;
        let installed = destination.join("tandem-server");
        let source = root.0.join("source");
        let valid = format!(
            "#!/bin/sh\nprintf '%s\\n' {}\n",
            shell_quote(&format!("tandem-server {}", server_signature()))
        );
        executable(&source, &valid)?;
        let digest = artifact_digest(&source).await?;
        std::fs::write(&installed, b"previous")?;
        let tampered = format!("{valid}\n# changed bytes\n");
        let result = shell(&upload_command(&digest), &root.0, tampered.as_bytes())?;
        assert!(!result.status.success());
        assert_eq!(std::fs::read(&installed)?, b"previous");
        let wrong_version = "#!/bin/sh\nprintf 'tandem-server incompatible\\n'\n";
        executable(&source, wrong_version)?;
        let result = shell(
            &upload_command(&artifact_digest(&source).await?),
            &root.0,
            wrong_version.as_bytes(),
        )?;
        assert!(!result.status.success());
        assert_eq!(std::fs::read(&installed)?, b"previous");
        let result = shell(&upload_command(&digest), &root.0, valid.as_bytes())?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(&installed)?, valid.as_bytes());
        assert_eq!(std::fs::read_dir(&destination)?.count(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verified_execution_pins_the_inode_across_replacement_and_preserves_rpc_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TestDirectory::new("artifact-pin")?;
        let destination = root.0.join(".local/share/tandem/bin");
        std::fs::create_dir_all(&destination)?;
        let installed = destination.join("tandem-server");
        let original = format!(
            r#"#!/bin/sh
case "$1" in
  --version)
    if [ -f "$HOME/replacement" ]; then mv "$HOME/replacement" "$HOME/.local/share/tandem/bin/tandem-server"; fi
    printf '%s\n' {}
    ;;
  --proxy) printf 'original:'; cat ;;
  --install-runtime) printf 'original-runtime' ;;
esac
"#,
            shell_quote(&format!("tandem-server {}", server_signature()))
        );
        for (mode, expected) in [
            (ServerMode::Proxy, "original:rpc input"),
            (ServerMode::Runtime, "original-runtime"),
        ] {
            executable(&installed, &original)?;
            executable(
                &root.0.join("replacement"),
                "#!/bin/sh\nprintf 'replacement'\n",
            )?;
            let digest = artifact_digest(&installed).await?;
            let result = shell(
                &guarded_server_command(&digest, mode),
                &root.0,
                b"rpc input",
            )?;
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(String::from_utf8(result.stdout)?, expected);
            assert_ne!(artifact_digest(&installed).await?, digest);
            assert_eq!(std::fs::read_dir(&destination)?.count(), 1);
        }
        executable(&installed, &original)?;
        let result = shell(
            &guarded_server_command(&"0".repeat(64), ServerMode::Proxy),
            &root.0,
            b"rpc input",
        )?;
        assert!(!result.status.success());
        assert_eq!(result.stdout, b"");
        assert_eq!(std::fs::read_dir(&destination)?.count(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminating_the_wrapper_reaps_its_proxy_and_removes_the_pin()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::process::Stdio;

        let root = TestDirectory::new("artifact-cancel")?;
        let destination = root.0.join(".local/share/tandem/bin");
        std::fs::create_dir_all(&destination)?;
        let installed = destination.join("tandem-server");
        let program = format!(
            r#"#!/bin/sh
case "$1" in
  --version) printf '%s\n' {} ;;
  --proxy) printf '%s\n' "$$" > "$HOME/proxy-pid"; exec sleep 30 ;;
esac
"#,
            shell_quote(&format!("tandem-server {}", server_signature())),
        );
        executable(&installed, &program)?;
        let command =
            guarded_server_command(&artifact_digest(&installed).await?, ServerMode::Proxy);
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", &command])
            .env("HOME", &root.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let proxy_pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(root.0.join("proxy-pid")).await
                    && let Ok(pid) = value.trim().parse::<u32>()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let wrapper_pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("wrapper exited before signal"))?;
        assert!(
            std::process::Command::new("kill")
                .args(["-TERM", &wrapper_pid.to_string()])
                .status()?
                .success()
        );
        let completed = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        if completed.is_err() {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &proxy_pid.to_string()])
                .status();
            child.kill().await?;
        }
        assert!(!completed??.success());
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", &proxy_pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
        );
        assert_eq!(std::fs::read_dir(&destination)?.count(), 1);
        assert_eq!(std::fs::read(&installed)?, program.as_bytes());
        Ok(())
    }

    #[test]
    fn maps_supported_architectures_to_exact_release_artifacts() {
        for (os, architecture, expected) in [
            ("Linux", "x86_64", Some("x86_64-unknown-linux-gnu")),
            ("Linux", "arm64", Some("aarch64-unknown-linux-gnu")),
            ("Darwin", "arm64", Some("aarch64-apple-darwin")),
            ("Darwin", "x86_64", Some("x86_64-apple-darwin")),
            ("Linux", "armv7l", None),
            ("Windows_NT", "x86_64", None),
        ] {
            let actual = RemotePlatform::from_uname(os, architecture)
                .ok()
                .map(|platform| platform.target);
            assert_eq!(actual.as_deref(), expected, "{os}/{architecture}");
        }
    }

    #[test]
    fn directory_protocol_preserves_spaces_newlines_and_shell_characters() {
        let output = b"/home/dev/a b\0/home/dev/a\nb\0/home/dev/it's $(safe)\0/\0";
        let directories =
            parse_directories(output).unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(
            directories
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["a b", "a\nb", "it's $(safe)", "/"]
        );
        for invalid in [b"relative\0".as_slice(), b"/truncated", b"/\xff\0"] {
            assert!(parse_directories(invalid).is_err());
        }
    }

    #[test]
    fn malformed_bootstrap_responses_and_directory_inputs_have_distinct_variants() {
        assert!(matches!(
            parse_probe(b"Linux"),
            Err(RemoteError::InvalidResponse(
                RemoteResponseError::IncompleteProbe
            ))
        ));
        assert!(matches!(
            parse_probe(b"\xff"),
            Err(RemoteError::InvalidResponse(
                RemoteResponseError::ProbeEncoding(_)
            ))
        ));
        assert!(matches!(
            parse_directories(b"relative\0"),
            Err(RemoteError::InvalidResponse(
                RemoteResponseError::RelativeDirectory
            ))
        ));
        assert!(matches!(
            parse_directories(b"/truncated"),
            Err(RemoteError::InvalidResponse(
                RemoteResponseError::IncompleteDirectory
            ))
        ));
        assert!(matches!(
            parse_directories(b"/\xff\0"),
            Err(RemoteError::InvalidResponse(
                RemoteResponseError::DirectoryEncoding(_)
            ))
        ));
        assert!(matches!(
            directory_command("", false),
            Err(RemoteError::InvalidDirectory(
                DirectoryValidationError::Empty
            ))
        ));
        assert!(matches!(
            directory_command(&"x".repeat(4097), false),
            Err(RemoteError::InvalidDirectory(
                DirectoryValidationError::TooLong { limit: 4096 }
            ))
        ));
        assert!(matches!(
            directory_command("/a\0b", false),
            Err(RemoteError::InvalidDirectory(
                DirectoryValidationError::NullByte
            ))
        ));
    }

    #[test]
    fn probe_does_not_mistake_banners_or_other_binaries_for_our_version() {
        for (output, expected) in [
            ("Linux\0x86_64\0tandem-server 0.1.0\0\0", Some("0.1.0")),
            ("Linux\0x86_64\0\0\0", None),
            ("Linux\0x86_64\0other-server 0.1.0\0\0", None),
        ] {
            let probe = parse_probe(output.as_bytes())
                .unwrap_or_else(|error| panic!("probe failed: {error}"));
            assert_eq!(probe.version.as_deref(), expected);
        }
        assert!(parse_probe(b"Welcome\nLinux\0x86_64\0\0\0").is_err());
    }

    #[test]
    fn shared_daemon_requires_matching_protocol_and_release() {
        let valid = serde_json::json!({
            "protocolVersion": tandem_protocol::PROTOCOL_VERSION,
            "serverVersion": VERSION,
            "processId": 123,
            "runtimeReady": true,
        });
        assert!(valid_server_info(&valid));
        let mut unprepared = valid.clone();
        unprepared["runtimeReady"] = serde_json::json!(false);
        assert!(valid_server_info(&unprepared));
        for (field, invalid) in [
            ("protocolVersion", serde_json::json!(0)),
            ("protocolVersion", serde_json::Value::Null),
            ("serverVersion", serde_json::json!("unrecognized")),
            ("processId", serde_json::json!(0)),
            ("processId", serde_json::json!("123")),
            ("runtimeReady", serde_json::Value::Null),
            ("runtimeReady", serde_json::json!("true")),
        ] {
            let mut value = valid.clone();
            value[field] = invalid;
            assert!(!valid_server_info(&value));
        }
    }

    #[test]
    fn probe_requires_canonical_hash_and_matching_release_and_bytes() {
        let digest = "a".repeat(64);
        let probe = parse_probe(
            format!(
                "Linux\0x86_64\0tandem-server {}\0{digest}\0",
                server_signature()
            )
            .as_bytes(),
        )
        .unwrap_or_else(|error| panic!("probe failed: {error}"));
        assert!(probe.matches(&digest));
        assert!(!probe.matches(&"b".repeat(64)));
        let wrong_version = ServerProbe {
            version: Some("different".into()),
            ..probe
        };
        assert!(!wrong_version.matches(&digest));
        for invalid in [
            "invalid".to_owned(),
            "a".repeat(63),
            "A".repeat(64),
            format!("{}\n", "a".repeat(64)),
        ] {
            assert!(matches!(
                parse_probe(format!("Linux\0x86_64\0\0{invalid}\0").as_bytes()),
                Err(RemoteError::InvalidResponse(
                    RemoteResponseError::ArtifactDigest
                ))
            ));
        }
        for incomplete in [
            "Linux\0x86_64\0\0",
            "Linux\0x86_64\0\0\0extra",
            "Linux\0x86_64\0\0\0\0",
        ] {
            assert!(matches!(
                parse_probe(incomplete.as_bytes()),
                Err(RemoteError::InvalidResponse(
                    RemoteResponseError::IncompleteProbe
                ))
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn shell_directory_discovery_preserves_literals_and_expands_only_home()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new("directory-test")?;
        let literal_name = "a ' $(exit 91)\n b";
        let literal_directory = directory.0.join(literal_name);
        std::fs::create_dir(&literal_directory)?;
        std::fs::create_dir(directory.0.join(".hidden"))?;
        std::fs::write(directory.0.join("plain-file"), b"file")?;
        for (input, expected) in [
            ("~".to_owned(), directory.0.clone()),
            (format!("~/{literal_name}"), literal_directory.clone()),
            (literal_directory.display().to_string(), literal_directory),
        ] {
            let output = std::process::Command::new("sh")
                .args(["-c", &directory_command(&input, false)?])
                .env("HOME", &directory.0)
                .output()?;
            assert!(
                output.status.success(),
                "directory command failed for {input:?}"
            );
            let parsed = parse_directories(&output.stdout)?;
            assert_eq!(
                parsed[0].path,
                std::fs::canonicalize(expected)?.display().to_string()
            );
        }
        let output = std::process::Command::new("sh")
            .args(["-c", &directory_command("~", true)?])
            .env("HOME", &directory.0)
            .output()?;
        let mut names = parse_directories(&output.stdout)?
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, [".hidden", literal_name]);
        let root = std::process::Command::new("sh")
            .args(["-c", &directory_command("/", true)?])
            .output()?;
        assert!(
            parse_directories(&root.stdout)?
                .iter()
                .all(|entry| !entry.path.starts_with("//"))
        );
        Ok(())
    }
}
