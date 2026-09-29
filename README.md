# LazyRAD

A Visual Basic 6-style RAD IDE in Rust, built on the [xui](https://github.com/va1erian/xui)
UI toolkit with [Rhai](https://rhai.rs) as its scripting language. You draw a
form, double-click a button, write the handler, press F5, debug it, then export a
single executable.

See [PLAN.md](PLAN.md) for the architecture, key decisions, milestones and the
xui gaps the project tracks.

## Workspace

| Crate | Role |
|---|---|
| `lazyrad-project` | project + form file model (`.lrp`/`.lfm`), load/save, validation |
| `lazyrad-runtime` | Rhai engine setup, stdlib, form instantiation, event binding |
| `lazyrad-player` (bin) | the runtime host: runs a project dir, a payload, or `--debug` |
| `lazyrad-designer` | the xui form-designer surface, toolbox and property grid |
| `lazyrad-ide` (bin) | the IDE shell |

## Building

```text
cargo build
cargo run -p lazyrad-ide
cargo run -p lazyrad-player
```

Both binaries open an empty xui window until their milestones land.

## Checks

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The same commands run in CI on Windows and Linux (`.github/workflows/ci.yml`).
