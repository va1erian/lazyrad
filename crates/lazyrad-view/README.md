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
  and `eval` are disabled (`tests/sandbox.rs`). A binding cannot write through
  to the host's `state`, even if the host keeps it in shared cells: every
  variable is deep-copied into the evaluation scope (`tests/host_state.rs`).
- **Bounded.** Every cap ends in a diagnostic, never a panic or an abort. The
  numbers are in `src/limits.rs`; in practice:

  | What | Cap | Means |
  |---|---|---|
  | array elements in a value | **512** | the longest list, array literal and `map` |
  | map entries in a value | **512** | **one per field, nested maps included** |
  | a list of 5-field rows | 102 rows | `(512 - 1) / 5`: the entry holding the list costs one |
  | a list of 4-field rows | 127 rows | likewise; 10-field rows: 51; 20-field rows: 25 |
  | text in a value | 32 KiB | summed over texts inside arrays; a long text can be read but not used |
  | operations | 32 768 | a `map`, `filter` or `reduce` step costs 3 to 5 per element: 8 five-operation steps over 500 elements fit |
  | nested closure calls | 4 | |
  | nesting | 16 and 10 | **every operator in a chain is a level**: 16 terms in `a + b + ...` or `a && b && ...`, 12 `else if` branches, 10 chained method calls |

  The 512 is the same number whether a value is built or read from `state`
  (the engine enforces it, and the view of `state` is counted the way Rhai
  counts), and it is the visible budget the project has chosen.

  Native stack: the worst expressions found need about 350 KiB in a debug
  build (the suite runs them on 1 MiB).

  **Memory.** In the worst shape (a list of about five hundred one-field rows
  returned on each iteration of a `map` over a 511-element list) one
  evaluation transiently allocates about **154 MiB** in a debug build; 102
  five-field rows on 511 iterations is 52 MiB. For comparison the same shape
  was 545 MiB at 1024 and 217 MiB at 512 elements with 1024 entries;
  `src/limits.rs` has the table. It is a pathological expression, not a
  typical binding, and it ends in a diagnostic, but it is a real number.
- **What it can see is bounded too.** An entry of `state` that does not fit the
  caps is removed from the copy a binding reads (a map that does not fit is
  pruned inside, so `state.cfg.name` survives a 2000-row `state.cfg.rows`).
  *Any* read of a removed entry is a "Property not found" diagnostic that names
  the entry and its size; `keys()`, `len()` and `contains` on the map around it
  do not see it. A text over the cap is never removed and can be bound; a
  method call on it is a diagnostic that names it. Keep counts and summaries in
  `state`, and long lists in a control that owns its rows.
- **Closures may read `state`.** `state.items.filter(|x| x > state.n)` works.
  A source that trips Rhai's shared-variable race is retried on private copies;
  `(|n| state.items.filter(|x| x > n)).call(state.n)` keeps the fast path. A
  closure created *inside another closure* that mentions `state`, `props` or
  `env` reports "Data race detected" and has no workaround but restructuring.
- **A rejected edit is not pulled back.** The diff compares with what the
  session last applied, not with the widget, so a widget the user changed and
  the handler refused stays ahead of the state until the bound value next
  changes or the view is rebuilt.

## API notes

- `Node` implements `Drop` (to drop a deep chain without recursing), so fields
  cannot be moved out of a `Node` with a destructuring pattern or
  struct-update syntax. Use `std::mem::take(&mut node.children)` or clone.
- A `Tree` is at most 128 levels deep (`TreeError::TooDeep`).

## Not here (yet)

No `.lfm` model (the tree is a small model of its own), no renderer, no
dependency on `xui-*`. A `.lfm` loader maps its document onto `Node`s, and a
renderer implements `Renderer`.

The design is in the modern-UI plan (`docs/modern-ui-plan.md`, PR #118): §3.1,
§3.4, §4.5 and §8 (lane B).
