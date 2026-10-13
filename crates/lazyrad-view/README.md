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
  when the environment changes, bindings that read `state` run when it changes.
- **No platform:** only `rhai` and `thiserror`, and no file, thread, process,
  environment, network or clock access (`tests/no_platform_apis.rs` scans the
  sources and the manifest), so it builds for `wasm32-unknown-unknown` and
  static musl.

## What a binding can and cannot do

A binding is one Rhai *expression* over `state`, `props` and `env`.

- **Sealed.** The engine registers an explicit package list. `sleep`, `exit`,
  `parse_json`, reflection, clocks and I/O are not there, and `print`, `debug`
  and `eval` are disabled (`tests/sandbox.rs`).
- **Bounded.** Operations, call depth, expression depth and the size of any
  value it builds are limited, and it sees at most `VISIBLE_BUDGET` (1024)
  array elements and map entries per variable: a longer list is hidden from
  bindings and a binding that uses it gets a diagnostic naming it. Keep counts
  and summaries in `state`, and long lists in a control that owns its rows.
  The numbers, and why they are what they are, are in `src/bindings.rs`; one
  evaluation allocates at most about 64 MiB and needs about 350 KiB of native
  stack in a debug build.
- **Closures may read `state`.** `state.items.filter(|x| x > state.n)` works.
  A source that trips Rhai's shared-variable race is retried on private copies;
  `(|n| state.items.filter(|x| x > n)).call(state.n)` keeps the fast path. A
  closure created *inside another closure* that mentions `state`, `props` or
  `env` reports "Data race detected" and has no workaround but restructuring.
- **A rejected edit is not pulled back.** The diff compares with what the
  session last applied, not with the widget, so a widget the user changed and
  the handler refused stays ahead of the state until the bound value next
  changes or the view is rebuilt.

## Not here (yet)

No `.lfm` model (the tree is a small model of its own), no renderer, no
dependency on `xui-*`. A `.lfm` loader maps its document onto `Node`s, and a
renderer implements `Renderer`.

The design is in the modern-UI plan (`docs/modern-ui-plan.md`, PR #118): §3.1,
§3.4, §4.5 and §8 (lane B).
