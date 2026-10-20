use std::{path::Path, process::Stdio, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

use crate::{ConnectionValidationError, RemoteError, SshDiagnostic};

#[cfg(all(test, windows))]
mod windows_tests;

pub(crate) const STDERR_LIMIT: usize = 16 * 1024;
const OUTPUT_LIMIT: usize = 1024 * 1024;

/// A public-key SSH destination. SSH config aliases and agent keys are supported.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnection {
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub identity_file: Option<String>,
}

impl SshConnection {
    /// Reject option-like destinations and characters interpreted by SSH or shells.
    ///
    /// # Errors
    /// Returns an error for an empty destination, unsafe hostname, or invalid port.
    pub fn validate(&self) -> Result<(), RemoteError> {
        let valid_host = !self.host.is_empty()
            && self.host.len() <= 253
            && !self.host.starts_with('-')
            && self.host.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':')
            });
        if !valid_host {
            return Err(RemoteError::InvalidConnection(
                ConnectionValidationError::Host,
            ));
        }
        if self.user.is_empty()
            || self.user.len() > 128
            || !self
                .user
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(RemoteError::InvalidConnection(
                ConnectionValidationError::Username,
            ));
        }
        if self.port == 0 {
            return Err(RemoteError::InvalidConnection(
                ConnectionValidationError::Port,
            ));
        }
        if self
            .identity_file
            .as_ref()
            .is_some_and(|path| path.is_empty() || path.contains('\0'))
        {
            return Err(RemoteError::InvalidConnection(
                ConnectionValidationError::IdentityFile,
            ));
        }
        Ok(())
    }

    pub(crate) fn command(&self, remote_command: &str) -> Command {
        let mut command = Command::new("ssh");
        // SSH communicates through pipes; a desktop launch must not create a console.
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "PreferredAuthentications=publickey",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "RemoteCommand=none",
            "-o",
            "RequestTTY=no",
            "-o",
            "StdinNull=no",
            "-o",
            "ForkAfterAuthentication=no",
            "-o",
            "EscapeChar=none",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "LogLevel=ERROR",
        ]);
        command
            .arg("-p")
            .arg(self.port.to_string())
            .arg("-l")
            .arg(&self.user);
        if let Some(path) = &self.identity_file {
            command.arg("-i").arg(path);
        }
        command
            .arg(&self.host)
            .arg(remote_command)
            .kill_on_drop(true);
        command
    }
}

pub(crate) async fn run(
    connection: &SshConnection,
    operation: &'static str,
    remote_command: &str,
    input: Option<&Path>,
    deadline: Duration,
) -> Result<Vec<u8>, RemoteError> {
    let mut child = connection
        .command(remote_command)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(missing_pipe)?;
    let stderr = child.stderr.take().ok_or_else(missing_pipe)?;
    let stdin = child.stdin.take();
    let work = async {
        let write = async {
            if let (Some(path), Some(mut writer)) = (input, stdin) {
                let mut file = tokio::fs::File::open(path).await?;
                tokio::io::copy(&mut file, &mut writer).await?;
                writer.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        };
        let (status, output, diagnostic, sent) = tokio::join!(
            child.wait(),
            read_bounded(stdout, OUTPUT_LIMIT),
            read_bounded(stderr, STDERR_LIMIT),
            write,
        );
        let status = status?;
        let (diagnostic, _) = diagnostic?;
        if !status.success() {
            return Err(RemoteError::ConnectionFailed {
                operation,
                status,
                detail: safe_diagnostic(&diagnostic),
            });
        }
        sent?;
        let (output, exceeded) = output?;
        if exceeded {
            return Err(RemoteError::OutputTooLarge {
                limit: OUTPUT_LIMIT,
            });
        }
        Ok(output)
    };
    if let Ok(result) = timeout(deadline, work).await {
        result
    } else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        Err(RemoteError::Timeout {
            operation,
            seconds: deadline.as_secs(),
        })
    }
}

