Tandem remote binaries are placed here by `just remote-build` or the release workflow.
They are bundled under `remote/<target>/tandem-server` and deployed through SSH.
The deployed executable is `~/.local/share/tandem/bin/tandem-server`.
Only Linux and macOS are supported as remote hosts.
