//! Remote Agent host. Stdout is reserved for the private stdio protocol.

#[cfg(unix)]
mod daemon;
mod error;
mod stdio;

use error::{Result, ServerError};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();
    let mode = arguments(std::env::args().skip(1))?;
    if print_information(&mode) {
        return Ok(());
    }
    let mut builder = if matches!(mode, Mode::Proxy | Mode::InstallRuntime) {
        tokio::runtime::Builder::new_current_thread()
    } else {
        tokio::runtime::Builder::new_multi_thread()
    };
    let runtime = builder.enable_all().build()?;
    let result = runtime.block_on(run(mode));
    // A remote socket can close while Tokio's blocking stdin reader is pending.
    // Bound shutdown so that this reader cannot keep an SSH proxy alive forever.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

async fn run(mode: Mode) -> Result<()> {
    match mode {
        Mode::Stdio => stdio::run(false).await,
        Mode::Proxy => run_proxy().await,
        Mode::Daemon => run_daemon().await,
        Mode::InstallRuntime => install_runtime().await,
        Mode::Version | Mode::Help => Ok(()),
    }
}

fn print_information(mode: &Mode) -> bool {
    match mode {
        Mode::Version => {
            println!(
                "tandem-server {} (protocol {})",
                env!("CARGO_PKG_VERSION"),
                tandem_protocol::PROTOCOL_VERSION
            );
            true
        }
        Mode::Help => {
            println!(
                "tandem-server [--proxy | --daemon | --stdio | --install-runtime | --version | --help]\n\nSSH connections use --proxy to connect to the shared Unix daemon. The default --stdio mode runs an isolated host. For the development HTTP API, use tandem-dev-server."
            );
            true
        }
        _ => false,
    }
}

#[cfg(unix)]
async fn run_proxy() -> Result<()> {
    daemon::proxy().await.map_err(Into::into)
}

#[cfg(unix)]
async fn run_daemon() -> Result<()> {
    daemon::run().await
}

#[cfg(unix)]
async fn install_runtime() -> Result<()> {
    daemon::install_runtime().await.map_err(Into::into)
}

#[cfg(not(unix))]
fn run_proxy() -> std::future::Ready<Result<()>> {
    std::future::ready(Err(ServerError::UnixOnlyMode))
}

#[cfg(not(unix))]
fn run_daemon() -> std::future::Ready<Result<()>> {
    std::future::ready(Err(ServerError::UnixOnlyMode))
}

#[cfg(not(unix))]
async fn install_runtime() -> Result<()> {
    stdio::run(true).await
}

#[derive(Debug, Eq, PartialEq)]
enum Mode {
    Stdio,
    Proxy,
    Daemon,
    InstallRuntime,
    Version,
    Help,
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<Mode> {
    let mut mode = None;
    for argument in arguments {
        let requested = match argument.as_str() {
            "--stdio" => Mode::Stdio,
            "--proxy" => Mode::Proxy,
            "--daemon" => Mode::Daemon,
            "--install-runtime" => Mode::InstallRuntime,
            "--version" => Mode::Version,
            "--help" => Mode::Help,
            other => {
                return Err(ServerError::UnknownOption {
                    option: other.to_owned(),
                });
            }
        };
        if mode.is_some() {
            return Err(ServerError::ConflictingModes);
        }
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
            (vec!["--proxy"], Mode::Proxy),
            (vec!["--daemon"], Mode::Daemon),
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
            vec!["--proxy", "--daemon"],
            vec!["--help", "--stdio"],
            vec!["--version", "--help"],
            vec!["unknown"],
        ] {
            assert!(
                matches!(
                    arguments(input.iter().map(ToString::to_string)),
                    Err(ServerError::UnknownOption { .. } | ServerError::ConflictingModes)
                ),
                "{input:?}"
            );
        }
    }
}
