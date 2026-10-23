Run `pnpm remote:build` or `just remote-build` to prepare remote programs here. Tauri builds run this preparation automatically, including `pnpm tauri build --no-bundle`.

Each `<target>/` directory contains `tandem-server` and `artifact.json`. Preparation verifies the current source fingerprint, target architecture, and executable SHA-256 before reusing an artifact. Keep both files together when copying a prepared target between build machines.

The default build prepares Linux x64 and, on macOS, the native macOS target. Cross-platform Linux builds require a running Docker-compatible engine; valid cached artifacts do not. Release CI supplies Linux x64 and Apple Silicon macOS targets with the same metadata. See [Packaging and remote artifacts](../../docs/development.md#packaging-and-remote-artifacts) for additional targets.

Tauri copies these files under `remote/<target>/` in its resources, including beside an executable built without an installer. SSH deploys the selected program to `~/.local/share/tandem/bin/tandem-server`. Only Linux and macOS are supported as remote hosts.
