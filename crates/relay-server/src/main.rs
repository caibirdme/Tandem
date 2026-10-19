//! Remote Agent host. Stdout is reserved for the private stdio protocol.

mod stdio;

use anyhow::{Result, bail, ensure};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();
    match arguments(std::env::args().skip(1))? {
        Mode::Stdio => stdio::run(false).await,
        Mode::InstallRuntime => stdio::run(true).await,
        Mode::Version => {
            println!("relay-server {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Mode::Help => {
            println!(
                "relay-server [--stdio | --install-runtime | --version | --help]\n\nThe default mode is --stdio. For the development HTTP API, use relay-dev-server."
            );
            Ok(())
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Mode {
    Stdio,
    InstallRuntime,
    Version,
    Help,
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<Mode> {
    let mut mode = None;
    for argument in arguments {
        let requested = match argument.as_str() {
            "--stdio" => Mode::Stdio,
            "--install-runtime" => Mode::InstallRuntime,
            "--version" => Mode::Version,
            "--help" => Mode::Help,
            other => bail!("unknown option: {other}; use --help for remote server options"),
        };
        ensure!(mode.is_none(), "choose only one relay-server mode");
        mode = Some(requested);
    }
    Ok(mode.unwrap_or(Mode::Stdio))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_explicit_and_informational_requests_do_not_select_a_service() -> Result<()> {
        for (input, expected) in [
            (vec![], Mode::Stdio),
            (vec!["--stdio"], Mode::Stdio),
            (vec!["--install-runtime"], Mode::InstallRuntime),
            (vec!["--version"], Mode::Version),
            (vec!["--help"], Mode::Help),
        ] {
            assert_eq!(arguments(input.into_iter().map(str::to_owned))?, expected);
        }
        Ok(())
    }

    #[test]
    fn remote_host_rejects_http_flags_and_conflicting_modes() {
        for input in [
            vec!["--port", "4317"],
            vec!["--data-dir", "state"],
            vec!["--ephemeral-secrets"],
            vec!["--stdio", "--install-runtime"],
            vec!["--help", "--stdio"],
            vec!["--version", "--help"],
            vec!["unknown"],
        ] {
            assert!(
                arguments(input.iter().map(ToString::to_string)).is_err(),
                "{input:?}"
            );
        }
    }
}
