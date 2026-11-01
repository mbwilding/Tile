# Agent instructions

Tile is a Windows tiling window manager with a built-in key remapper, written in Rust. See
`README.md` for an overview and `docs/development.md` for the crate layout.

## Before finishing a change

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

All three must be clean. `unsafe` is allowed only in `tile-input`, `tile-shell` and `tile-sys`,
each block with a `// SAFETY:` comment.

## Documentation

Everything has documentation, and docs are updated in the same change as the code:

- Every public item gets a doc comment. New code is not finished without them.
- Comments in code stay minimal; say why, not what.
- **`docs/configuration.md` must match the real defaults.** It holds a reference of every setting
  and a copy-paste default config between the `default-config` markers. When you add, remove or
  change a setting or a default (in `crates/tile-config/src/schema.rs`), update that block and the
  reference tables. `cargo test -p tile-config --test docs` fails if the block drifts.
- **`examples/default.yml` must match too**: every setting at its default and every default bind
  listed. `cargo test -p tile-config --test examples` checks it and that every example loads.
- Keybinds, dispatchers and key names go in `docs/keybinds.md` (and the default bind table must
  match `DEFAULT_BINDS`); layouts in `docs/layouts.md`; rules in `docs/window-rules.md`;
  remapping in `docs/key-remapping.md`; the `tile` command line in `docs/cli.md`; anything touching how
  Tile interacts with Windows in `docs/windows-integration.md`.
- `README.md` stays an overview and quickstart that links to `docs/`. Don't grow it with
  reference material; add that to a docs page and link it.
- A new tray item, file or user-visible behaviour is documented where users will look for it.

## Style

- Australian English in docs and comments.
- Match the surrounding code's naming and idiom.
