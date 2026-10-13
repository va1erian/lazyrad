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
| `lazyrad-debug-proto` | the IDE <-> player debug protocol: one JSON object per line |
| `lazyrad-player` (bin) | the runtime host: runs a project dir or a payload, `--watch` reloads, `--debug` runs under the debugger |
| `lazyrad-designer` | the xui form-designer surface, toolbox and property grid |
| `lazyrad-view` | renderer-neutral size classes, Rhai bindings and a patch diff any renderer can apply |
| `lazyrad-ide` (bin) | the IDE shell |

## Samples

| Sample | Shows |
|---|---|
| `examples/hello` | a form, a handler and a standard module |
| `examples/calculator` | many buttons sharing one module |
| `examples/todo` | a list whose rows a script adds and removes |
| `examples/brickbreaker` | a game on a `Canvas`: a 60 fps frame loop, keyboard and mouse input, drawing |

Run one with `cargo run -p lazyrad-player -- examples/brickbreaker`, or open its
`.lrp` in the IDE and press F5.

## Building

```text
cargo build
cargo run -p lazyrad-ide
cargo run -p lazyrad-player
```

Started with no project, the player opens an empty xui window.

## Debugging

Click in the code editor and press **F9** to set a breakpoint, then **F5**: with a
breakpoint set, *Start* runs the program under the debugger (`lazyrad-player --debug`),
without one it runs normally with hot reload. *Run -> Start with Debugging* (Ctrl+F5)
always uses the debugger, so *Break* (Ctrl+Shift+F5) and break-on-error work with no
breakpoint. When the program stops, the Locals, Call Stack and Immediate tabs below
the code fill in; **F5** continues, **F8** steps into, **Shift+F8** over,
**Ctrl+Shift+F8** out, and **Shift+F5** ends the run. Breakpoints are kept for the
session, not saved in the project.

```text
lazyrad-player --debug <project dir | .lrp>   # speaks the protocol on stdin/stdout
```

The protocol (`crates/lazyrad-debug-proto`) is one JSON object per line: commands
(`set_breakpoints`, `continue`, `step_into`, `step_over`, `step_out`, `pause`, `stop`,
`evaluate`) on the player's stdin, events (`ready`, `stopped`, `resumed`, `evaluated`,
`output`, `error`, `exited`) on its stdout, every message carrying `"v":1`. While the
program is stopped its window is frozen, as in VB6; the IDE is another process and
stays responsive. See PLAN.md section 7 for what it can and cannot do.

## Checks

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The same commands run in CI on Windows and Linux (`.github/workflows/ci.yml`).
