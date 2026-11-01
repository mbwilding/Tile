# Development

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
```

`cargo run` starts the daemon (the default member); pass `--workspace` to build or test
everything. See the README for running a build without installing it.

Aliases in `.cargo/config.toml`:

| Alias | Runs | Does |
| --- | --- | --- |
| `cargo release` | `cargo build --release --workspace` | builds `tile.exe` in release |
| `cargo deploy` | `cargo run --release -p tile -- install` | installs the build from `cargo release` |
| `cargo deploy-dev` | `cargo run -p tile -- install` | installs the debug build from `cargo build --workspace` |

## Project layout

| Crate | Purpose |
| --- | --- |
| `tile-core` | Platform-independent state, layouts, dispatchers, easing and the launcher's search (unit tested) |
| `tile-remap` | The remapping engine: keyd's layers and overloads as a pure state machine (tested) |
| `tile-config` | YAML schema, keybind parsing, window rules, remap layers, the tray switches and file watching |
| `tile-hook` | The low-level keyboard and mouse hooks, `SendInput`, shell icons, the notification area host (`Shell_TrayWnd`), volume, battery, network and radios and other raw calls: the only `unsafe` |
| `tile-platform` | Win32: window events, placement and animation, the keybind hook, the Win+drag mouse hook, tray, the launcher window, the sign-in programs, virtual desktops |
| `tile-ipc` | Named-pipe JSON protocol |
| `tile` | The program (`tile.exe`): the daemon, the command line and install/uninstall |

All `unsafe` lives in `tile-hook`, each block with a `// SAFETY:` comment; every other crate has
`unsafe_code = "forbid"`. The rest of Win32 goes through safe wrappers:
[`winsafe`](https://crates.io/crates/winsafe),
[`wineventhook`](https://crates.io/crates/wineventhook),
[`uiautomation`](https://crates.io/crates/uiautomation),
[`tray-icon`](https://crates.io/crates/tray-icon) and
[`winvd`](https://crates.io/crates/winvd).

`tile-core` holds the whole model: `State` owns monitors, workspaces and window membership, and
every change returns a list of effects (place, show, hide, focus, close, exec) for the daemon to
apply. That keeps the layouts and dispatchers testable without Windows.

## CI

Every pull request runs the `CI` workflow: `yamllint --strict .` (rules in `.yamllint.yml`),
`cargo fmt --check`, clippy with warnings denied, and the tests, the same checks as
[`AGENTS.md`](../AGENTS.md). Pushes to `main` run them in the `Release` workflow, before it builds.

## Releasing

Every push to `main` that changes more than docs and examples runs the `Release` workflow, which tests, builds
and publishes a GitHub release holding `tile.zip`, which has `tile.exe`. No
tag is pushed: the version is the major and minor from `Cargo.toml`, with the workflow run number
as the patch (`0.1.7`), and the release creates its own tag. To release by hand, run it from the
Actions tab or with `gh workflow run release.yml`. Bump the major or minor in `Cargo.toml` for a
bigger release.

## Documentation

Everything is documented, and the docs change with the code:

- Doc comments on every public item.
- `docs/configuration.md` lists each setting and a copy-paste default config. A test
  (`crates/tile-config/tests/docs.rs`) fails when that block and the real defaults disagree, so
  a new setting means updating the block.
- Keys and dispatchers go in `docs/keybinds.md`, layouts in `docs/layouts.md`, and so on. The
  README stays an overview and quickstart, linking to these.

## Not yet

- `Win` + right-drag to resize a window in the direction the mouse moves, pushing the
  neighbouring tiles along.
