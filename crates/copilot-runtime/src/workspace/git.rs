use std::{ffi::OsStr, path::Path, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

use super::{Result, ReviewError};

pub(super) const MAX_OUTPUT: usize = 16 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) struct Output {
    pub bytes: Vec<u8>,
    pub code: Option<i32>,
}

pub(super) async fn run(
    directory: &Path,
    arguments: &[impl AsRef<OsStr>],
    operation: &'static str,
    allow_difference: bool,
    limit: usize,
) -> Result<Output> {
    run_input(
        directory,
        arguments,
        operation,
        allow_difference,
        limit,
        None,
    )
    .await
}

pub(super) async fn run_input(
    directory: &Path,
    arguments: &[impl AsRef<OsStr>],
    operation: &'static str,
    allow_difference: bool,
    limit: usize,
    input: Option<&[u8]>,
) -> Result<Output> {
    verify_version(directory).await?;
    execute(
        directory,
        arguments,
        operation,
        allow_difference,
        limit,
        input,
        true,
    )
    .await
}

async fn verify_version(directory: &Path) -> Result<()> {
    static VERSION: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    VERSION
        .get_or_try_init(|| async {
            let output = execute(
                directory,
                &["--version"],
                "checking the Git version",
                false,
                256,
                None,
                false,
            )
            .await?;
            let raw =
                std::str::from_utf8(&output.bytes).map_err(|_| ReviewError::InvalidGitOutput)?;
            let version = raw
                .split_whitespace()
                .nth(2)
                .ok_or(ReviewError::InvalidGitOutput)?;
            let mut parts = version.split('.');
            let major: u32 = parts
                .next()
                .and_then(|part| part.parse().ok())
                .ok_or(ReviewError::InvalidGitOutput)?;
            let minor: u32 = parts
                .next()
                .and_then(|part| part.parse().ok())
                .ok_or(ReviewError::InvalidGitOutput)?;
            if (major, minor) < (2, 45) {
                return Err(ReviewError::GitVersionUnsupported {
                    version: version.into(),
                });
            }
            Ok(())
        })
        .await?;
    Ok(())
}

async fn execute(
    directory: &Path,
    arguments: &[impl AsRef<OsStr>],
    operation: &'static str,
    allow_difference: bool,
    limit: usize,
    input: Option<&[u8]>,
    strict: bool,
) -> Result<Output> {
    let mut command = Command::new("git");
    command.args([
        "--no-optional-locks",
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.quotePath=true",
    ]);
    if strict {
        command.arg("--no-lazy-fetch");
    }
    command
        .args(arguments)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_EXTERNAL_DIFF",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(variable);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReviewError::GitUnavailable(error)
        } else {
            ReviewError::GitIo(error)
        }
    })?;
    let stdout = child.stdout.take().ok_or(ReviewError::InvalidGitOutput)?;
    let stderr = child.stderr.take().ok_or(ReviewError::InvalidGitOutput)?;
    let stdin = child.stdin.take();
    let mut bytes = Vec::new();
    let mut diagnostic = Vec::new();
    let result = timeout(COMMAND_TIMEOUT, async {
        let read_out = async {
            stdout
                .take(u64::try_from(limit + 1).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
                .await
        };
        let read_error = async { stderr.take(16 * 1024).read_to_end(&mut diagnostic).await };
        let write_input = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                stdin.write_all(input).await?;
                stdin.shutdown().await?;
            }
            Ok::<_, std::io::Error>(())
        };
        tokio::try_join!(read_out, read_error, write_input, child.wait())
    })
    .await;
    let status = match result {
        Ok(Ok((_, _, (), status))) => status,
        Ok(Err(error)) => {
            let _ = child.kill().await;
            return Err(ReviewError::GitIo(error));
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(ReviewError::Timeout);
        }
    };
    if bytes.len() > limit {
        return Err(ReviewError::Limit);
    }
    if !(status.success() || allow_difference && status.code() == Some(1)) {
        return Err(ReviewError::GitFailed {
            operation,
            code: status.code(),
        });
    }
    Ok(Output {
        bytes,
        code: status.code(),
    })
}