pub(crate) fn missing_pipe() -> RemoteError {
    RemoteError::MissingPipe
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Drain even after reaching the limit so a noisy child cannot deadlock on its pipe.
async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<(Vec<u8>, bool), std::io::Error> {
    let mut output = Vec::new();
    let mut buffer = vec![0_u8; 8192];
    let mut exceeded = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok((output, exceeded));
        }
        let remaining = limit.saturating_sub(output.len());
        exceeded |= count > remaining;
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
}

/// Only return recognized failure categories; remote stderr can contain secrets.
pub(crate) fn safe_diagnostic(stderr: &[u8]) -> SshDiagnostic {
    let message = String::from_utf8_lossy(stderr).to_lowercase();
    if message.contains("host key verification failed")
        || message.contains("host identification has changed")
    {
        SshDiagnostic::HostKey
    } else if message.contains("permission denied")
        || message.contains("no supported authentication")
    {
        SshDiagnostic::PermissionDenied
    } else if message.contains("could not resolve hostname") {
        SshDiagnostic::Hostname
    } else if message.contains("connection refused") {
        SshDiagnostic::ConnectionRefused
    } else if message.contains("timed out") || message.contains("no route to host") {
        SshDiagnostic::Unreachable
    } else if message.contains("not found") || message.contains("no such file") {
        SshDiagnostic::MissingPath
    } else if message.contains("exec format") || message.contains("bad cpu type") {
        SshDiagnostic::Architecture
    } else {
        SshDiagnostic::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ssh_option_and_shell_injection() {
        for (host, accepted) in [
            ("build-host", true),
            ("host.example.com", true),
            ("2001:db8::1", true),
            ("-oProxyCommand=evil", false),
            ("user@host", false),
            ("host; touch /tmp/injected", false),
            ("host\nother", false),
            ("", false),
        ] {
            let connection = SshConnection {
                host: host.to_owned(),
                port: 22,
                user: "developer".into(),
                identity_file: None,
            };
            assert_eq!(connection.validate().is_ok(), accepted, "{host:?}");
        }
    }

    #[test]
    fn invalid_connection_fields_have_distinct_variants() {
        let valid = SshConnection {
            host: "host".into(),
            port: 22,
            user: "developer".into(),
            identity_file: None,
        };
        for (connection, expected) in [
            (
                SshConnection {
                    host: "-unsafe".into(),
                    ..valid.clone()
                },
                ConnectionValidationError::Host,
            ),
            (
                SshConnection {
                    user: String::new(),
                    ..valid.clone()
                },
                ConnectionValidationError::Username,
            ),
            (
                SshConnection {
                    port: 0,
                    ..valid.clone()
                },
                ConnectionValidationError::Port,
            ),
            (
                SshConnection {
                    identity_file: Some(String::new()),
                    ..valid
                },
                ConnectionValidationError::IdentityFile,
            ),
        ] {
            let Err(RemoteError::InvalidConnection(actual)) = connection.validate() else {
                panic!("invalid SSH field was not classified as {expected:?}");
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn quotes_remote_directory_as_one_literal() {
        for (value, expected) in [
            ("/home/user/project", "'/home/user/project'"),
            ("/home/a b/it's", "'/home/a b/it'\\''s'"),
            ("$(touch /tmp/evil)", "'$(touch /tmp/evil)'"),
        ] {
            assert_eq!(shell_quote(value), expected);
        }
    }

    #[tokio::test]
    async fn bounds_output_without_stopping_pipe_drain() {
        let data = vec![b'x'; 32_000];
        let result = read_bounded(data.as_slice(), 1024).await;
        let (output, exceeded) = result.unwrap_or_else(|error| panic!("read failed: {error}"));
        assert_eq!(output.len(), 1024);
        assert!(exceeded);
    }

    #[test]
    fn ssh_diagnostics_never_echo_arbitrary_server_output() {
        for diagnostic in [
            "provider secret=sk-private",
            "Permission denied: sk-private",
            "Exec format error: sk-private",
        ] {
            assert!(
                !safe_diagnostic(diagnostic.as_bytes())
                    .to_string()
                    .contains("sk-private")
            );
        }
    }
}
