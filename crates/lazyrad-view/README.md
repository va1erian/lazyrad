# lazyrad-view

The renderer-neutral core of LazyRAD's responsive, state-bound UI layer
(lane B of the modern-UI plan). It turns a tree of nodes, the form's `state` and
the window's environment into **patches** that any renderer can apply.

```text
  Tree (nodes, bind, responsive)      state: Map     Environment
             └───────────────┬───────────────┴────────────┘
                         Session::build / update
                             │  Bindings: Rhai expressions, no side effects
                             ▼
                    Vec<Patch> + Vec<BindDiagnostic>
                             │
                     Renderer::apply
```

## What is in it

| Module | Provides |
|---|---|
| `size_class` | `SizeClass` (Material 3 widths: 600 / 840 / 1200 / 1600 dp, lower edge inclusive) and `Responsive`, the mobile-first cascade |
| `environment` | `Environment { width, height, dark, text_scale }` and the read-only `env` map scripts see (no `platform`) |
| `value` | `Value { Bool, Int, Float, Text, Enum }` and its Rhai conversion; `1` and `1.0` are never coerced |
| `tree` | `Node` and the validated `Tree` (unique names, one kind per property) |
| `bindings` | `Bindings`, the side-effect-free evaluator: expressions over `state`, `props` and `env` only, compiled once, with operation, depth and size limits |
| `patch` | `Patch { Create, Set, Unset, Remove, Move }` and the stable line-based `trace` format |
| `session` | `Session`: `build`, `update`, `update_with`, the failure policy and the diff |
| `renderer` | the `Renderer` trait and `RecordingRenderer` for golden patch traces |

## Rules it keeps

- **Resolution:** a property is its literal, overridden by the responsive patch
  cascaded for the current size class, overridden by its binding's value.
- **Failure is local:** a failing binding keeps its last good value and reports
  one `BindDiagnostic` per turn; nothing panics and no other node is affected.
- **Only what can have changed is re-evaluated:** bindings that read `env` run
  when the environment changes, bindings that read `state` run when what they
  can see of it changes.
- **No platform:** only `rhai` and `thiserror`, and no file, thread, process,
  environment, network, I/O or clock access (`tests/no_platform_apis.rs` scans
  the sources, including grouped `use std::{...}` paths, and the manifest).
  **Compiling for `wasm32-unknown-unknown` is not verified**: no wasm target is
  installed here or in CI, so that scan and `cargo tree --target
  wasm32-unknown-unknown` (which shows only Rhai's own `web-time` and
  `wasm-bindgen` edges) are the only evidence. The static musl build is checked
  in CI.

## What a binding can and cannot do

A binding is one Rhai *expression* over `state`, `props` and `env`.

- **Sealed.** The engine registers an explicit package list. `sleep`, `exit`,
  `parse_json`, reflection, clocks and I/O are not there, and `print`, `debug`
  and `eval` are disabled, and so is `type_of` (`tests/sandbox.rs`). A binding
  cannot write through
  to the host's `state`, even if the host keeps it in shared cells: every
  variable is deep-copied into the evaluation scope (`tests/host_state.rs`).
- **Bounded.** Every cap ends in a diagnostic, never a panic or an abort. The
  caps are constants in `src/limits.rs` (`lazyrad_view::limits`), which is the
  **one place that states the numbers**, what they mean for a list of rows, the
  native stack they need and the memory the worst expression costs.
  `tests/limits_docs.rs` fails when a table there drifts from its constant or
  when another doc repeats one. Worth knowing here:
  - the array and map caps are enforced as stated, for a value a binding
    builds and for one it reads from `state` alike;
  - **a list of records is limited by its fields, not its rows**: Rhai counts
    one map entry per field;
  - a long text can be read (`bind text: state.doc`) but not used;
  - **every operator in a chain is a nesting level**, so a long `a + b + ...`
    or a long `else if` chain is split across bindings.
- **What it can see is bounded too.** An entry of `state` that does not fit the
  caps is replaced, in the copy a binding reads, by an opaque *hidden entry*.
  It **keeps its key**, so `contains`, `in`, `keys()`, `len()` and a comparison
  of two maps see the real shape; *using* the value (reading it, comparing it,
  a text conversion or `to_json`, a truthiness test, a method call) is a
  diagnostic that names the entry and why it is hidden. A map that does not fit
  is pruned inside, so `state.cfg.name` survives a very long `state.cfg.rows`.
  `type_of` is not available, because it would name the hidden type. A text
  over the cap is never hidden and can be bound; a method call on it is a
  diagnostic that names it. Keep counts and summaries in `state`, and long
  lists in a control that owns its rows.
- **Closures may read `state`.** `state.items.filter(|x| x > state.n)` works.
  A source that trips Rhai's shared-variable race is retried on private,
  read-only copies that closures hold by handle, so it costs a few copies per
  evaluation however long the list (`BindingStats::state_copies` counts them),
  about the same as the hoisted spelling
  `(|n| state.items.filter(|x| x > n)).call(state.n)`. A closure created
  *inside another closure* that mentions `state`, `props` or `env` reports
  "Data race detected" and has no workaround but restructuring.
- **A rejected edit is not pulled back.** The diff compares with what the
  session last applied, not with the widget, so a widget the user changed and
  the handler refused stays ahead of the state until the bound value next
  changes or the view is rebuilt.

## API notes

- `Node` implements `Drop` (to drop a deep chain without recursing), so fields
  cannot be moved out of a `Node` with a destructuring pattern or
  struct-update syntax. Use `std::mem::take(&mut node.children)` or clone.
- A `Tree` is at most 128 levels deep (`TreeError::TooDeep`).

## Deferred

- **The peak memory of one pathological `map`.** A `map` keeps every result its
  closure returns until it finishes, and Rhai size-checks a result only when
  the call returns, so a closure that builds a large value on each iteration
  holds one copy per iteration. The caps bound that, and the project chose them
  knowing the measured peak (`src/limits.rs`); it is a known cost, not an
  enforced bound. The cure, if it is ever wanted, is to shadow `map` and `zip`
  with versions registered through `Engine::register_raw_fn`, which can
  size-check each result as it is produced and end the call at the first one
  that is over a cap. It means owning those two methods' semantics (index
  forms, curried function pointers), which is why it is not done.

## Not here (yet)

No `.lfm` model (the tree is a small model of its own), no renderer, no
dependency on `xui-*`. A `.lfm` loader maps its document onto `Node`s, and a
renderer implements `Renderer`.

The design is in the modern-UI plan (`docs/modern-ui-plan.md`, PR #118): §3.1,
§3.4, §4.5 and §8 (lane B).
