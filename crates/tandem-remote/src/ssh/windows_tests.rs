use std::{
    error::Error,
    io,
    path::PathBuf,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

use super::SshConnection;

const DEADLINE: Duration = Duration::from_secs(30);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct ConsoleProbe {
    directory: PathBuf,
}

impl Drop for ConsoleProbe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl ConsoleProbe {
    async fn build() -> Result<Self, Box<dyn Error>> {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tandem-console-probe-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&directory)?;
        let probe = Self { directory };
        let system_root = std::env::var_os("SystemRoot")
            .ok_or_else(|| io::Error::other("SystemRoot is unavailable"))?;
        let powershell =
            PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut compiler = Command::new(powershell);
        compiler
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(
                r#"$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.Runtime.InteropServices;
public static class ConsoleProbe {
    [DllImport("kernel32.dll")]
    private static extern IntPtr GetConsoleWindow();
    public static void Main() {
        Console.WriteLine("tandem-console-probe:" +
            (GetConsoleWindow() == IntPtr.Zero ? "none" : "attached"));
        Console.WriteLine(Console.ReadLine());
        Console.Error.WriteLine("tandem-console-probe:stderr");
    }
}
'@
$output = Join-Path $env:TANDEM_CONSOLE_PROBE_DIRECTORY 'ssh.exe'
Add-Type -TypeDefinition $source -Language CSharp -OutputAssembly $output -OutputType ConsoleApplication
"#,
            )
            .env("TANDEM_CONSOLE_PROBE_DIRECTORY", &probe.directory)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = timeout(DEADLINE, compiler.output()).await??;
        assert!(
            output.status.success(),
            "console probe compilation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(probe.directory.join("ssh.exe").is_file());
        Ok(probe)
    }
}

#[tokio::test]
async fn ssh_process_has_no_console_and_preserves_all_pipes() -> Result<(), Box<dyn Error>> {
    let probe = ConsoleProbe::build().await?;
    let connection = SshConnection {
        host: "console-probe.invalid".into(),
        port: Some(22),
        user: Some("console-probe".into()),
        identity_file: None,
    };
    // Resolve only the isolated console executable; no SSH server or user keys are used.
    let mut child = connection
        .command("true")
        .env("PATH", &probe.directory)
        .current_dir(&probe.directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("SSH input pipe is unavailable"))?;
    input.write_all(b"tandem-console-probe:stdin\n").await?;
    input.shutdown().await?;
    drop(input);
    let output = timeout(DEADLINE, child.wait_with_output()).await??;
    assert!(output.status.success(), "console probe failed");
    assert_eq!(
        String::from_utf8(output.stdout)?
            .lines()
            .collect::<Vec<_>>(),
        ["tandem-console-probe:none", "tandem-console-probe:stdin"]
    );
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "tandem-console-probe:stderr"
    );
    Ok(())
}
