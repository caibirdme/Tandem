# Dependency refresh (2026-10-22)

This refresh follows deferred remote preparation commit `2a74472` in the separate `codex/update-all-dependencies` worktree. It covers every direct runtime and development npm package, all seven Rust manifests and their transitive lockfile graph, the package manager, compiler, and CI interpreter/actions. Python scripts use only the standard library and local test modules; there are no pip dependencies to update.

## Frontend and package manager

Versions were queried from the official npm registry using `pnpm view`; `pnpm outdated --format json` was recorded before and after. Manifest minimums now match the reviewed stable versions; already-current locked packages are distinguished below.

| Package                         | Previous manifest | New manifest | Previous locked | New locked |
| ------------------------------- | ----------------- | ------------ | --------------- | ---------- |
| `@radix-ui/react-dialog`        | `^1.1.15`         | `^1.3.1`     | `1.3.1`         | `1.3.1`    |
| `@radix-ui/react-dropdown-menu` | `^2.1.16`         | `^2.2.1`     | `2.2.1`         | `2.2.1`    |
| `@radix-ui/react-switch`        | `^1.2.6`          | `^1.4.1`     | `1.4.1`         | `1.4.1`    |
| `@tauri-apps/api`               | `^2.11.1`         | `^2.12.4`    | `2.12.4`        | `2.12.4`   |
| `@tauri-apps/plugin-dialog`     | `^2.6.0`          | `^2.8.3`     | `2.8.3`         | `2.8.3`    |
| `@tauri-apps/plugin-opener`     | `^2.7.2`          | `^2.7.2`     | `2.7.2`         | `2.7.2`    |
| `lucide-react`                  | `^0.468.0`        | `^1.61.0`    | `0.468.0`       | `1.61.0`   |
| `react`                         | `^19.2.0`         | `^19.3.1`    | `19.3.1`        | `19.3.1`   |
| `react-dom`                     | `^19.2.0`         | `^19.3.1`    | `19.3.1`        | `19.3.1`   |
| `react-markdown`                | `^10.1.0`         | `^10.1.0`    | `10.1.0`        | `10.1.0`   |
| `remark-gfm`                    | `^4.0.1`          | `^4.0.1`     | `4.0.1`         | `4.0.1`    |
| `@tauri-apps/cli`               | `^2.12.3`         | `^2.12.4`    | `2.12.3`        | `2.12.4`   |
| `@types/node`                   | `^24.0.0`         | `^24.19.2`   | `24.19.2`       | `24.19.2`  |
| `@types/react`                  | `^19.2.0`         | `^19.3.0`    | `19.3.0`        | `19.3.0`   |
| `@types/react-dom`              | `^19.2.0`         | `^19.3.0`    | `19.3.0`        | `19.3.0`   |
| `@vitejs/plugin-react`          | `^6.1.2`          | `^6.1.2`     | `6.1.2`         | `6.1.2`    |
| `oxfmt`                         | `^0.73.0`         | `^0.74.0`    | `0.73.0`        | `0.74.0`   |
| `oxlint`                        | `^1.88.0`         | `^1.89.0`    | `1.88.0`        | `1.89.0`   |
| `typescript`                    | `~7.0.2`          | `~7.0.2`     | `7.0.2`         | `7.0.2`    |
| `vite`                          | `^8.3.4`          | `^8.3.5`     | `8.3.4`         | `8.3.5`    |
| `vitest`                        | `^4.1.0`          | `^5.0.5`     | `4.1.12`        | `5.0.5`    |

`pnpm` is updated from `12.9.1` to `12.14.0`, including its package-manager lock entries. Both application and transitive dependency resolutions were refreshed, not only manifest ranges.

The user explicitly approved upgrading releases younger than 24 hours. `pnpm-workspace.yaml` records 14 exact-version release-age exceptions for Tauri CLI 2.12.4 and its platform packages, Lucide 1.61.0, and Vite 8.3.5. Frozen installation without these entries was verified to fail the default 24-hour age check at update time. This skips only those versions' waiting period, not integrity checks or the global release-age policy; no wildcard exceptions or unrelated security-policy changes were added. CI includes the new configuration in frontend/bundle inputs. See the official [pnpm dependency-resolution settings](https://github.com/pnpm/pnpm/blob/main/pnpm/docs/settings/dependency-resolution.md).

The only remaining `pnpm outdated` entry is `@types/node` 24.19.2 versus registry latest 26.6.5. It intentionally tracks the project's Node 24 LTS runtime and CI rather than advertising APIs from Node 26, which is still Current. Node 24 remains the supported baseline, in line with the [official production LTS guidance](https://nodejs.org/en/about/previous-releases).

### Major-version review

