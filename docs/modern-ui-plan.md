# LazyRAD — Modern UI and the web target

**Status: draft, revised 13 October 2026.** Proposes milestones **M8–M11** as a
companion to [PLAN.md](../PLAN.md). Nothing here is built yet. "Verified" means read from
source or from GitHub; "assumed" means a spike must confirm it (§8, Phase 0).

> **Revision note.** The first draft proposed a flat-TOML "format 2". That was wrong: xui
> already decided (and implemented, on unmerged branches) a **RON tree** as `.lfm` format 2,
> as part of its [productivity plan](https://github.com/va1erian/xui/blob/main/docs/plans/productivity.md).
> This draft builds on that work instead of competing with it, and makes landing it the
> first step (§8, Phase P).
>
> An independent review (a Sonnet agent, 13 October; claims re-checked against source)
> found that layout fields cannot be patched in place (F11), that several CSS mappings
> diverge (Appendix A), that `xui-script` lacks what LazyRAD uses (F6) and that Phase P
> was mis-sequenced. All are folded in; the log is §11.

LazyRAD today is a VB6-style RAD tool: you draw a *form* of absolutely positioned
controls and export a Windows `.exe`. This plan adds a second, modern way to build UI:
**responsive, state-driven layouts** designed in a tree designer, that build both a
**desktop app** and a **web SPA** from the same project.

| You asked for | The plan's answer |
|---|---|
| Modern, responsive UI paradigms | xui's layout containers (already designed: `Row`, `Column`, `Wrap`, `Grid`, `fill`, `align`, `max_*`) plus **state bindings**, **responsive overrides** and size classes (§4) |
| A visual designer like Flutter / modern Android | A tree designer: outline, device and size-class preview, layout-field inspector, drop indicators, "wrap in…" quick actions (§6) |
| Web **and** desktop | One form, one Rhai script, two renderers: xui (desktop, LazyOS) and the DOM (web) (§3) |
| Rich SPA | Real DOM: text, IME, accessibility and mobile keyboards come from the browser. Router, http, storage in M10 (§8) |
| Start small, then generalise | Phase P lands the foundation; M8 is a small core with a demo and a go/no-go gate (§8) |

---

## 1. Goals and non-goals

**Goals**

1. A UI model that **reflows**: it adapts to the window or viewport instead of being
   pinned to coordinates.
2. **One project, two targets.** The same `.lfm` and the same `.rhai` handlers run
   unchanged on desktop (xui) and in the browser. No per-target branches in user code.
3. A **visual designer** that edits the layout tree, previews it at phone, tablet and
   desktop widths, and writes plain, diff-friendly text.
4. **Export** a static web bundle next to the existing `.exe` / `.lzp` exports, with no
   toolchain on the user's machine (the prebuilt-stub model of PLAN.md §8).
5. Keep what exists: absolute-layout "VB6 mode", the debugger, hot reload and LazyOS.

**Non-goals (for M8–M10)**

- Native Android / iOS targets. A PWA covers phones; reconsider at M11.
- Pixel-identical rendering across targets. The contract is **layout semantics**
  (sizing, order, wrapping, breakpoints), not identical glyphs (§5.4).
- Running forms on the web through a canvas port of xui. Possible later (§3.3).
- A Material clone. The look is token-driven and restylable; it is not a spec port.

---

## 2. What the sources tell us (verified)

| # | Finding | Evidence | Consequence |
|---|---|---|---|
| F1 | **xui decided `.lfm` format 2 = RON, a nested tree**, with layouts (`Row`, `Column`, `Wrap`, `Grid(columns: [Auto, Fill(1)])`, `Absolute`), containers (`Panel`, `Group`, `Tabs`), 14 widgets, per-node layout fields (`fill`, `width`, `height`, `max_width`, `max_height`, `align`, `span`, `at`, `anchor`), **control arrays** (`Button(name: "digit", array: 10)`), and a schema generated from the same macro definition as the format. | [xui#275](https://github.com/va1erian/xui/pull/275), `docs/plans/productivity.md` "Decisions" | Most of what my first draft invented (flow containers, layout params, tree document) **exists**. This plan only adds what it lacks (§4). |
| F2 | **That work is not on `main`.** #274 (move `xui-form` into xui) merged to `main`. #275 (RON) merged into `claude/w3-forms` and #276 (CLI) into `claude/w3-forms-ron`, both stacked bases that were never retargeted. `main`'s tip (`93592fd`, the rev LazyRAD pins) has the TOML-era copy. The RON commit is `f4f2e91` and the CLI commit `d798cd4`. Files both sides changed since the squash: only `CHANGELOG.md` and `Cargo.toml`. | `gh pr view 274/275/276`; GitHub compare API (file-level, not a trial merge) | Landing it is cheap and unblocks everything. **Phase P1.** |
| F3 | xui ships an **`xui-form` CLI** on that branch: `check`, `render --report --overlay` (headless PNGs plus the layout report of every rectangle), `fmt --check`, `migrate`, `schema --json`. | [xui#276](https://github.com/va1erian/xui/pull/276), `docs/forms.md` | The tooling for conformance tests, linting and DOM-mapping coverage is already there. |
| F4 | xui's layout engine is flex-class: row, column, grid (`Auto`/`Fixed`/`Fill` tracks, spans), wrap, overlay, stack, absolute; `Sizing::{Auto, Fixed, Min, Fill(weight)}`, `Align`, `justify`, `gap`, `padding`, max sizes, **width-bounded measurement**, `Ui::layout_report()`. | xui@93592fd `xui-core/src/layout/tree/*`, `arrange/*` | Flow containers are a **mapping job** in the builder, not an engine project. |
| F5 | LazyRAD is still on the **flat TOML `FormDoc`**: about 80 reference lines in the `src` of five crates (designer 29, runtime 27, IDE 12, project 11, rhai 3), plus about 70 in tests and examples. It has **three kinds and one property the RON model lacks**: `Timer`, `Canvas`, `PictureBox`, and `Button.icon`, backed by about 3,000 lines of factories (`canvas.rs` 1,214, `picture/` 1,789, `xui_factories.rs` 1,133). | `grep` over `crates/*` | Adoption is real work, and the kinds must be modelled upstream first (P2). |
| F6 | The script bridge is renderer-abstract at its core: `Control` and `Form` talk to an `Rc<dyn FormHost>`, and `EngineHost` is neutral. But **`ScriptForm` is tied to `Ui` and `LiveForm`**, and **`xui-script` lacks nine items LazyRAD uses**: `build_deferred`, `reload`, `state`, `set_state` (hot reload), `StopHandle`, `is_host_handle` (debugger), `control_methods`, `to_arg_value`, `source_name`. It also documents that top-level variables are not visible in handlers, which LazyRAD's `36d2d21` reversed. | `xui-script/src/*` on `claude/w3-forms` (grep); `control.rs:28` | A DOM host can reuse the bridge, but only after a session split (P2c) and upstreaming LazyRAD's deltas (P2b). `xui-script` is not a drop-in. |
| F7 | The RON `Catalog` has **no `register` / `alias`**: kinds are enum variants of the format. | #275 "API differences" | A user-defined component needs an instance node, not a registered kind (§4.6). |
| F8 | `xui-core` has only `png` and `zune-jpeg` dependencies; its `std::thread::spawn` is test-only. `xui-form` depends on `xui-core`, `serde`, `ron`, `thiserror`, but has **no feature that drops its builder** (only `migrate`). `lazyrad-runtime` pulls `xui-canvas` unconditionally, its `Platform` is `Send + Sync`, and `now()`/`today()` go through the `time` crate. `xui-script` hard-codes Rhai's `debugging`. Rhai `=1.26.1` documents WASM support. | Cargo manifests; `platform.rs:231`; `stdlib.rs:430-436` | The crates *should* compile to `wasm32` once a model-only build exists (P2c). **Not compiled yet** (no wasm target is installed). Spike S1. |
| F9 | xui has **no web backend**, **no touch or gesture events**, **no IME on the canvas backends** (G3; the Win32 backend hosts a native `EDIT`, which has IME) and no accessibility bridge on the canvas backends. | xui@93592fd `backend/event.rs`, `docs/backends.md` | A canvas-in-browser target would inherit all four gaps. A DOM target gets them from the browser. |
| F10 | The theme is a small semantic palette plus decoration tokens (`corner_radius`, `glow`, …; bevel and glow are painted by `theme/look.rs`, not CSS-expressible as tokens). `Ui::bounds(id)` exposes laid-out rectangles. | xui `docs/theming.md`; `app/ui/node_ops.rs` | Colour tokens map to CSS custom properties; decoration needs per-target rendering. The designer can read live bounds. |
| F11 | **Layout is immutable once mounted.** A mount holds `tree: Group<usize>` (`arrange/mount.rs`); layout kinds have fields but **no runtime properties**; `width`/`height` are read-only outside `Absolute`; only `layout::Placement` is live. A `Fill` item has **zero natural main extent** (`layout/tree/item.rs:232`). | `mount.rs:31`; `schema/catalog.rs`; #275 body | A `responsive` patch to `direction`, `gap`, `fill`, `align` or `max_*` cannot be applied in place. See §3.4 (remount over shared widgets) and G27. |
| F12 | The CLI renders at `form.size` (no `--size`), and `layout_report` is prose naming widgets by type and text, not by control name, and has no JSON. | `xui-form-cli/src/render.rs`; `arrange/report.rs` | Conformance and designer badges need structured output (A5, G28). |

### How this relates to xui's productivity plan

This plan **extends** its workstreams; it does not redefine them.

| xui workstream | State (as read) | This plan |
|---|---|---|
| W1 layout engine v2 | The pieces I read on `main` (grid, wrap, absolute, scroll, `Align`, max sizes, `layout_report`) are in. I did not audit W1.1's measure/arrange protocol | Uses it as is. Adds only G21 gaps (§9) |
| W2 builders and `Handle` | In on `main` | Uses it |
| W3.1 / W3.6 move `xui-form`, `xui-script` into xui | **On `main`** (#274) | LazyRAD must adopt them (P4) |
| W3.2 RON format 2, W3.5 CLI | **Stranded** on `claude/w3-forms` | **P1** |
| W4 designer on layouts | Tree editing not started; hot reload (W4.4) already ships in LazyRAD's player | P3, P4 and §6 |
| W5.5 single xui pin | Not done | Prerequisite for P4 and R8 |
| W5 other / W6 | Out of scope here | W6.1 "verify headlessly first" is reused for the conformance suite |

---

## 3. Architecture

### 3.1 The thesis: the document is the product, renderers are plugins

```text
  .lfm form (RON)          .rhai handlers           environment (window size,
  layout tree, bind,       fn <ctl>_<event>()        size class, theme, text scale)
  responsive overrides            │                           │
         │                        ▼                           ▼
         └──────────►  lazyrad-view  (renderer-neutral, wasm-clean)
                       state · binding evaluation (Rhai) · responsive cascade
                       component expansion · diff  ──►  Patch { Create, Set, Move, Remove }
                                     │
                    ┌────────────────┼────────────────────────┐
                    ▼                ▼                        ▼
              XuiRenderer       DomRenderer             (later) CanvasRenderer,
              desktop, LazyOS   web (wasm + DOM)         BlitzRenderer
              xui_form::build   FormHost for the DOM
              + LiveForm::set   + CSS emitter
```

Two properties make this cheap:

- **The script bridge already abstracts the renderer** (F6): the web needs a new *host*,
  not a new language runtime.
- **The layout vocabulary maps onto CSS flexbox and grid closely** (Appendix A lists
  the places it does not, from source), so the DOM renderer is a small translator and
  the divergences are tested, not assumed.

### 3.2 Targets

| Target | Renderer | Layout engine | Scripts | Output |
|---|---|---|---|---|
| Windows / Linux / macOS | `XuiRenderer` (xui canvas or D2D) | xui `arrange` | Rhai, native | `.exe` via the existing stub + payload (PLAN.md §8) |
| LazyOS | `XuiRenderer` | xui `arrange` | Rhai, native | `.lzp` (existing) |
| **Web** | `DomRenderer` (new) | the browser (flexbox, grid, media queries) | Rhai compiled to wasm | a static folder: `index.html`, `lazyrad-web.wasm`, JS glue, `app.bundle`, assets |

### 3.3 Why DOM for the web (options considered)

| Option | For | Against | Verdict |
|---|---|---|---|
| **A. xui in the browser** (`wasm32` + a `<canvas>` backend, like Flutter's CanvasKit) | 100% reuse, pixel-identical, no parity problem | Inherits F9: no IME, touch scrolling, ARIA, selection or autofill; multi-MB wasm plus fonts; a new `Backend` to write | Not the default. **A later fallback** for the `Canvas` control and exotic widgets (M11) |
| **B. DOM renderer** | What "rich SPA" means: accessibility, IME, mobile keyboards, native scrolling, selection, zoom, prerender; small payload (no widgets or rasteriser in wasm); responsive via real media queries | A second renderer, so parity must be tested; each component needs a DOM mapping | **Recommended** |
| **C. Blitz everywhere** (HTML/CSS as the IR; Blitz on desktop) | One layout engine, zero drift | Discards xui's widgets and the designer work; Blitz is young and large against "tiny single exe" | Rejected for now. The IR keeps the door open for a `BlitzRenderer` |

**Revisit trigger for B:** if the conformance suite (§5.4) shows more than a handful of
semantics that cannot be expressed identically, or the web needs widgets (data grid, rich
text) feasible only as xui custom widgets, promote option A for those components.

### 3.4 The turn model

LazyRAD's model stays: *one event, one handler run, then repaint*. Bindings add a cheap
step after the handler:

```text
 DOM event / xui event
        │  Msg::Event { form, control, event, args }
        ▼
 run fn <control>_<event>(args…) on the form's AST        (debugger works as today)
        ▼
 re-evaluate bindings against `state`, `props`, `env`     (never stops the debugger)
        ▼
 diff against the last applied values  ──►  [Patch::Set …]
        ▼
 renderer applies patches                                  (skip a Set equal to the live
                                                            value: keeps caret and IME)
```

This is the Elm / Flutter `setState` shape without a virtual-DOM engine: the document is
a static template, so the only structural changes (M9) are conditionals, repeaters and
components, and rebuilding a subtree is an acceptable first implementation (hot reload
already rebuilds a form in place, `lazyrad-runtime/src/reload.rs`).

**Layout fields are not live (F11).** A `Set` on a widget property (`text`, `visible`,
`enabled`, …) is applied in place. A `Set` on a *layout* field (`direction`, `gap`,
`fill`, `align`, `max_*`, `padding`) changes the mounted `Group` tree, which is
immutable. The xui renderer therefore applies layout patches by **re-mounting the
enclosing level over the same shared widgets**, the technique LazyRAD's `Placement`
already uses for absolute geometry (`placement.rs`: widgets are held through `Rc`, so
they are not recreated). Edit text, caret, selection and a scroll view's offset live in
the widgets and must survive; a spike (S3) proves it before M8 depends on it, and G27
tracks live layout cells upstream. Layout patches are only produced at size-class
changes, so the cost is paid once per breakpoint crossing, not per event. The DOM
renderer is unaffected: CSS applies them natively.

---

## 4. The form document: xui's format 2, plus four additions

### 4.1 What exists (on `claude/w3-forms`)

```ron
Form(title: "Hello", size: (360, 160), root: Column(gap: 8, padding: 16, children: [
    Row(gap: 8, children: [
        Edit(name: "name_edit", placeholder: "Your name", fill: 1),
        Button(name: "greet_button", text: "Greet"),
    ]),
    Label(name: "result_label"),
]))
```

A nested tree, `Kind(field: value, …)`, defaults omitted, byte-stable canonical output
(`PrettyConfig`), errors with line, column and "did you mean". Handlers keep the naming
convention (`fn greet_button_click()`). **LazyRAD adopts this as is.** Names follow xui:
`Group` (not `GroupBox`), `placeholder` (not `cue`), PascalCase enum values.

### 4.2 What this plan adds

All four are additive and keep the one-definition rule (format and schema come from the
same macros). Syntax below is a **sketch**; spike R1 (§8) confirms how the macros carry it.

```ron
Form(name: "profile_form", title: "Profile", size: (720, 560), root: Column(padding: 16, gap: 12, children: [
    Label(name: "title_label", text: "Profile"),
    Edit(name: "name_edit", placeholder: "Your name"),
    Label(name: "greeting_label", bind: {
        "text":    "`Hello, ${state.name}!`",
        "visible": "state.name != \"\"",
    }),
    Flex(name: "cards", direction: Vertical, gap: 12,
         responsive: { Medium: (direction: Horizontal) },    // stacks on a phone, side by side from 600 dp
         children: [
        Panel(name: "counter_card", card: true, responsive: { Medium: (fill: 1) }, content: Column(padding: 12, gap: 8, children: [
            Label(name: "count_label", bind: { "text": "`Clicked ${state.count} times`" }),
            Button(name: "add_button", text: "Add one", align: Start),
        ])),
    ]),
]))
```

```rhai
fn form_load()             { state.name = ""; state.count = 0; }
fn name_edit_change(text)  { state.name = text; }
fn add_button_click()      { state.count += 1; }
```

| # | Addition | What it is | Why this shape |
|---|---|---|---|
| 1 | **`bind`** (a common node field) | `bind: { "<property>": "<Rhai expression>" }`. One-way, state → property | A **side table**, so no widget field changes type and `deny_unknown_fields` and the typed schema stay intact. The schema validates that each key is a runtime-writable property of that kind |
| 2 | **`responsive`** (a common node field) | `{ <SizeClass>: (<partial fields>) }`, mobile-first cascade | Same reason. A patch type with every field optional can be generated by the macros that already list each node's fields |
| 3 | **New kinds** | `Flex` (a `Stack` whose `direction` is a field, so it can be responsive), `Scroll`, `Overlay`, `Spacer` | `Row`/`Column` are separate variants, so a responsive axis needs a `direction` field. The others are xui layouts the model has not exposed yet |
| 4 | **`margin`** | A layout field: `margin: 8` | Missing from the model and from `arrange` (G21). A one-child wrapper layout until then. `min_width`/`min_height` are **deferred**: an entry has one sizing slot and `Min` is in the rigid class with `Fixed`, so `fill` plus `min_width` cannot be expressed (§4.4) |
| 5 | **Accessible names** | `label_for: "name_edit"` on `Label`; `accessible_name` and `description` on any widget | The DOM needs `<label for>` and `aria-*`; the web demo's accessibility claim is untestable without them |

Not additions, already there: `fill`, `align`, `align_items`, `justify`, `gap`, `padding`,
`max_*`, `span`, `Panel(card: true)` (my earlier `Card`), control arrays.

**Mechanics R1 must settle** (from the model's source):

- The canonical writer uses `struct_names(true)`, so a patch prints as
  `FlexPatch(direction: Horizontal)`, not the bare `( … )` of the sketch. The sketch is
  valid RON either way.
- Clearing an `Option` field in a patch needs `Some(None)`. Pick a reset idiom (an
  explicit `Reset` marker is clearer than the nested option).
- New common fields must also be added to the schema's `LAYOUT_FIELDS`, or `check` and
  `schema --json` will not list them.
- **Layout nodes have no `name` today** (`Row`, `Column`, `Wrap`, `Grid`, `Absolute`). A
  `bind` or `responsive` on one needs either an optional `name` or a tree-path address.
  `Flex` gets a `name`; the rest follow whichever R1 chooses.
- `Diagnostic.node` carries a widget name only, and a bad `bind` key is a map key, so a
  load error has no line, column or "did you mean". Validation of bind keys therefore
  reports through `Form::validate`, with the key and node.

### 4.3 Kinds

| Group | Kinds | M8 | Maps to (xui) | Maps to (DOM) |
|---|---|---|---|---|
| Flow | `Row`, `Column`, `Wrap`, `Grid` (**exist**) | ✓ | `row()` / `column()` / `wrap()` / `grid()` | `display:flex` / `flex-wrap` / `grid` |
| Flow | `Flex` (**new**) | ✓ | `row()` or `column()` by `direction` | `display:flex` |
| Layering | `Overlay` (**new**) | ✓ | `stack()` (children stretch) or `overlay()` (centred at natural size): R1 picks one meaning | stacked grid cell |
| Scroll | `Scroll` (**new**) | ✓ | `scroll()` (vertical only today) | `overflow:auto` |
| Surface | `Panel(card: true)`, `Group`, `Separator` (**exist**), `Spacer` (**new**) | ✓ | `Panel` + `look::card` | `div`, `div` + title, `hr` |
| Content | `Label`, `Button`, `Edit`, `CheckBox` (**exist**) | ✓ | existing widgets | `span`/`p`, `button`, `input` |
| Content | `ToggleButton`, `Slider`, `ProgressBar`, `ComboBox`, `RadioGroup`, `Hyperlink`, `NumberField`, `MultilineEdit` (**exist**) | M9 | existing | native elements |
| Content | `PictureBox` → `Image`, `Icon`, `Tabs` (**exist / port**) | M9 | `PictureBox`, Lucide | `img`, inline SVG |
| Classic | `Absolute` (**exists**) | ✓ (interop) | `absolute()` + anchors | `position:absolute` |
| Data | `ListView`, tree, table | M11 | existing / new | virtualized DOM |

Each kind gets a per-target support flag in the catalog. A project declares its targets,
and the designer and `xui-form check` warn when a form uses a kind the web target lacks,
the role Android's `minSdk` lint plays. `xui-form schema --json` drives a test that every
kind has a DOM mapping.

### 4.4 Layout fields (they depend on the parent)

As in Android's `LayoutParams`, the property grid shows different sizing fields depending
on the **parent's** layout; `Form::validate` already warns on a layout field the parent
ignores (`span` outside a `Grid`).

| Field | Under | Meaning | xui |
|---|---|---|---|
| `width`, `height` | flow parent | requested size; absent = content | `Item::size` / `Sizing::Fixed` |
| `fill` | `Row`, `Column`, `Flex`, `Grid` row | share of leftover main-axis space; absent = content | `Sizing::Fill(w)` |
| `max_width`, `max_height` | flow parent | upper bound | `Item::max_width/height` |
| `align` | flow, `Grid`, `Overlay` | `Start` \| `Center` \| `End` \| `Stretch` across the parent | `Align` |
| `span` | `Grid` | columns covered | `Item::span` |
| `at`, `anchor` | `Absolute` | free position, **unchanged** | `absolute()` |
| `min_width`, `min_height` *(deferred, G21)* | flow parent | lower bound | `Sizing::Min` shrinks like `Fixed` and shares the entry's single sizing slot, so it cannot combine with `fill` |
| `margin` *(new)* | flow parent | space outside the node | wrapper with padding (G21) |

**`fill` needs a definite main extent.** A `Fill` item has no natural main extent
(F11), so a `fill` child of a `Column` or vertical `Flex` whose own height is `auto`
collapses to zero, where CSS would show the content height (`min-height: auto`). The
sketch in §4.2 therefore puts `fill` in a `Medium` patch, where the parent's axis is
horizontal. `Form::validate` gains a warning for `fill` under a vertical parent with an
indefinite height, and the conformance fixtures include the case. The web renderer emits
`min-height: 0`/`min-width: 0` only where the xui semantics are "shrink to zero".

Container fields: `gap`, `padding`, `align_items` (default for children, across),
`justify` (along; space-between needs G21), `direction` (`Flex`), `columns` (`Grid`),
`axis` (`Scroll`).

### 4.5 State, bindings and responsive values

- **State** is `form.state`, the object map that already exists. In scripts `state` is
  a **shared handle type** (like `form`), not a copy: today `form.state` returns a clone
  (`xui-script/src/control.rs:237`), so a pushed copy would silently lose
  `state.count += 1`. Reads must not clone the whole map each time.
- **Reserved names:** `state`, `props` and `env`. LazyRAD already has shared top-level
  `let`s (PLAN.md §4.1); bindings do **not** see them, only the three reserved names, so
  an expression's inputs are explicit.
- **A binding** is evaluated by the same Rhai engine on both targets, in a
  **side-effect-free engine** (no assignment, no stdlib I/O). It sees `state`, `props`
  (M9) and `env` only, **not control properties**: reading controls would make evaluation
  order observable and allow cycles. Evaluated at build, after every event turn, and on
  `env` change (only the bindings that read `env`); not per frame.
- **Tooling:** the debugger's step-into skips `.lfm` expressions; renaming a control
  rewrites the `bind` and `label_for` strings that name it, as `rename_handlers` does for
  handlers.
- **Ownership:** a bound property belongs to its binding. A handler that writes it is a
  lint warning. A failing expression keeps the last good value and reports in the Output
  pane; it never crashes the app. Expressions are compile-checked at load.
- **Two-way input** stays explicit (`fn name_edit_change(text) { state.name = text; }`):
  transparent in the debugger, no magic.
- `env` = `{ width, height, size_class, dark, text_scale }`, read-only. There is no
  `env.platform`: it would invite the `if web` branches the M8 gate forbids.

Size classes follow Material 3 by width, cascading **mobile-first** like CSS `min-width`
queries:

| Class | Width | Typical |
|---|---|---|
| `Compact` | < 600 dp | phone portrait |
| `Medium` | 600 – 839 dp | tablet portrait, foldable |
| `Expanded` | 840 – 1199 dp | tablet landscape, small desktop |
| `Large`, `XLarge` | ≥ 1200 / ≥ 1600 dp | desktop (accepted now, cascade down) |

On the web these compile to real `@media` rules, so layout responds before any script
runs. On xui the runtime recomputes on `Resize` and applies `Set` patches. A dip equals a
CSS px at 96 dpi, so the numbers match. Container queries are M11.

### 4.6 Components, lists, conditionals (M9)

Because kinds are enum variants (F7), a user component is an **instance node**, not a
registered kind:

```ron
Form(name: "stat_card", props: [Prop(name: "title", kind: Text, default: "Stat")],
     root: Panel(card: true, content: Label(bind: { "text": "props.title" })))

Component(of: "stat_card", name: "users_stat", props: { "title": "Users" })
```

Slots are children of the instance. `Repeat(items: "state.todos", template: …)` (keyed,
with `item` and `index` in scope) and `Show(when: "…", content: …)` cover lists and
optional content. They sit beside, and do not replace, control arrays, which stay the
static "ten buttons" tool.

### 4.7 Compatibility

- **One file type.** A view is a `.lfm`; there is no `.lvw`. A form with `props` is
  usable as a component. `xui-form migrate` converts every existing LazyRAD form once.
- `.lrp` is unchanged except for project-level `targets = ["desktop", "web"]` and a `[web]`
  table (title, favicon, base path).
- Scripts keep `form` and `form_load`. `state` is an alias for `form.state`.
- A stretch tool, **Convert to flow layout** (W4.2 in xui's plan), infers rows and
  columns from the rectangles of an `Absolute` form by alignment clustering.

---

## 5. Runtime, crates and packaging

### 5.1 Crate changes

| Where | Change |
|---|---|
| **xui** `xui-form` | The model additions of §4.2; tree ops (§6.2); **a `build` feature gate** (none exists: only `migrate`) so the web crate links the model without widgets. Developed through xui's established process, then pinned by LazyRAD |
| **xui** `xui-script` | LazyRAD's deltas (P2b: `build_deferred`, `reload`, `state`/`set_state`, `StopHandle`, `is_host_handle`, `control_methods`, `to_arg_value`, `source_name`, top-level-variable semantics); a **renderer-neutral session split** (P2c): engine, AST, function table, run/load/close and callbacks apart from `Ui`/`LiveForm`; Rhai `debugging` behind a feature |
| `lazyrad-view` (new, LazyRAD) | Renderer-neutral and `wasm32`-clean: `Environment`/size classes, responsive cascade, **the single home of the binding pass**, `ViewTree`, `Patch`, `Renderer` trait, golden patch traces. Depends on the `xui-form` model (with the build gate off) and `rhai`. Upstream to xui once stable |
| `lazyrad-runtime` | Drops its `xui-form`/`xui-rhai` copies for xui's (P4); calls `lazyrad-view`'s binding pass; feeds `Resize` into `env`. Gains a `wasm32` CI job (it already has the `desktop` split for musl), a `canvas-control` feature so the web build drops `xui-canvas`, and a web-safe `Platform` (today `Send + Sync`) |
| `lazyrad-web` (new, `cdylib`) | wasm-bindgen entry, bundle loader, `DomRenderer`, `DomHost: FormHost`, the web `Platform`, CSS emitter |
| `lazyrad-packager` | `web` export: copies the prebuilt stub, writes `index.html`, the bundle and assets. Same model as the `.exe` |
| `lazyrad-project` | `.lrp` gains `targets = ["desktop", "web"]` and a `[web]` table (title, favicon, base path), validated and round-tripped |
| `lazyrad-designer` | Moves onto the tree (P3, P4), then the flow-mode features of §6 |
| `lazyrad-ide` | `targets`, the preview bar, *Run in browser*, *Make Web App* |

### 5.2 The web runtime

- **Same engine, new host.** `lazyrad-web` builds the Rhai engine with the web `Platform`,
  installs the `on_var` resolver over `DomHost` (a `FormHost` over DOM elements keyed by
  node name), compiles the shipped `.rhai`, then runs the turn model of §3.4.
- **Events** use delegation on the form root; each DOM event becomes the same
  `Msg::Event { control, event, args }` the player uses.
- **Stdlib on web:** `msg_box` is an in-page dialog (non-blocking, as on canvas today);
  `now`/`today` go through the `time` crate, which **panics on `wasm32-unknown-unknown`**,
  so the web `Platform` supplies the clock (`Date`) and `random` seeds from
  `crypto.getRandomValues`; `file_*` goes through the web `FsPolicy` (in-memory, then
  IndexedDB); `http` and `json` are M10.
- **Window mapping.** `title` → `document.title`; `size` is the design size (fixtures and
  the designer's default viewport) while the root fills the viewport; `resizable` is
  ignored. `Hyperlink` URLs are limited to `http`, `https` and `mailto`.
- **Budgets** (targets to measure in S1, not facts): wasm ≤ 1.5 MB gzipped, interactive
  in ≤ 1 s on a mid laptop, no widget code or rasteriser in the module.

### 5.3 Export and dev loop

```text
Make Web App  ─►  dist/
                  ├─ index.html          shell: root element, CSS for the default state
                  ├─ lazyrad-web.js      wasm-bindgen glue (prebuilt)
                  ├─ lazyrad-web.wasm    the stub: Rhai + lazyrad-view + DomRenderer
                  ├─ app.bundle          the project payload (.lfm, .rhai, assets), checksummed
                  └─ assets/…
```

- **Run in browser** starts a tiny static server with a reload endpoint (the analogue of
  the player's `--watch`) and opens the default browser. State survives reloads through
  `form_reload(old_state)`, as today.
- **Prerender (M10):** the first frame is deterministic from the form and default state,
  so export can emit it as HTML and CSS for instant first paint and SEO, with wasm
  hydrating it.
- **Debugging on the web** is out of M8. The protocol is plain JSON, so a WebSocket
  transport through the dev server is the natural later step.

### 5.4 Parity, tested rather than hoped for

- **Semantic subset.** The layout features both engines honour are written down
  (Appendix A) and nothing else is used.
- **Conformance fixtures.** Two kinds, at several viewport widths: fixed-size boxes (no
  font drift, ±1 px) and **text-bearing fixtures with the bundled font** (wrapping text in
  a `Row`, a `fill` card, a `Wrap`; asserted by structure and rough extent, since text
  is where the engines differ, Appendix A). The expected rectangles come from
  `xui-form render --report`, which needs `--size` and a structured report first (A5,
  G28), and a LazyRAD-side render of the *resolved* form, because the CLI cannot evaluate
  `responsive`. A headless-Chromium and WebKit job compares the DOM. Opt-in in CI, one
  command locally.
- **Text** may differ by font metrics. Both targets bundle the same font, and the rule
  for authors is "design flexibly": that is what responsive means.
- **Logic parity is free:** one crate, one engine, golden patch traces asserted from the
  same event script on both runtimes.

---

## 6. The visual designer

### 6.1 Experience

```text
┌ Toolbox ──┬──────────── Preview ───────────────────┬ Outline ───────────┐
│ Layout    │ [Phone] [Tablet] [Desktop] [<-> 412 px]│ ▾ root (Column)    │
│  Row      │ light/dark   text 1.0x   class: compact │   title_label      │
│  Column   │ ┌──────────────────────────┐           │   name_edit        │
│  Flex     │ │ Profile                  │           │   greeting_label   │
│  Scroll   │ │ [Your name           ]   │           │ ▾ cards (Flex)     │
│ Controls  │ │ ┌──────────────────────┐ │           │   ▸ counter_card   │
│  Label    │ │ │ Clicked 0 times      │ │           ├ Properties ────────┤
│  Button   │ │ │ [Add one]            │ │           │ Layout             │
│  Edit     │ │ └──────────────────────┘ │           │  width   [Auto ▾]  │
│ …         │ └──────────────────────────┘           │  fill    [1     ]  │
│           │ ! cards: clipped by 24 px at 320 px    │  margin  [0     ]  │
└───────────┴────────────────────────────────────────┴────────────────────┘
```

| Feature | What it does | Inspired by |
|---|---|---|
| **Outline tree** | Every node; drag to reorder and reparent, show/hide, lock | Flutter widget tree, Android Studio component tree |
| **Viewport bar** | Phone / tablet / desktop presets plus a free-resize handle; the live layout reflows as you drag; a badge shows the size class | Android Studio device frames, devtools |
| **Layout inspector** | Selection overlay draws margin, padding and gap; the parent's layout drives the property grid | Android layout inspector, Chrome devtools |
| **Sizing control** | Per axis: `Auto` / `Fill(weight)` / `Fixed(n)`, plus min and max | Android Studio's constraint widget |
| **Drop indicators** | Dragging from the toolbox or canvas shows an insertion line between siblings or a highlight into a container | Compose and Flutter designers, FlutterFlow |
| **Quick actions** | Wrap in Row / Column / Panel / Scroll, unwrap, move up/down, duplicate, extract as component (M9) | Flutter's "Wrap with widget" and "Extract widget" |
| **Per-breakpoint editing** | A context switch (`all` / `Medium+` / `Expanded+`) makes edits write `responsive` entries; a dot marks overridden properties | Webflow, Figma variants |
| **Binding editor** | An expression field with completion for `state.*` and `env.*`; errors inline | Compose previews, Android data binding |
| **Layout warnings** | `layout_report()` warnings (clipping, zero size, truncated text, overlap) become badges on the node | Flutter's overflow stripes |
| **Theme and text scale** | Light/dark toggle and 1.0–2.0× text scale in the preview | Android Studio preview toggles |

### 6.2 How it is built

- **Phases P3 and P4 first: the tree API, then the designer onto it.** xui's PR states "the designer's flat
  editing helpers (`insert`/`remove`/`rename`/`reparent`) have no equivalent yet; tree
  editing is W4". So: a path-addressed tree API in `xui-form` (`NodePath`; `insert_at`,
  `move_to`, `remove`, `rename`, `wrap`, `unwrap`, each returning its inverse for the
  existing `History`), then `Surface` re-based on it with today's behaviour intact.
  `Absolute` containers keep the VB6 drag-and-resize interactions (xui W4.2).
- **Geometry comes from the layout, not the document.** The preview is built with the
  real builder in design mode; hit-testing and overlays read `Ui::bounds(id)` per node at
  paint time, so they always agree with the engine.
- **Drop targets** are computed from sibling rectangles along the container's main axis
  (before/after the nearest sibling, or "into" an empty container).
- **Reuse:** `PropertyGrid` (it already follows the `Catalog`; layout fields become a
  category keyed by the parent), `Toolbox`, `History`, viewport fitting and scrolling
  (recent designer work), hot reload for run-from-IDE.
- **Sequence:** outline reorder and toolbox click-to-insert first (cheap, keyboard
  friendly, testable); free drag-and-drop on the canvas second.
- **Text view:** the same file opens in the code editor. Saves are canonical (`fmt`), so
  switching never reformats it; `xui-form fmt --check` can gate CI. Comments are not kept
  across a designer save (an xui decision).

### 6.3 Deliberately not in the first cut

Free-form constraints (ConstraintLayout-style relational anchors), animation timelines
and a visual state-machine editor. The first two do not map to CSS cleanly; the third is
a separate product.

---

## 7. Component library tiers

| Tier | Contents | Milestone | Notes |
|---|---|---|---|
| 0 Core | `Row`, `Column`, `Flex`, `Overlay`, `Scroll`, `Panel`, `Spacer`, `Separator`, `Label`, `Button`, `Edit`, `CheckBox` | M8 | Both renderers; enough for forms and dashboards |
| 1 Standard | `Wrap`, `Grid`, `ToggleButton`, `Slider`, `ProgressBar`, `ComboBox`, `RadioGroup`, `Image`, `Icon`; button variants (filled, tonal, outlined, text) | M9 | Painted xui widgets on `look::*` tokens; CSS counterparts |
| 2 Adaptive | `Scaffold` (app bar, body, drawer), `NavigationBar` (Compact) ↔ `NavigationRail` (Medium) ↔ `NavigationDrawer` (Expanded), list-detail pane, dialog, sheet, snackbar, `Tabs` | M9–M10 | The adaptive trio is the best demonstration of "responsive" |
| 3 Data | virtualized list, data table, tree, validation helpers | M10–M11 | `ListView`/`TreeView` exist on xui; the web needs virtualization |
| 4 Rich | rich-text editor, charts, code editor, `Canvas` | M11 | May be xui-only on desktop at first; `Canvas` maps to `<canvas>` |

---

## 8. Milestones

Sizes are rough, for one developer, as in PLAN.md §11. Each milestone ends in a demo.

### Phase P — Land and adopt xui's format 2 (≈ 8–10 weeks with two agents in tandem)

Mostly xui's own W3/W4, plus the deltas LazyRAD needs. It is worth doing on its own: it
cuts LazyRAD's calculator form from 159 lines to about 40. **M8's model and view-core
lanes (A, B) need only P1–P2c; the desktop and designer lanes (C, E) need P4.**

| # | Work | Repo | Size | After |
|---|---|---|---|---|
| P1 | **Land #275 and #276 on `main`.** Branch from `main`, take `f4f2e91` (RON) and `d798cd4` (CLI) (the squash `ede6e5a` already carries the move), resolve `CHANGELOG.md` and `Cargo.toml`, re-run CI. Leave the stale stacked branches for the owner to delete | xui | days | — |
| P2 | **Model the kinds LazyRAD has and the RON model lacks:** `Timer`, `Canvas`, `PictureBox`, `Button.icon`, with their factories. About 3,000 lines move onto xui's factory API. Upstream, because the model is a closed enum (D3) | xui | 2–3 wk | P1 |
| P2b | **Upstream LazyRAD's `xui-rhai` deltas** into `xui-script` (the nine names in F6) and settle top-level-variable semantics (D8) | xui | ≈ 1 wk | P1 |
| P2c | **Wasm factoring:** a `build` feature gate in `xui-form`; a renderer-neutral `ScriptForm` session split; Rhai `debugging` behind a feature | xui | 1–2 wk | P1 |
| P3 | **Tree API:** path-addressed `NodePath`; `insert_at`, `move_to`, `remove`, `rename`, `wrap`, `unwrap`, each returning its inverse (§6.2) | xui | 1–2 wk | P1 |
| P4 | **Adopt.** One xui rev (W5.5); delete LazyRAD's `xui-form`/`xui-rhai` copies; port the `FormDoc` sites (designer onto the P3 tree API, runtime, IDE, project); `xui-form migrate` the samples and templates; keep hot reload, rename-handlers and the debugger working | LazyRAD | 3–4 wk | P2, P2b, P3 |
| P5 | **Docs:** PLAN.md §1 (TOML), §1.1 (`GroupBox`), §2, README, samples, tutorial | LazyRAD | days | P4 |

**Gate:** the four samples and the IDE templates run, debug and export from format-2
forms (through the kinds P2 added); the designer passes today's designer tests re-based
on the tree; hot reload and the debugger work on format-2 forms; the musl CI job is green.

### Phase 0 — Spikes (≈ 1–2 weeks; S1 and S2 run beside P1, S3 and R1 need P1)

Cheap experiments that can change the plan. Each has a pass condition.

| # | Spike | Pass condition |
|---|---|---|
| S1 | `cargo check --target wasm32-unknown-unknown` for `xui-core`, `xui-form` (a scratch `cfg` gate of its builder stands in for P2c), `rhai =1.26.1`, plus a "hello wasm" running a Rhai script and measuring size | Compiles; gzipped wasm within the §5.2 budget. Expected runtime panics (they compile, then panic on `wasm32-unknown-unknown`): `SystemTime::now` (`stdlib.rs:384`), `time::OffsetDateTime::now_*` (`stdlib.rs:430-436`), `Instant::now` in the `Canvas` control |
| S2 | Hand-write the §4.2 form as HTML and CSS; compare to `xui-form render --report` at 360, 700 and 1000 px, with **text-bearing** fixtures (wrapping text in a `Row`, a `fill` card, a `Wrap`) and the Appendix A divergences | The divergences are confirmed or refuted; no semantic that needs a hack; fixed-size rectangles within ±1 px |
| S3 | **Remount over shared widgets** (§3.4): on a built RON form, change a `Row` to a `Column` and a `gap` through a level re-mount that reuses the widgets, while an `Edit` has focus, caret and text and a scroll view is scrolled | Text, caret, selection, focus and scroll offset survive; no widget is recreated; cost measured on the calculator and a 200-node form. If it fails, G27 (live layout cells) moves into M8 |
| R1 | Prototype `bind` and `responsive` as common fields in the `node!`/`widget!` macros (including the generated all-optional patch type and the mechanics in §4.2) | Round-trips byte-stable; `Form::validate` reports a bad key; the catalog JSON lists both; a layout node can be addressed |

### M8 — Core: the small proof (≈ 8–10 weeks wall-clock, two agents in tandem)

Scope: tier 0, bindings, responsive overrides, the xui and DOM renderers, web export, and
the flow-designer essentials. **Out of M8:** components, lists, conditionals, navigation,
the theming editor, animation, http. Sizes assume at most **two agents at once** (the
dispatch workflow's cap), so the lanes below are ordered by what blocks what, not run
all at once: A and B first (they need only P1–P2c), then C, D and E.

| Lane | Work packages (issue-sized) |
|---|---|
| **A Model** (xui) | A1 `bind` field, validation and expression compile check. A2 `responsive` patches and size-class cascade in the schema. A3 `Flex`, `Scroll`, `Overlay`, `Spacer` kinds with factories. A4 `margin` (wrapper); `min_*` deferred (G21). A5 CLI: `render --size`, `--report json` (node path, control name, warnings), `label_for` and `accessible_name` |
| **B View core** (`lazyrad-view`) | B1 `Environment` and cascade. B2 binding evaluator and `Patch` diff, skip-if-equal. B3 `Renderer` trait, `ViewTree`, golden patch traces. **B3 and A1 define the contracts the other lanes need first** |
| **C Desktop** | C1 builder: new kinds and layout fields → `arrange`, layout patches by re-mount (S3). C2 runtime: calls B2 after each turn, `Resize` → `env`, shared `state` handle. C3 lint: bound-property writes, unknown state keys, binding compile errors (`check_project`) |
| **D Web** | D1 `lazyrad-web` skeleton, bundle loader, web `Platform`. D2 `DomRenderer` and `DomHost`, event delegation. D3 CSS emitter with media queries, driven by the catalog. D4 dev server and *Run in browser*. D5 *Make Web App* and the stub CI job. D6 `.lrp` `targets` and `[web]` (`lazyrad-project`) |
| **E Designer** | E1 viewport bar and size-class badge. E2 drop indicators and outline drag. E3 property grid: layout fields per parent, sizing control, responsive indicator, binding field. E4 layout-warning badges (from A5's structured report). E5 quick actions (wrap, unwrap, move) |
| **F Quality** | F1 `examples/modern_hello` (the demo below). F2 conformance fixtures (fixed-size and text-bearing) and the Chromium and WebKit job. F3 web smoke test (load the bundle, click, assert) plus an axe-core audit of the demo |

**The demo that closes M8**

1. `lazyrad-player examples/modern_hello` opens a native window at 720 × 560. Dragging it
   narrower than 600 dp flips the two cards from a row to a column.
2. In the IDE: open `profile_form.lfm`, drag a `Button` from the toolbox into the `cards`
   row, set `fill: 1`, switch the viewport to *Phone*, press F5.
3. *Make Web App*, serve `dist/`, open it in a browser: the same behaviour, the same
   `.rhai` file byte for byte, text is selectable, Tab moves focus in tree order, every
   input has an accessible name (`label_for`), and axe-core reports no critical
   violations.

**Gate to M9 (go / no-go)**

- The demo works on Windows and in Chromium, Firefox and WebKit.
- S3 passed: layout patches keep text, caret, focus and scroll offset.
- Conformance fixtures pass; known deviations are listed, not hidden.
- Web budgets (§5.2) are met, or the miss is understood and costed.
- Classic (absolute) forms still open, run and export unchanged; musl CI green.
- No `if web` anywhere in a sample's script.

### M9 — Compose: components, lists, theming (≈ 5–6 weeks)

Tier 1 components and the adaptive scaffolds; `props`, `Component`, `Repeat`, `Show`;
design tokens (colour roles, type scale, shape, spacing, density) with a theme editor,
mapped to xui's `Theme` and CSS custom properties; extract-component, per-breakpoint
editing, sample data for previews; xui's *Convert to flow layout* assistant. Evaluate
`xui-blitz` as an in-IDE *web preview* tab and as a layout oracle.

### M10 — Rich SPA (≈ 5 weeks)

Router (`form.navigate`, params, deep links, browser history on web, an in-app stack on
desktop); `http` (callback-based, since Rhai is synchronous) and `json`; persistent
`settings`; validation helpers; i18n; an accessibility audit in the designer (labels,
focus order, `Color::contrast_ratio`); PWA manifest and offline cache; prerendered first
paint; per-route code splitting; performance budgets in CI; a security pass (URL schemes
on `Hyperlink` and `http`, the web `FsPolicy`, no `eval`-like surfaces in bindings).

### M11 — Breadth (open-ended)

Data table and virtualized lists; dialogs, sheets, snackbars; animation and transitions;
touch and gestures (G19); container queries; `Canvas` on web via `<canvas>`; the
xui-in-the-browser fallback (option A); a debug transport for web; evaluate a
`BlitzRenderer` and native mobile wrappers.

---

## 9. xui gaps this work will meet

Numbering continues PLAN.md §10. "Workaround" means LazyRAD proceeds without an xui change.

| # | Gap | Impact | Proposal |
|---|---|---|---|
| G19 | **No touch, pen or gesture events** (F9) | Phone-class desktop windows; any canvas-on-web | Upstream a `Pointer` event unifying mouse, touch and pen, plus pan/fling for `ScrollView`. The web gets it from the DOM, so M8 is not blocked |
| G20 | **No web backend** (F9) | Option A only | Out of scope until M11. Cheapest first step: drive `OffscreenBackend` (`inject` + `render`) from a JS shim |
| G21 | **Layout gaps**: no per-item margin; `justify` has no space-between / around / evenly; `Min` shares the entry's single sizing slot with `fill` and applies on the main axis only; a `Wrap` ignores `fill`; a `Row` gives its children no width bound, so wrapped text under-reports its height; no aspect ratio | Faithful `margin`, distribution, `min_*` with `fill`, wrapped text in rows | Workaround: a one-child wrapper for margin, spacer weights for distribution; `min_*` deferred. Upstream later |
| G22 | **No virtualized item list** for arbitrary row templates | M10–M11 lists | Upstream a lazy-list widget; windowed DOM on web |
| G23 | **Theme has no Material-like roles or state layers** | M9 tokens | Workaround: LazyRAD-owned painted widgets over an app-level token set; upstream when stable |
| G24 | **No animation driver** (timers exist, G18) | Transitions | Upstream a tick-based tween helper; CSS transitions on web |
| G25 | **No accessibility bridge on canvas backends** | Desktop screen readers | The model exists in xui-core; a bridge is a backend job. The web is covered by ARIA from the DOM |
| G26 | **The format is a closed enum** (F7): no `register` / `alias`, so LazyRAD-only kinds and user components have no registration path | P2, §4.6 | P2 models the three kinds upstream; components use an instance node. A generic extension node is the alternative (D3) |
| G27 | **No live layout** (F11): the mounted `Group` tree is immutable, layout kinds have no runtime properties | `responsive` patches and any script that wants to change a `gap` or `direction` | Workaround: re-mount the level over shared widgets (S3). Upstream: live layout cells like `layout::Placement`, so a layout field is a runtime property |
| G28 | **No structured layout report** (F12): `layout_report()` is a `String` naming widgets by type and text; `Diagnostic.node` is a widget name only | Designer badges (E4), conformance (F2), agents | Upstream a typed report (node path, control name, rect, warning kind) and `render --size` / `--report json` (A5) |

---

## 10. Risks, and decisions needed

### Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Layout parity drift** between xui and the browser. Appendix A already lists eight divergences from source | A written semantic subset, conformance fixtures (fixed-size **and** text-bearing) from the structured report (A5), no features only one engine has, "design flexibly" for text |
| R2 | **Scope creep toward "build Flutter"** | Every milestone ends in a demo with a gate; tiers (§7) are ordered; M8 excludes components, lists and navigation on purpose |
| R3 | **Web bundle size and startup** | Budgets measured in S1; no widget code or rasteriser in the wasm; prebuilt stub; lazy assets |
| R4 | **Two binding styles confuse users** (handlers that set properties vs bindings) | One-way only, ownership rule, lint, docs. Imperative code stays legal for anything not bound |
| R5 | **Rhai is synchronous**; web I/O is async | Callback-style `http`/`file` (the non-blocking `msg_box` pattern); the operation budget already limits runaway handlers |
| R6 | **Designer complexity** (tree drag and drop in a custom-painted toolkit) | Outline reorder and click-to-insert first; pure, window-free tree ops; reuse overlay and hit-test patterns |
| R7 | **Work stranded on stacked branches** (it already happened, F2) | P1 now. Process fix: retarget stacked PRs to `main` before merging the bottom one, or merge the stack top-down; a CI or bot check that a merged PR's base is the default branch |
| R8 | **Three repos pinned at different xui revs** | xui's W5.5 single pin; adoption (P4) lands against one rev; the musl CI job gates every PR |
| R9 | **LazyOS regressions** | Forms go through the `desktop` / `Platform` seams already in place; no `cfg(target_arch)` outside seams |
| R10 | **Bandwidth** | At most two agents at once; lanes are independent after A1 and B3; work packages are issue-sized for the existing dispatch workflow; xui-side work follows the established xui process |
| R11 | **Layout patches lose widget state** (F11) | Remount over shared widgets, proven by spike S3 before M8 relies on it; patches only at size-class changes; G27 as the upstream fix |
| R12 | **`xui-script` and LazyRAD's runtime diverge again** (F6) | P2b upstreams the deltas before adoption; one Rhai pin; LazyRAD keeps only its stdlib, `FsPolicy` and player |

### Decisions I need from you

| # | Decision | Recommendation |
|---|---|---|
| D1 | Web renderer: DOM (B) now, canvas (A) later as a fallback | **Yes**, with the §3.3 revisit trigger |
| D2 | **Land the stranded RON work now (P1)**, before anything else | **Yes**: it is the first implementation task, run in a scratchpad clone, never in `E:\repos\xui` |
| D3 | LazyRAD-only kinds (`Timer`, `Canvas`, `PictureBox`): model upstream as enum variants, or add a generic extension node | **Upstream variants** for the three (they are not LazyRAD-specific). Components get an instance node (§4.6) |
| D4 | Bindings and responsive overrides as **common side-table fields** (§4.2), not field-type wrappers | **Yes**, pending spike R1 |
| D5 | One `.lfm` file type; no `.lvw`. A form with `props` is a component | **Yes** |
| D6 | Native Android and iOS in scope? | **No**; PWA first, revisit at M11 |
| D7 | Should existing classic forms be exportable to the web? | **Yes, selectively**: `Absolute` maps to positioned elements, but only forms whose kinds have a DOM mapping export. Of the four samples only `calculator` is tier 0: `hello` needs `ComboBox` (M9), `todo` needs `ListView` (M11), `brickbreaker` needs `Canvas` (M11) |
| D8 | Top-level `let`s visible in handlers (LazyRAD's `36d2d21`; `xui-script` documents the opposite): upstream LazyRAD's behaviour | **Yes** (P2b). Bindings still see only `state`, `props` and `env` (§4.5) |

---

## 11. Review log

An independent Sonnet agent reviewed the first revision of this document against the
sources (read-only, 13 October 2026). The claims that changed the plan were re-checked
by hand before being adopted.

| Finding | Disposition |
|---|---|
| Layout patches cannot be applied in place to a mounted xui form | **Adopted**, re-checked (F11). Reduced from a blocker to a costed design constraint: remount over shared widgets is LazyRAD's existing `Placement` technique. Spike S3 and gap G27 added |
| The sample's `fill: 1` collapses on xui | **Adopted** (§4.2, §4.4): `fill` moved into a `Medium` patch, a validation warning, a fixture |
| The CSS mapping is not "nearly one for one" | **Adopted**: Appendix A divergences, text-bearing fixtures, `min_*` deferred, `Overlay` and `Scroll` clarified |
| `xui-script` is not a drop-in; Phase P mis-sequenced | **Adopted**, re-checked (F6): P2b, P2c, P3 before P4 (adoption) |
| Conformance and designer tooling does not exist yet | **Adopted** (F12, G28, A5) |
| No model-only build; `ScriptForm` tied to `Ui`; `time::now_*` panics; `Platform: Send + Sync` | **Adopted** (F8, P2c, §5.2) |
| Sizing and scope contradictions; D7 "for free" | **Adopted**: two-agent cap, P2 resized, D7 corrected, tiers reconciled |
| `state` is not an alias; binding scope; `env.platform` | **Adopted** (§4.5) |
| RON mechanics (`struct_names`, `Some(None)`, `LAYOUT_FIELDS`, unnamed layouts, diagnostics) | **Adopted** as R1 work (§4.2) |
| Docs, accessibility acceptance, WebKit, window mapping, `.lrp` work package | **Adopted** (P5, §4.2 row 5, gate, §5.2, work package D6 in lane D) |
| "No IME" is false on Win32; `now()` does not use `web-time`; `d798cd4` is the CLI commit; F5 counts | **Corrected** (F5, F9, §5.2) |
| Unverifiable: wasm compile, size and time budgets, third-party framework columns | **Open**: S1 and the budgets in §5.2 |

---

## Appendix A — Vocabulary map and the shared layout subset

Names are xui's (format 2). Everything in the first column is part of the semantic subset
both renderers must honour, **with the divergences listed after the table**.

| LazyRAD (`.lfm`) | xui `arrange` | CSS | Jetpack Compose | Flutter | Android XML |
|---|---|---|---|---|---|
| `Column` | `column()` | `flex-direction: column` | `Column` | `Column` | `LinearLayout` (vertical) |
| `Row` | `row()` | `flex-direction: row` | `Row` | `Row` | `LinearLayout` (horizontal) |
| `Flex` + `direction` | `row()`/`column()` by value | `display: flex` | `if`/`else` on size class | `Flex` | `LinearLayout` |
| `Wrap` | `wrap()` | `flex-wrap: wrap` | `FlowRow` / `FlowColumn` | `Wrap` | `FlexboxLayout` |
| `Grid(columns: …)` | `grid(tracks)` | `display: grid` | `LazyVerticalGrid` | `GridView` | `GridLayout` |
| `Overlay` | `overlay()` / `stack()` | stacked grid cell | `Box` | `Stack` | `FrameLayout` |
| `Scroll` | `scroll()` | `overflow: auto` | `verticalScroll` | `SingleChildScrollView` | `ScrollView` |
| `Absolute` + `at` / `anchor` | `absolute()` | `position: absolute` | `Box` + `offset` | `Positioned` | `ConstraintLayout` (partly) |
| `Panel(card: true)` | `Panel` + `look::card` | `div` + surface | `Card` | `Card` | `CardView` |
| `fill: n` | `Sizing::Fill(n)` | `flex: n 1 0` | `Modifier.weight(n)` | `Expanded(flex: n)` | `layout_weight` |
| `width` / `height` | `Sizing::Width` / `Sizing::Height` | `width` / `height` | `Modifier.width/height` | `SizedBox` | `layout_width/height` |
| `max_width` | `Item::max_width` | `max-width` | `widthIn(max=)` | `ConstrainedBox` | `maxWidth` |
| `gap` | `Layout::gap` | `gap` | `Arrangement.spacedBy` | `SizedBox` / `spacing` | divider / space |
| `padding` | `Layout::padding` | `padding` | `Modifier.padding` | `Padding` | `padding` |
| `align` / `align_items` | `Align` | `align-self` / `align-items` | `Alignment` | `CrossAxisAlignment` | `gravity` |
| `justify` | `Layout::justify` | `justify-content` | `Arrangement` | `MainAxisAlignment` | `gravity` |
| `responsive` + size classes | `env.size_class` | `@media (min-width: …)` | `WindowSizeClass` | `MediaQuery` / `LayoutBuilder` | `w600dp` qualifiers |
| `state` + `bind` | post-turn pass | — | `State` / `remember` | `setState` | `ViewModel` + data binding |

### Known divergences (read from source; spike S2 confirms or refutes each)

| Area | xui | CSS, naively | Handling |
|---|---|---|---|
| `fill` and content | A `Fill` item has zero natural main extent (`layout/tree/item.rs:232`) | `min-height: auto` shows the content | Validation warning; fixture; emit `min-*: 0` only where xui shrinks to zero |
| `fill` in a `Wrap` | Ignored (`wrap.rs`) | `flex: n 1 0` would stop the line ever wrapping | Emit no flex-grow inside a `Wrap` |
| Row children | No width bound at measure time, so wrapped text under-reports its height (`flow.rs`) | Grows to fit | Text-bearing fixtures; may need an xui change (G21) |
| `min_*` with `fill` | One sizing slot per entry; `Min` is in the rigid class | `min-width` and `flex` are independent | `min_*` deferred |
| `Overlay` | `overlay()` centres at natural size; `stack()` stretches | A stacked grid cell stretches | R1 picks one meaning |
| `Scroll` | Vertical only (`scrollview/content.rs`) | Either axis | Vertical only in M8 |
| `Grid` | `Auto` ignores spanning items; `Fill` rows are derived from items | `auto` and `fr` size differently | Emit `minmax(0, Nfr)`, `min-width: 0` and explicit rows |
| `Group` | A frame with content insets | `fieldset` has `min-inline-size: min-content` | Emit a `div` with a title |

**Not in the subset (so not used):** baseline alignment, negative margins, `auto`
margins, intrinsic-size cycles, `position: sticky`, and anything CSS grid can do that
`grid(tracks)` cannot.
