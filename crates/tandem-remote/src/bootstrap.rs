use std::{path::PathBuf, time::Duration};

use serde::Serialize;

use crate::{
    RemoteError, RemoteTransport, SshConnection,
    ssh::{run, shell_quote},
};

const PROBE: &str = r#"set -eu
printf '%s\000%s\000' "$(uname -s)" "$(uname -m)"
if [ -x "$HOME/.local/share/tandem/bin/tandem-server" ]; then
  "$HOME/.local/share/tandem/bin/tandem-server" --version || true
fi
"#;
const RUNTIME_COMMAND: &str = r#"export PATH="$HOME/.local/share/tandem/bin:$HOME/.local/bin:$PATH"; exec "$HOME/.local/share/tandem/bin/tandem-server" --install-runtime"#;
const SERVER_COMMAND: &str = r#"export PATH="$HOME/.local/share/tandem/bin:$HOME/.local/bin:$PATH"; exec "$HOME/.local/share/tandem/bin/tandem-server" --stdio"#;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        let environment_directory = std::env::var_os("TANDEM_REMOTE_ARTIFACTS")
            .or_else(|| std::env::var_os("RELAY_REMOTE_ARTIFACTS"))
            .map(PathBuf::from);
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
                return Err(RemoteError::InvalidArtifact {
                    path: path.display().to_string(),
                    reason: "expected a non-empty executable smaller than 512 MiB",
                });
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

    /// Verify host access, atomically deploy the matching server, and prepare its runtime.
    ///
    /// # Errors
    /// Returns an SSH, unsupported-platform, missing-artifact, or runtime setup error.
    pub async fn bootstrap(&self) -> Result<BootstrapReport, RemoteError> {
        let output = run(
            &self.connection,
            "environment probe",
            PROBE,
            None,
            PROBE_TIMEOUT,
        )
        .await?;
        let (platform, version) = parse_probe(&output)?;
        let deployed = version.as_deref() != Some(VERSION);
        if deployed {
            let artifact = self.artifacts.locate(&platform).await?;
            run(
                &self.connection,
                "server deployment",
                &upload_command(),
                Some(&artifact),
                INSTALL_TIMEOUT,
            )
            .await?;
        }
        run(
            &self.connection,
            "runtime initialization",
            RUNTIME_COMMAND,
            None,
            INSTALL_TIMEOUT,
        )
        .await?;
        Ok(BootstrapReport {
            platform,
            server_version: VERSION.into(),
            deployed,
            runtime_ready: true,
        })
    }

    /// Bootstrap and start a remote server, with all RPC traffic inside the SSH channel.
    ///
    /// # Errors
    /// Returns a bootstrap or process startup error.
    pub async fn connect(&self) -> Result<RemoteTransport, RemoteError> {
        self.bootstrap().await?;
        self.open_transport()
    }

    /// Start a previously bootstrapped server without repeating deployment checks.
    ///
    /// # Errors
    /// Returns an error if the system SSH executable cannot be launched.
    pub fn open_transport(&self) -> Result<RemoteTransport, RemoteError> {
        RemoteTransport::spawn(self.connection.command(SERVER_COMMAND))
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
        if directories.len() != 1 {
            return Err(RemoteError::Protocol(
                "expected one canonical directory".into(),
            ));
        }
        directories
            .pop()
            .ok_or_else(|| RemoteError::Protocol("missing canonical directory".into()))
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

fn parse_probe(output: &[u8]) -> Result<(RemotePlatform, Option<String>), RemoteError> {
    let response = std::str::from_utf8(output)
        .map_err(|_| RemoteError::Protocol("environment probe was not UTF-8".into()))?;
    let mut parts = response.splitn(3, '\0');
    let os = parts
        .next()
        .ok_or_else(|| RemoteError::Protocol("missing remote OS".into()))?;
    let architecture = parts
        .next()
        .ok_or_else(|| RemoteError::Protocol("missing remote architecture".into()))?;
    let version = parts
        .next()
        .ok_or_else(|| RemoteError::Protocol("incomplete environment probe".into()))?;
    let version = version
        .trim()
        .strip_prefix("tandem-server ")
        .map(str::to_owned);
    Ok((RemotePlatform::from_uname(os, architecture)?, version))
}

fn upload_command() -> String {
    format!(
        r#"set -eu
umask 077
destination="$HOME/.local/share/tandem/bin"
mkdir -p "$destination"
temporary=$(mktemp "$destination/.tandem-server.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cat > "$temporary"
chmod 700 "$temporary"
actual_version=$("$temporary" --version)
[ "$actual_version" = {} ] || {{ printf '%s\n' 'tandem-server version mismatch' >&2; exit 1; }}
mv -f "$temporary" "$destination/tandem-server"
"#,
        shell_quote(&format!("tandem-server {VERSION}"))
    )
}

fn directory_command(path: &str, list: bool) -> Result<String, RemoteError> {
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(RemoteError::InvalidDirectory(
            "use a non-empty path without NUL bytes, at most 4096 bytes long".into(),
        ));
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
        return Err(RemoteError::Protocol(
            "incomplete directory response".into(),
        ));
    }
    output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let path = std::str::from_utf8(part)
                .map_err(|_| RemoteError::Protocol("directory names must be valid UTF-8".into()))?;
            if !path.starts_with('/') {
                return Err(RemoteError::Protocol(
                    "remote directory was not absolute".into(),
                ));
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
    fn probe_does_not_mistake_banners_or_other_binaries_for_our_version() {
        for (output, expected) in [
            ("Linux\0x86_64\0tandem-server 0.1.0\n", Some("0.1.0")),
            ("Linux\0x86_64\0", None),
            ("Linux\0x86_64\0other-server 0.1.0\n", None),
            ("Linux\0x86_64\0relay-server 0.1.4\n", None),
        ] {
            let (_, version) = parse_probe(output.as_bytes())
                .unwrap_or_else(|error| panic!("probe failed: {error}"));
            assert_eq!(version.as_deref(), expected);
        }
        assert!(parse_probe(b"Welcome\nLinux\0x86_64\0").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn shell_directory_discovery_preserves_literals_and_expands_only_home()
    -> Result<(), Box<dyn std::error::Error>> {
        struct TestDirectory(PathBuf);
        impl Drop for TestDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = TestDirectory(std::env::temp_dir().join(format!(
            "tandem-directory-test-{}-{timestamp}",
            std::process::id()
        )));
        std::fs::create_dir(&directory.0)?;
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