- [Lucide v1 migration](https://lucide.dev/guide/react/migration): removed brand icons are not imported by Tandem. Existing icon imports typecheck and component/static-render tests pass; icon artwork can differ from the old 0.468 release. No browser visual acceptance is claimed in this sandbox.
- [Vitest 5 migration](https://vitest.dev/guide/migration/): the existing Node 24/Vite 8 toolchain satisfies the new requirements. Mock call history now clears before each test; hoisted mocks must be top-level, and asynchronous assertions must be awaited. The existing suite passes these stricter defaults without compatibility overrides. Tandem does not configure browser providers, benchmarks, custom reporters, or nested test projects affected by other breaking changes.

## Rust

The official crates.io sparse index was queried for every direct dependency, excluding prereleases and yanked versions. Existing compatible major/minor requirements remain unchanged, including `github-copilot-sdk = "1"`, as required by `AGENTS.md`. `cargo update` refreshed the complete graph to the latest versions supported by upstream constraints and Rust 1.99.1.

| Crate                | Previous locked | New locked |
| -------------------- | --------------- | ---------- |
| `cc`                 | `1.8.0`         | `1.9.0`    |
| `crc32fast`          | `1.5.2`         | `1.5.3`    |
| `find-msvc-tools`    | `0.1.14`        | `0.1.15`   |
| `github-copilot-sdk` | `1.0.24`        | `1.0.25`   |
| `ico`                | `0.5.0`         | `0.6.0`    |
| `plist`              | `1.10.1`        | `1.10.3`   |
| `rustls-webpki`      | `0.103.15`      | `0.103.16` |
| `tauri`              | `2.12.4`        | `2.12.5`   |
| `tauri-build`        | `2.7.3`         | `2.7.4`    |
| `tauri-codegen`      | `2.7.3`         | `2.7.4`    |
| `tauri-macros`       | `2.7.3`         | `2.7.4`    |
| `tauri-runtime-wry`  | `2.12.2`        | `2.12.3`   |
| `tauri-winres`       | `0.3.6`         | `0.3.7`    |
| `tokio`              | `1.53.3`        | `1.54.1`   |
| `typed-path`         | `0.12.3`        | `0.12.4`   |
| `ureq`               | `3.4.2`         | `3.5.0`    |
| `ureq-proto`         | `0.6.4`         | `0.6.5`    |
| `webpki-root-certs`  | `1.0.9`         | `1.0.10`   |
| `zip`                | `9.0.4`         | `9.0.5`    |

The no-longer-needed `png` 0.17.16 lock entry was removed.

Six older transitive entries remain constrained by current upstream dependencies: `brotli-decompressor` 6.0.1 (`brotli` requires `~6.0`), `generic-array` 0.14.7 (`crypto-common` pins `=0.14.7`), `matchit` 0.8.4 (`axum` pins `=0.8.4`), and the GTK build chain's `toml` 0.8.2 / `toml_edit` 0.20.2 / `toml_datetime` 0.6.3 (`proc-macro-crate` pins the latter two). A trial resolution of `toml` 0.8.23 confirmed its conflict with the required `toml_datetime =0.6.3`; no unsupported overrides or registry-source patches were applied. These are not stale direct Tandem requirements.

Other direct Rust dependencies were already current. SDK [1.0.25](https://github.com/github/copilot-sdk/releases/tag/v1.0.25) bundles Copilot CLI [1.0.102](https://github.com/github/copilot-cli/releases/tag/v1.0.102), replacing 1.0.101. The official Linux archive was verified against the SHA-256 recorded in the published SDK crate. This CLI release changes subagent approval inheritance, overload retries, and proxy handling; no additional experimental SDK features or permission policy changes are enabled by Tandem. SDK 1.0.25 also scopes `send_and_wait` completion to the accepted request, ignoring idle events before that request starts. The summary no-text fixture now returns a request ID and emits its matching user-message event before idle; an additional stale-idle case still expects timeout. This updates the protocol fixture rather than weakening production completion or error-redaction behavior.

## Toolchains and CI

- Rust remains 1.99.1: the official stable channel manifest still lists this release. Nightly rustfmt remains the existing rolling formatting tool; this update does not introduce a nightly compiler requirement for builds.
- Python CI moves from 3.14 to 3.15, whose current stable maintenance release is [3.15.1](https://www.python.org/downloads/release/python-3151/). Python 3.15 defaults to UTF-8. The scripts remain standard-library-only.
- Every existing GitHub Action major reference is already current: checkout v7 (latest 7.0.1), setup-node v7 (7.1.0), setup-python v7 (7.0.0), upload-artifact v7 (7.0.2), download-artifact v8 (8.0.2), paths-filter v4 (4.0.3), and rust-cache v2 (2.9.3). Their official release pages were checked; the existing rolling major-ref policy is retained. OS runner/container baselines remain unchanged because they encode platform/glibc compatibility, not library versions.

Validation and remaining environment limits are recorded in [Verification](verification.md).
