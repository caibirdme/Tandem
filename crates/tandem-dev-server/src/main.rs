//! Local HTTP host for browser development and application acceptance tests.

mod http;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tandem_core::{App, AppOptions, MemoryVault};

#[derive(Debug, thiserror::Error)]
enum ArgumentError {
    #[error("choose either --version or --help")]
    ConflictingModes,
    #[error("--port requires a value from 1 to 65535")]
    MissingPort,
    #[error("--port must be an integer from 1 to 65535 (received {value:?})")]
    InvalidPortFormat { value: String },
    #[error("--port must be an integer from 1 to 65535 (received {value:?})")]
    InvalidPortNumber {
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("--port must be an integer from 1 to 65535")]
    ZeroPort,
    #[error("--data-dir requires a path")]
    MissingDataDirectory,
    #[error("unknown option: {option}; use --help for development server options")]
    UnknownOption { option: String },
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Arguments(#[from] ArgumentError),
    #[error(transparent)]
    Application(#[from] tandem_core::Error),
    #[error("Cannot locate the development server executable: {0}")]
    ExecutablePath(#[source] std::io::Error),
    #[error(transparent)]
    Http(#[from] http::HttpError),
}

type Result<T> = std::result::Result<T, Error>;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();
    let arguments = arguments(std::env::args().skip(1))?;
    match arguments.mode {
        Mode::Version => {
            println!("tandem-dev-server {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Mode::Help => {
            println!(
                "tandem-dev-server [--port 4317] [--data-dir PATH] [--ephemeral-secrets]\ntandem-dev-server --version | --help\n\nFor the remote Agent host, use tandem-server."
            );
            return Ok(());
        }
        Mode::Http => {}
    }
    let mut options = arguments
        .data_directory
        .map_or_else(AppOptions::native, |directory| {
            Ok(AppOptions::local(directory))
        })?;
    if arguments.ephemeral_secrets {
        options.secrets = Arc::new(MemoryVault::default());
    }
    // SSH deployment needs the remote host, never this HTTP development binary.
    let executable = std::env::current_exe().map_err(Error::ExecutablePath)?;
    options.remote_artifacts.local_server = Some(remote_server_path(&executable));
    let app = App::open(options)?;
    let result = http::serve(Arc::clone(&app), arguments.port)
        .await
        .map_err(Error::from);
    let stop_result = app.shutdown().await.map_err(Error::from);
    result.and(stop_result)
}

fn remote_server_path(executable: &Path) -> PathBuf {
    executable.with_file_name(format!("tandem-server{}", std::env::consts::EXE_SUFFIX))
}

#[derive(Debug, Default, Eq, PartialEq)]
enum Mode {
    #[default]
    Http,
    Version,
    Help,
}

#[derive(Default)]
struct Arguments {
    mode: Mode,
    ephemeral_secrets: bool,
    port: u16,
    data_directory: Option<PathBuf>,
}

fn arguments(
    arguments: impl IntoIterator<Item = String>,
) -> std::result::Result<Arguments, ArgumentError> {
    let mut parsed = Arguments {
        port: 4317,
        ..Arguments::default()
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--version" | "--help" => {
                if parsed.mode != Mode::Http {
                    return Err(ArgumentError::ConflictingModes);
                }
                parsed.mode = if argument == "--version" {
                    Mode::Version
                } else {
                    Mode::Help
                };
            }
            "--ephemeral-secrets" => parsed.ephemeral_secrets = true,
            "--port" => {
                let port = arguments.next().ok_or(ArgumentError::MissingPort)?;
                if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(ArgumentError::InvalidPortFormat { value: port });
                }
                parsed.port = port
                    .parse()
                    .map_err(|source| ArgumentError::InvalidPortNumber {
                        value: port,
                        source,
                    })?;
                if parsed.port == 0 {
                    return Err(ArgumentError::ZeroPort);
                }
            }
            "--data-dir" => {
                let directory = arguments
                    .next()
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or(ArgumentError::MissingDataDirectory)?;
                parsed.data_directory = Some(directory.into());
            }
            other => {
                return Err(ArgumentError::UnknownOption {
                    option: other.into(),
                });
            }
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_options_retain_their_values() -> Result<()> {
        let defaults = arguments([])?;
        assert_eq!(defaults.mode, Mode::Http);
        assert_eq!(defaults.port, 4317);
        for port in ["1", "65535"] {
            let parsed = arguments(
                ["--port", port, "--data-dir", "state", "--ephemeral-secrets"].map(str::to_owned),
            )?;
            assert_eq!(parsed.port.to_string(), port);
            assert_eq!(parsed.data_directory, Some(PathBuf::from("state")));
            assert!(parsed.ephemeral_secrets);
        }
        Ok(())
    }

    #[test]
    fn informational_modes_do_not_select_the_http_service() -> Result<()> {
        for (input, expected) in [
            (vec!["--help"], Mode::Help),
            (vec!["--version"], Mode::Version),
            (vec!["--port", "4317", "--help"], Mode::Help),
        ] {
            assert_eq!(
                arguments(input.into_iter().map(str::to_owned))?.mode,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn invalid_ports_paths_and_remote_modes_are_typed() {
        for option in ["--stdio", "--install-runtime"] {
            assert!(matches!(
                arguments([option.into()]),
                Err(ArgumentError::UnknownOption { option: actual }) if actual == option
            ));
        }
        assert!(matches!(
            arguments(["--port".into()]),
            Err(ArgumentError::MissingPort)
        ));
        for value in ["", "-1", "+4317", "invalid"] {
            assert!(matches!(
                arguments(["--port", value].map(str::to_owned)),
                Err(ArgumentError::InvalidPortFormat { value: actual }) if actual == value
            ));
        }
        for value in ["65536", "99999999999999999999"] {
            assert!(matches!(
                arguments(["--port", value].map(str::to_owned)),
                Err(ArgumentError::InvalidPortNumber { value: actual, source })
                    if actual == value && *source.kind() == std::num::IntErrorKind::PosOverflow
            ));
        }
        assert!(matches!(
            arguments(["--port", "0"].map(str::to_owned)),
            Err(ArgumentError::ZeroPort)
        ));
        for input in [
            vec!["--data-dir"],
            vec!["--data-dir", ""],
            vec!["--data-dir", "--help"],
        ] {
            assert!(matches!(
                arguments(input.into_iter().map(str::to_owned)),
                Err(ArgumentError::MissingDataDirectory)
            ));
        }
        for input in [
            ["--help", "--version"],
            ["--help", "--help"],
            ["--version", "--help"],
        ] {
            assert!(matches!(
                arguments(input.map(str::to_owned)),
                Err(ArgumentError::ConflictingModes)
            ));
        }
    }

    #[test]
    fn ssh_artifact_is_the_sibling_remote_server() {
        let directory = Path::new("development artifacts");
        let executable =
            directory.join(format!("tandem-dev-server{}", std::env::consts::EXE_SUFFIX));
        let expected = directory.join(format!("tandem-server{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(remote_server_path(&executable), expected);
        assert_ne!(remote_server_path(&executable), executable);
    }
}
