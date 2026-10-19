//! Local HTTP host for browser development and application acceptance tests.

mod http;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use tandem_core::{App, AppOptions, MemoryVault};

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
    options.remote_artifacts.local_server = Some(remote_server_path(&std::env::current_exe()?));
    let app = App::open(options)?;
    let result = http::serve(Arc::clone(&app), arguments.port).await;
    let stop_result = app.shutdown().await;
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

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<Arguments> {
    let mut parsed = Arguments {
        port: 4317,
        ..Arguments::default()
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--version" | "--help" => {
                ensure!(
                    parsed.mode == Mode::Http,
                    "choose either --version or --help"
                );
                parsed.mode = if argument == "--version" {
                    Mode::Version
                } else {
                    Mode::Help
                };
            }
            "--ephemeral-secrets" => parsed.ephemeral_secrets = true,
            "--port" => {
                let port = arguments
                    .next()
                    .context("--port requires a value from 1 to 65535")?;
                ensure!(
                    port.bytes().all(|byte| byte.is_ascii_digit()),
                    "--port must be an integer from 1 to 65535"
                );
                parsed.port = port
                    .parse()
                    .context("--port must be an integer from 1 to 65535")?;
                ensure!(
                    parsed.port != 0,
                    "--port must be an integer from 1 to 65535"
                );
            }
            "--data-dir" => {
                let directory = arguments
                    .next()
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .context("--data-dir requires a path")?;
                parsed.data_directory = Some(directory.into());
            }
            other => bail!("unknown option: {other}; use --help for development server options"),
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
    fn invalid_ports_paths_and_remote_modes_are_rejected() {
        for input in [
            vec!["--stdio"],
            vec!["--install-runtime"],
            vec!["--port"],
            vec!["--port", "0"],
            vec!["--port", "65536"],
            vec!["--port", "-1"],
            vec!["--port", "+4317"],
            vec!["--port", "invalid"],
            vec!["--data-dir"],
            vec!["--data-dir", "--help"],
            vec!["--help", "--version"],
        ] {
            assert!(
                arguments(input.iter().map(ToString::to_string)).is_err(),
                "{input:?}"
            );
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
