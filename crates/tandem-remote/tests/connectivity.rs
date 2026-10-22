//! A fake SSH executable verifies the connectivity boundary without a remote host.
#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

use tandem_remote::{ArtifactLocator, RemoteClient, SshConnection};

#[test]
fn connectivity_does_not_deploy_or_prepare_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::temp_dir().join(format!(
        "tandem-connectivity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&directory)?;
    let result = check_probe(&directory);
    std::fs::remove_dir_all(&directory)?;
    result
}

fn check_probe(directory: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let ssh = directory.join("ssh");
    std::fs::write(
        &ssh,
        "#!/bin/sh\nfor argument do command=$argument; done\nprintf '%s\\n' \"$command\" >> \"$TANDEM_TEST_SSH_LOG\"\n[ \"$command\" = true ] || exit 71\nexit \"$TANDEM_TEST_SSH_EXIT\"\n",
    )?;
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700))?;
    for status in ["0", "255"] {
        let log = directory.join(format!("probe-{status}.log"));
        let output = Command::new(std::env::current_exe()?)
            .args(["--exact", "connectivity_probe_child", "--nocapture"])
            .env("PATH", directory)
            .env("TANDEM_TEST_SSH_LOG", &log)
            .env("TANDEM_TEST_SSH_EXIT", status)
            .output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read_to_string(log)?, "true\n");
    }
    Ok(())
}

#[tokio::test]
async fn connectivity_probe_child() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(status) = std::env::var("TANDEM_TEST_SSH_EXIT") else {
        return Ok(());
    };
    let client = RemoteClient::new(
        SshConnection {
            host: "test-host".into(),
            port: None,
            user: None,
            identity_file: None,
        },
        ArtifactLocator {
            directory: Some(PathBuf::from("/nonexistent-tandem-artifacts")),
            ..ArtifactLocator::default()
        },
    )?;
    let result = client.check_connection().await;
    if status == "0" {
        result?;
    } else {
        assert!(matches!(
            result,
            Err(tandem_remote::RemoteError::ConnectionFailed {
                operation: "connection test",
                ..
            })
        ));
    }
    Ok(())
}
