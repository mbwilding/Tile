# Development

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

`cargo run` starts the daemon (the default member); pass `--workspace` to build or test
everything. See the README for running a build without installing it.

Aliases in `.cargo/config.toml`:

| Alias | Runs | Does |
| --- | --- | --- |
| `cargo release` | `cargo build --release --workspace` | builds `Tile.exe` in release |
| `cargo deploy` | `cargo run --release -p tile -- install` | installs the build from `cargo release` |
| `cargo deploy-dev` | `cargo run -p tile -- install` | installs the debug build from `cargo build --workspace` |

## Project layout

| Crate | Purpose |
| --- | --- |
| `tile-core` | Platform-independent window manager model: state, one state per virtual desktop, layouts with their defaults and limits, dispatchers, geometry and easing (unit tested) |
| `tile-remap` | The key remapper: layers and overloads as a pure state machine, its config parser and the key names shared with keybinds (unit tested) |
| `tile-config` | YAML schema, keybind parsing, window rules, the tray switches and file watching (tested against `docs/` and `examples/`) |
| `tile-ipc` | Named-pipe JSON protocol |
| `tile-launcher` | The app launcher: the search, calculator and system items (tested), and its window |
| `tile-platform` | Win32 for the window manager: window events, placement and animation, the keybind hook, the Win+drag mouse hook, tray, the workspace indicator, the sign-in programs, virtual desktops |
| `tile-input` | The low-level keyboard and mouse hooks, `SendInput` and hiding the pointer |
| `tile-shell` | What Explorer provides, for shell mode: the notification area host (`Shell_TrayWnd`), the black backdrop, shell icons and the Store app frames |
| `tile-sys` | Volume, battery, network, Wi-Fi and Bluetooth, the single-instance mutex, the console and window messages |
| `tile` | The program (`Tile.exe`): the daemon, the command line and install/uninstall |

The crates layer upward: `tile-core`, `tile-remap` and `tile-config` know nothing of Windows;
`tile-input`, `tile-shell` and `tile-sys` are the raw Win32 calls; `tile-platform` and
`tile-launcher` build on them; `tile` joins everything in the daemon.

All `unsafe` lives in `tile-input`, `tile-shell` and `tile-sys`, each block with a `// SAFETY:`
comment; every other crate has `unsafe_code = "forbid"`. The rest of Win32 goes through safe
wrappers:
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
and publishes a GitHub release holding `Tile.exe`. No
tag is pushed: the version is the major and minor from `Cargo.toml`, with the workflow run number
as the patch (`0.1.7`), and the release creates its own tag. To release by hand, run it from the
Actions tab or with `gh workflow run release.yml`. Bump the major or minor in `Cargo.toml` for a
bigger release.

## Documentation

The docs change with the code; [`AGENTS.md`](../AGENTS.md#documentation) lists what goes where
and the tests that keep `docs/configuration.md` and `examples/default.yml` in step with the
defaults.

## Not yet

- `Win` + right-drag to resize a window in the direction the mouse moves, pushing the
  neighbouring tiles along.
