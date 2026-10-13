# LazyRAD — Modern UI and the web target

**Status: draft, revised 13 October 2026.** Proposes milestones **M8–M11** as a
companion to [PLAN.md](../PLAN.md). Only lane B (`lazyrad-view`) exists so far, as an
open PR under review; everything else is a plan. "Verified" means read from
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
| Animations as a first-class feature, in the spirit of Windows Phone 7 minus the 3D | **Motion** is declarative data on every node: enter and exit presets (`Fade`, `Slide`, `Feather`, a flat `Turnstile`, `Continuum`), implicit transitions, press feedback, reduced motion honoured. Transform and opacity only, no 3D (§4.8) |
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
6. **Motion is first class.** Enter and exit transitions, implicit transitions on change
   and press feedback are part of the document, the designer and **both** renderers
   from M8, in the spirit of Windows Phone 7's Metro motion with the 3D left out (§4.8).

**Non-goals (for M8–M10)**

- Native Android / iOS targets. A PWA covers phones; reconsider at M11.
- Pixel-identical rendering across targets. The contract is **layout semantics**
  (sizing, order, wrapping, breakpoints), not identical glyphs (§5.4).
- Running forms on the web through a canvas port of xui. Possible later (§3.3).
- A Material clone. The look is token-driven and restylable; it is not a spec port.
- **3D.** No perspective, no rotation about the X or Y axis, no flips or tilts. The 2D
  parts of the Windows Phone 7 effects (slide, fade, scale, stagger) are kept (§4.8).

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
| F13 | xui has **no per-node opacity or transform**, no shared frame clock and no region repaint: the present path allocates a surface and clones the whole frame per present, and `arrange` re-places a node whose bounds differ from its layout rectangle on any relayout | `xui-canvas/src/backend/app/present.rs`; `arrange/mount.rs:87`; `NodeSpec` | Motion on desktop is real work (P2d), not a configuration. G24, G29, G30 |

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
                       component expansion · motion · diff
                                          ──►  Patch { Create, Set, Unset, Animate, Move, Remove }
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
| Windows / Linux / macOS | `XuiRenderer` (xui canvas, `WinitBackend`) | xui `arrange` | Rhai, native | `.exe` via the existing stub + payload (PLAN.md §8) |
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
 diff against the last applied values  ──►  [Patch::Set / Patch::Unset …]
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

**Motion is data too (§4.8).** When a patch would `Set` an *animatable* property
(`opacity`, a translate, a scale, a rotation, a paint colour) on a node whose `motion`
declares a `change` transition, the core emits `Patch::Animate { from, to, duration,
delay, easing }` instead, and the renderer runs it. Enter and exit presets produce
`Animate` patches the same way. Layout fields never animate; they re-mount (above), and a
reflow is animated afterwards as a transform (FLIP, M9). The core owns a virtual clock
for tests; the renderers own the real one.

---

## 4. The form document: xui's format 2, plus six additions

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

All six are additive and keep the one-definition rule (format and schema come from the
same macros). Syntax below is a **sketch**; spike S5 (§8) confirms how the macros carry it.

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
| 6 | **`motion`** (a common field on widget nodes) | `enter`, `exit`, `change` and `press`, from a small preset vocabulary (§4.8) | Same side-table reason as `bind`. Motion is first class (goal 6), so it has a typed home in the schema rather than living in script |

Not additions, already there: `fill`, `align`, `align_items`, `justify`, `gap`, `padding`,
`max_*`, `span`, `Panel(card: true)` (my earlier `Card`), control arrays.

**Mechanics S5 must settle** (from the model's source):

- The canonical writer uses `struct_names(true)`, so a patch prints as
  `FlexPatch(direction: Horizontal)`, not the bare `( … )` of the sketch. The sketch is
  valid RON either way.
- Clearing an `Option` field in a patch needs `Some(None)`. Pick a reset idiom (an
  explicit `Reset` marker is clearer than the nested option).
- New common fields must also be added to the schema's `LAYOUT_FIELDS`, or `check` and
  `schema --json` will not list them.
- **Layout nodes have no `name` today** (`Row`, `Column`, `Wrap`, `Grid`, `Absolute`). A
  `bind` or `responsive` on one needs either an optional `name` or a tree-path address.
  `Flex` gets a `name`; the rest follow whichever S5 chooses.
- `Diagnostic.node` carries a widget name only, and a bad `bind` key is a map key, so a
  load error has no line, column or "did you mean". Validation of bind keys therefore
  reports through `Form::validate`, with the key and node.
- **Field order.** `node!` appends the layout fields *after* `content` and `children`,
  so what `fmt` writes is not the order of the sketches in this document. Declare the
  new common fields (`bind`, `responsive`, `motion`) before `content` if a stable,
  readable order matters.
- **The schema does not describe presets.** `FieldSpec.ty` is a type string only, so a
  preset enum with data (`Slide(from, distance)`, `Feather(stagger)`) will not show its
  structure in `schema --json`; the schema needs a way to say it, or the designer reads
  the variants some other way.
- **Prefer a defaulted `motion: Motion` over `Option<Motion>`**, so an absent field and
  an empty one are the same and the canonical writer omits it.
- **Node-name uniqueness** (found in lane B) conflicts with unnamed layout nodes and with
  control arrays (`digit[0]` … `digit[9]` share one declared `name`). The view core keys
  nodes by a unique string name and rejects empty and duplicate names; S5 decides how
  unnamed layouts and array elements get one (a name, an index or a path).

### 4.3 Kinds

| Group | Kinds | M8 | Maps to (xui) | Maps to (DOM) |
|---|---|---|---|---|
| Flow | `Row`, `Column`, `Wrap`, `Grid` (**exist**) | ✓ | `row()` / `column()` / `wrap()` / `grid()` | `display:flex` / `flex-wrap` / `grid` |
| Flow | `Flex` (**new**) | ✓ | `row()` or `column()` by `direction` | `display:flex` |
| Layering | `Overlay` (**new**) | ✓ | `stack()` (children stretch) or `overlay()` (centred at natural size): S5 picks one meaning | stacked grid cell |
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
  `state.count += 1`. Reads must not clone the whole map each time (the first cut of
  `lazyrad-view` clones it twice per turn; §11.1 lists the fix).
- **Reserved names:** `state`, `props` and `env`. LazyRAD already has shared top-level
  `let`s (PLAN.md §4.1); bindings do **not** see them, only the three reserved names, so
  an expression's inputs are explicit.
- **A binding** is evaluated by the same Rhai engine on both targets, in a
  **side-effect-free engine** (no assignment, no stdlib I/O). It sees `state`, `props`
  (M9) and `env` only, **not control properties**: reading controls would make evaluation
  order observable and allow cycles. Evaluated at build, after every event turn, and on
  `env` change; not per frame. **Implementation note (lane B):** the two triggers
  collapse into one `update(state, env)`. Skipping state-reading bindings on a pure
  resize needs state-change detection; the first cut deep-compares the state map, and
  a **version counter on the shared state handle** is the cheaper design.
- **Tooling:** the debugger's step-into skips `.lfm` expressions; renaming a control
  rewrites the `bind` and `label_for` strings that name it, as `rename_handlers` does for
  handlers.
- **Ownership:** a bound property belongs to its binding. A handler that writes it is a
  lint warning. A failing expression keeps the last good value and reports in the Output
  pane; it never crashes the app. Expressions are compile-checked at load.
  **Two mechanisms, one consequence** (found in lane B): the session diffs against the
  last value it *applied*, so an unchanged binding emits nothing, and the session also
  skips a `Set` equal to the renderer's live value (`Renderer::live_value`) to keep the
  caret. Together, a value the *user* changed with no handler sticks until the bound
  value next changes: the widget is not snapped back. A true controlled input (revert
  on a rejected edit) would need the renderer to report edits, which is why inputs bind
  through a handler that writes `state` (below).
- **Typed properties.** A binding's result must match the property's kind. The kind
  comes from the **catalog schema**, not from the first value a binding happens to
  produce. Lane B had no schema wired in, so it types a property from its literal or
  patch, and a bound-only property from its first good result: the fallback this
  sentence deprecates.
- **Two-way input** stays explicit (`fn name_edit_change(text) { state.name = text; }`):
  transparent in the debugger, no magic.
- `env` = `{ width, height, size_class, dark, text_scale, reduce_motion }`, read-only.
  There is no `env.platform`: it would invite the `if web` branches the M8 gate forbids.

Size classes follow Material 3 by width, cascading **mobile-first** like CSS `min-width`
queries. The cascade is **per property**, not whole-patch inheritance: a `Medium` patch
that sets only `gap` leaves a `Compact` patch's `direction` in force. When the window
shrinks again, a property returns to its **base value** (a `Set` of that value);
`Patch::Unset` applies only when no value sets the property at all:

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

### 4.8 Motion (first class)

Motion is **declarative data in the form**, like layout and bindings, not code you
remember to call. That is what lets the designer edit it, both renderers honour it, and
tests assert it. The vocabulary is Windows Phone 7's *Metro* motion, **minus the 3D**.
The facts about the originals below were checked against the Windows Phone Toolkit
source (`microsoftarchive/WindowsPhoneToolkit`), not recalled.

**Rules**

1. **Transform and opacity only.** The animatable properties are those that do not
   change layout: `opacity`, `translate_x`, `translate_y`, `scale_x`, `scale_y`,
   `rotate` (2D; used by `Roll` and `Rotate`) and, from M9, paint colours (they need
   colour properties first, G17). Each preset fixes its **transform origin**: the
   node's centre for `Roll`, `Rotate` and `PressScale`, the left edge for `Turnstile`,
   and `-0.2` of the width for `Feather`'s items; an `origin` property is M9.
   `width`, `gap`, `fill` and the like are **not** animated: layout is
   immutable once mounted (F11), and a reflow per frame is what makes UI janky. A
   reflow itself is animated with FLIP (measure the first and last rectangle, animate
   the difference as a transform; M9). The Toolkit works the same way: it animates
   render transforms and opacity, and `Transition.Begin()` switches the element to a
   bitmap cache so they run on the compositor.
2. **No 3D.** No perspective, no rotation about the X or Y axis, no Z translation, no
   `Swivel`, no flips, no tilt depth. Where an effect *is* a 3D rotation with no 2D
   remainder (`Turnstile`, and `Feather`'s per-item rotation), we offer a **flat
   reading** and label it as ours (table below). The DOM renderer must never emit
   `perspective`, `rotateX`, `rotateY`, `rotate3d`, `translateZ`, `translate3d`,
   `scale3d` or `matrix3d`; a conformance lint checks. (`rotateZ` is 2D and allowed.)
3. **Reduced motion is part of the contract.** `env.reduce_motion` carries the OS
   setting (`prefers-reduced-motion` on the web). When it is set, presets resolve to a
   plain fade or an instant change and nothing loops.
4. **Fast in, slow out.** The Toolkit uses `ExponentialEase` (ease-out, exponent 6) for
   its transitions: durations of 250 to 350 ms (a turnstile is 350 ms in and 250 ms out;
   slide left and right 500 ms in and 300 ms out; roll 600 ms), a 40 ms stagger in and 50 ms out for
   feather, and a tilt that releases over 100 ms after a 200 ms delay. These become
   motion **tokens**, tuned by eye and editable in the theme editor (M9). An
   exponential ease-out is not a cubic-bezier, so the web uses a CSS `linear()`
   approximation and the fixtures carry a tolerance (R14).

**Presets** (the 2D subset, and what is ours):

| Preset | What it does | WP7 origin (checked) | What is ours |
|---|---|---|---|
| `Slide(from, distance)` | translates in from an edge, with an opacity fade | Toolkit `SlideTransition`: translate (about 200) plus opacity, exponent 6 | nothing: this is the original's 2D |
| `Fade` | opacity 0 ↔ 1 | the opacity half of Slide and Rotate. Turnstile and Swivel do **not** fade: their opacity switches in a step | the preset as a standalone |
| `Roll` / `Rotate` | rotates about the node's centre (`Rotate` also fades in; `Roll` does not) | Toolkit `RollTransition` and `RotateTransition`: `RotationZ` about the centre, which is **2D** | nothing: kept as is |
| `Turnstile` (flat) | the node squashes in horizontally from its left edge, `scale_x` from about 0.17 to 1 (the orthographic width of the original's 80° turn), mirrored on exit | Toolkit `TurnstileTransition`: `RotationY` of -80° to 0 about the left edge, no slide and no fade | **the flat projection of a 3D rotation.** A plain `Slide` is the alternative (D11) |
| `Feather(stagger)` | the direct children of the container's content layout (each a widget node, or wrapped in a `Panel`) enter one after another, each a short flat `Turnstile` (or `Slide`), 40 ms apart in and 50 ms out | Toolkit `TurnstileFeatherEffect`: a per-item `RotationY` about `CenterOfRotationX = -0.2`, 350 ms, staggered | the stagger is the original's; the per-item motion is the flat reading |
| `PressScale` | a pressed item eases down to about 0.96 and back, releasing over 100 ms after a 200 ms delay | Toolkit `TiltEffect`: a tilt of up to 0.3 rad plus a depression of up to 25 px in Z | the scale, and the 0.96; there is no tilt |
| `ProgressDots` | indeterminate dots sliding across | Toolkit `PerformanceProgressBar` | nothing |
| `Continuum(key)` | a shared element carries from one place to another while the rest slides and fades | the platform's Continuum pattern. **Not in the Toolkit**; its behaviour was not verified here | ours: the web's View Transitions and xui's FLIP define it |
| `Pivot` / `Panorama` slide | header and content move at different speeds | the platform controls' look. **Not in the Toolkit** (only `LockablePivot`) | ours (finger tracking needs G19) |
| *Swivel*, *flip*, tile flip | excluded | Toolkit `SwivelTransition`: `RotationX` (3D) | all of it |

**Document (a sketch, like §4.2).** One common field, `motion`, so no widget field
changes type. It is accepted on **widget nodes** (`Panel`, `Group`, `Tabs`, `Label`,
`Button`, …); layouts (`Row`, `Column`, `Flex`, …) are entries in a parent's tree, not
backend nodes, so to animate one, wrap it in a `Panel`:

```ron
Panel(name: "counter_card", card: true, motion: Motion(
    enter:  Slide(from: Bottom, distance: 40),
    exit:   Fade,
    change: { "opacity": Ease(ms: 200), "translate_y": Ease(ms: 300) },
    press:  PressScale,
), content: Column(padding: 12, children: []))
```

The six properties of rule 1 are **common runtime properties** of every widget node (like
`visible` and `enabled`), read and write, so `change`, `bind` and `animate()` have
something to write. They default to `opacity: 1` and the identity transform and are never
saved to the file at their defaults.

- `enter` and `exit` run when a node is created or removed, or when its `visible`
  changes. `enter` is gated on **node identity**, so it does not replay on a hot reload
  or on a rebuild of a subtree. An exit holds the node, and the room it takes, until it
  finishes, then removes it (collapsing the room is M11). On xui, an exit through
  `visible` delays `set_visible(false)`; a *structural* exit (M9) needs a ghost entry
  carried through the remount of §3.4 (S3 covers it).
- `change` makes a changed *animatable* property, bound or scripted, animate from the
  last applied value instead of jumping.
- `press` is feedback for buttons and other activatable nodes.
- Scripts: `card.animate(#{ opacity: 1.0, translate_y: 0.0 }, 250)` writes the same
  `Animate` patch. It bypasses the binding diff, so animating a property that is also
  bound is a lint warning, like any write to a bound property. In M9
  `card.play("pulse")` runs named keyframes declared on the form. Both honour reduced
  motion.

**Pipeline.** `Patch::Animate { node, property, from, to, duration_ms, delay_ms,
easing }` sits beside `Set` (§3.1). The view core decides **what** animates: it already
knows the last applied value, the new one and the node's `motion`. The renderer decides
**how**: the DOM renderer uses CSS transitions and the Web Animations API, on the
compositor; the xui renderer runs a `MotionDriver` and needs per-node opacity and
transform from the backend (G29). Four contract points:

- **`from` may be `Current`.** The core knows only the applied *target*, not the
  interpolated value, so an interrupted animation starts from where the renderer is.
- **Completion is reported.** The renderer calls back `finished(node, property)`, so the
  core can sequence a `Remove` after an exit and an `enter` after a delay.
- **Skip-if-equal does not cover `Animate`.** `Session::update_with` drops a `Set`
  that equals the renderer's live value and passes every other patch through
  (`session.rs`), so an `Animate` towards the live value needs the core's own check,
  and an `Unset` of an animatable property applies at once. The "already entered"
  set behind identity-gated `enter` must also survive `reset()`, which `build` and
  `replace_tree` call today.
- **Two clocks.** The core has a virtual clock (`advance(dt)`), so golden traces assert
  values at fixed times (t = 0, 100 ms, 250 ms). A real run is driven by the renderer.
  `OffscreenBackend::set_timer` returns `TimerId(0)`, so `MotionDriver` takes an
  injected `tick(now)` rather than owning a timer.

A renderer that cannot animate a property applies `to` at once.

**On the web** (renderer-local; none of it is decided by the core): FLIP needs measured
rectangles, and a reflow there comes from `@media`, so "first" is cached through a
`ResizeObserver`. A staggered `animation-delay` needs `animation-fill-mode: backwards`
so the item does not flash before it starts. The View Transitions API makes the page
non-interactive while it runs and snapshots in-flight animations; its use for
`Continuum` is to be verified at M9.

**In the designer** (§6.1): a motion inspector per node, play and scrub of the entrance
in the preview, a reduced-motion toggle, and lints (animating a layout property, a
duration over 500 ms, a loop, a 3D name).

---

## 5. Runtime, crates and packaging

### 5.1 Crate changes

| Where | Change |
|---|---|
| **xui** `xui-form` | The model additions of §4.2; tree ops (§6.2); **a `build` feature gate** (none exists: only `migrate`) so the web crate links the model without widgets. Developed through xui's established process, then pinned by LazyRAD |
| **xui** `xui-script` | LazyRAD's deltas (P2b: `build_deferred`, `reload`, `state`/`set_state`, `StopHandle`, `is_host_handle`, `control_methods`, `to_arg_value`, `source_name`, top-level-variable semantics); a **renderer-neutral session split** (P2c): engine, AST, function table, run/load/close and callbacks apart from `Ui`/`LiveForm`; Rhai `debugging` behind a feature |
| **xui** `xui-core` / `xui-canvas` | Per-node opacity and 2D transform in the compositing path (G29); a shared frame clock (G24); a reduced-motion setting (G30). Translate works on the backend through node moves, but `arrange` undoes it on any relayout until P2d-c |
| `lazyrad-view` (new, LazyRAD) | Renderer-neutral and intended to be `wasm32`-clean (not yet compiled for it: no target is installed): `Environment`/size classes, responsive cascade, **the single home of the binding pass**, its own small `Tree`/`Node` model, `Patch` (including `Unset`, and `Animate` once the motion planner lands), a virtual clock, `Renderer` trait, golden patch traces. **No `xui-form` dependency** (it depends on `rhai` and `thiserror` only), so a mapper from the RON `Form` to its `Tree` is separate work (B5). Upstream to xui once stable |
| `lazyrad-runtime` | Drops its `xui-form`/`xui-rhai` copies for xui's (P4); calls `lazyrad-view`'s binding pass; feeds `Resize` and reduced motion into `env`; hosts the xui renderer's `MotionDriver`. Gains a `wasm32` CI job (it already has the `desktop` split for musl), a `canvas-control` feature so the web build drops `xui-canvas`, and a web-safe `Platform` (today `Send + Sync`) |
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
- **Motion fixtures.** The virtual clock gives golden values for each preset at fixed
  times (xui side). On the web the animation is paused and its `currentTime` set before
  the computed style is read. Values must agree within a small tolerance, and both sides
  use the same easing curves. A lint fails the build if a DOM mapping emits any 3D
  transform, and a fixture runs every preset under reduced motion.

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
| **Motion inspector** | Per node: pick `enter`, `exit`, `change` and `press` presets, set duration, delay, stagger and an easing curve; lints for layout-property animation, over-long durations and loops | Expression Blend behaviours, Flutter motion tooling |
| **Play and scrub** | A play button and a scrubber over the form's entrance (including staggers) in the preview; a reduced-motion toggle | Android Studio Animation Preview |

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

Free-form constraints (ConstraintLayout-style relational anchors), a **freehand
multi-track timeline editor** and a visual state-machine editor. Motion itself is not on
this list: presets, implicit transitions and named keyframes are first class (§4.8).
What stays out is drawing arbitrary multi-track animation by hand. Constraints do not
map to CSS cleanly, and the timeline and state machine are a separate product.

---

## 7. Component library tiers

| Tier | Contents | Milestone | Notes |
|---|---|---|---|
| 0 Core | `Row`, `Column`, `Flex`, `Overlay`, `Scroll`, `Panel`, `Spacer`, `Separator`, `Label`, `Button`, `Edit`, `CheckBox`; **motion**: `Fade`, `Slide`, `PressScale` and `change` transitions on opacity, translate and scale | M8 | Both renderers; enough for forms and dashboards |
| 1 Standard | `Wrap`, `Grid`, `ToggleButton`, `Slider`, `ProgressBar`, `ComboBox`, `RadioGroup`, `Image`, `Icon`; button variants (filled, tonal, outlined, text); **motion**: `Feather`, flat `Turnstile`, `Roll`, `Rotate`, `ProgressDots`, `Continuum` within a form, the non-finger `Pivot`/`Panorama` slide, FLIP layout transitions, named keyframes, motion tokens | M9 | Painted xui widgets on `look::*` tokens; CSS counterparts |
| 2 Adaptive | `Scaffold` (app bar, body, drawer), `NavigationBar` (Compact) ↔ `NavigationRail` (Medium) ↔ `NavigationDrawer` (Expanded), list-detail pane, dialog, sheet, snackbar, `Tabs`; **motion**: route transitions (forward and back), `Continuum` across routes | M9–M10 | The adaptive trio is the best demonstration of "responsive"; route motion needs the router |
| 3 Data | virtualized list, data table, tree, validation helpers | M10–M11 | `ListView`/`TreeView` exist on xui; the web needs virtualization |
| 4 Rich | rich-text editor, charts, code editor, `Canvas` | M11 | May be xui-only on desktop at first; `Canvas` maps to `<canvas>` |

---

## 8. Milestones

Sizes are rough, for one developer, as in PLAN.md §11. Each milestone ends in a demo.

### Phase P — Land and adopt xui's format 2 (≈ 10–12 weeks with two agents in tandem, P2d included)

Mostly xui's own W3/W4, plus the deltas LazyRAD needs. It is worth doing on its own: it
cuts LazyRAD's calculator form from 159 lines to about 40. **M8's model and view-core
lanes (A, B) need only P1–P2c; the desktop and designer lanes (C, E) need P4, and the
desktop motion work (C4) also needs P2d.**

| # | Work | Repo | Size | After |
|---|---|---|---|---|
| P1 | **Land #275 and #276 on `main`.** Branch from `main`, take `f4f2e91` (RON) and `d798cd4` (CLI) (the squash `ede6e5a` already carries the move), resolve `CHANGELOG.md` and `Cargo.toml`, re-run CI. Leave the stale stacked branches for the owner to delete | xui | days | — |
| P1b | **Harden format 2** against the first independent review of it (§11.1): `migrate` must reject invalid v1 (duplicate names, leaf or cyclic parents) and produce grids that land on the v1 pixels (asymmetric right margin, nested `Group`); `to_ron` must not panic at or past RON's recursion limit; `array`/`index` need caps and are rejected on containers; plus the minors listed there | xui | ≈ 1 wk | P1 |
| P2 | **Model the kinds LazyRAD has and the RON model lacks:** `Timer`, `Canvas`, `PictureBox`, `Button.icon`, with their factories. About 3,000 lines move onto xui's factory API. Upstream, because the model is a closed enum (D3) | xui | 2–3 wk | P1 |
| P2b | **Upstream LazyRAD's `xui-rhai` deltas** into `xui-script` (the nine names in F6) and settle top-level-variable semantics (D8) | xui | ≈ 1 wk | P1 |
| P2c | **Wasm factoring:** a `build` feature gate in `xui-form`; a renderer-neutral `ScriptForm` session split; Rhai `debugging` behind a feature | xui | 1–2 wk | P1 |
| P2d-a | **Clock, easing and reduced motion:** a shared frame clock, the Metro easing set (exponential ease-out and friends) and `Backend::prefers_reduced_motion()` (G24, G30). The Win32 backend already calls `SystemParametersInfoW` (`xui-win32/src/sys/dwm.rs`); macOS and Linux are unverified | xui | days | P1 |
| P2d-b | **Region repaint and per-node layers** (G29). Today the present path allocates a fresh surface per frame, composites every node, then clones the whole frame (`xui-canvas/src/backend/app/present.rs`); `invalidate_rect` ignores its rectangle and `Surface::paint_region` is used only in tests. Region repaint comes first; layers (paint a subtree, composite with opacity and transform) sit on it | xui | 2–3 wk (spike S4 sizes it) | P1 |
| P2d-c | **A translate that survives relayout.** The backend moves a node by a parent-relative rectangle, but `arrange/mount.rs` re-places any node whose bounds differ from its layout rectangle, so any relayout (a text change, a resize, `set_visible`) snaps an animated node back. Needs an offset layer in `place()`, or the driver re-asserts offsets after each layout. Transforms are **visual only**, and one input rule holds on both targets, as in the Toolkit (`IsHitTestVisible = false` during a transition): **a node takes no input while its `enter` or `exit` runs** (`pointer-events: none` on the web); otherwise hit-testing stays on the layout rectangle. Hit-testing through a transform is M11 | xui | ≈ 1 wk | P2d-b |
| P3 | **Tree API:** path-addressed `NodePath`; `insert_at`, `move_to`, `remove`, `rename`, `wrap`, `unwrap`, each returning its inverse (§6.2) | xui | 1–2 wk | P1 |
| P4 | **Adopt.** One xui rev (W5.5); delete LazyRAD's `xui-form`/`xui-rhai` copies; port the `FormDoc` sites (designer onto the P3 tree API, runtime, IDE, project); `xui-form migrate` the samples and templates; keep hot reload, rename-handlers and the debugger working | LazyRAD | 3–4 wk | P2, P2b, P3 |
| P5 | **Docs:** PLAN.md §1 (TOML), §1.1 (`GroupBox`), §2, README, samples, tutorial | LazyRAD | days | P4 |

**Gate:** the four samples and the IDE templates run, debug and export from format-2
forms (through the kinds P2 added); the designer passes today's designer tests re-based
on the tree; hot reload and the debugger work on format-2 forms; the musl CI job is green.

### Phase 0 — Spikes (≈ 1–2 weeks; S1, S2 and S4 run beside P1, S3 and S5 need P1)

Cheap experiments that can change the plan. Each has a pass condition.

| # | Spike | Pass condition |
|---|---|---|
| S1 | `cargo check --target wasm32-unknown-unknown` for `xui-core`, `xui-form` (a scratch `cfg` gate of its builder stands in for P2c), `rhai =1.26.1`, plus a "hello wasm" running a Rhai script and measuring size | Compiles; gzipped wasm within the §5.2 budget. Expected runtime panics (they compile, then panic on `wasm32-unknown-unknown`): `SystemTime::now` (`stdlib.rs:384`), `time::OffsetDateTime::now_*` (`stdlib.rs:430-436`), `Instant::now` in the `Canvas` control |
| S2 | Hand-write the §4.2 form as HTML and CSS; compare to `xui-form render --report` at 360, 700 and 1000 px, with **text-bearing** fixtures (wrapping text in a `Row`, a `fill` card, a `Wrap`) and the Appendix A divergences | The divergences are confirmed or refuted; no semantic that needs a hack; fixed-size rectangles within ±1 px |
| S3 | **Remount over shared widgets** (§3.4): on a built RON form, change a `Row` to a `Column` and a `gap` through a level re-mount that reuses the widgets, while an `Edit` has focus, caret and text and a scroll view is scrolled | Text, caret, selection, focus and scroll offset survive; no widget is recreated; cost measured on the calculator and a 200-node form. If it fails, G27 (live layout cells) moves into M8 |
| S4 | **Motion on the xui canvas** (§4.8). First, a **throwaway region repaint** in `composite`/`redraw` on a spike branch (nothing uses `paint_region` outside tests; the product version is half of P2d-b, 2–3 weeks, and is *not* part of this spike). Then fade and translate a subtree of about 30 widgets (a card with text, an `Edit` and a button) at 60 frames per second, with per-node opacity done by painting the subtree into a layer and compositing it. Baseline to beat: a replica of today's present path (allocate, fill, clone, convert) cost about 8 ms per 1080p frame **before any widget painted** (an indicative measurement, release build) | Frame time under 16 ms on a mid laptop at 1080p with the animated region at 400 × 300 on the throwaway region path; the layer composite cost measured separately; the cost on the LazyOS software path measured. Hit-testing through a transform is *not* a criterion: transforms are visual only (P2d-c). If it fails, the presets degrade on xui to translate-only or instant, and M8 says so |
| S5 | Prototype `bind`, `responsive` and `motion` as common fields in the `node!`/`widget!` macros (including the generated all-optional patch type and the mechanics in §4.2) | Round-trips byte-stable; `Form::validate` reports a bad key; the catalog JSON lists all three; a layout node can be addressed |

### M8 — Core: the small proof (≈ 10–12 weeks wall-clock, two agents in tandem, after P1 to P2d)

Scope: tier 0, bindings, responsive overrides, **the motion core (§4.8: `Fade`, `Slide`,
`PressScale`, `change` transitions, reduced motion)**, the xui and DOM renderers, web
export, and the flow-designer essentials. **Out of M8:** components, lists,
conditionals, navigation, the theming editor, `Turnstile`, `Feather`, `Roll`, `Rotate`,
`ProgressDots`, `Continuum` and the non-finger `Pivot`/`Panorama` slide (all M9), route
transitions (M10), FLIP and keyframes (M9), hit-testing through a transform (M11), http.
Sizes assume at most **two agents at once** (the
dispatch workflow's cap), so the lanes below are ordered by what blocks what, not run
all at once: A and B first (they need only P1–P2c), then C, D and E.

| Lane | Work packages (issue-sized) |
|---|---|
| **A Model** (xui) | A1 `bind` field, validation and expression compile check. A2 `responsive` patches and size-class cascade in the schema. A3 `Flex`, `Scroll`, `Overlay`, `Spacer` kinds with factories. A4 `margin` (wrapper); `min_*` deferred (G21). A5 CLI: `render --size`, `--report json` (node path, control name, warnings), `label_for` and `accessible_name`. A6 the `motion` field, its presets and validation (animating a layout property, a loop, a 3D name), **and the six common runtime properties** `opacity`, `translate_x/y`, `scale_x/y`, `rotate` (they must exist for `change`, `bind` and `animate()` to write; `motion` is accepted on widget nodes only) |
| **B View core** (`lazyrad-view`) | B1 `Environment` and cascade (and `reduce_motion`, which changes #119's `to_map`: it pins five keys). B2 binding evaluator and `Patch` diff, skip-if-equal. B3 `Renderer` trait, its own `Tree`, golden patch traces. **B3 and A1 define the contracts the other lanes need first.** *Status: B1 to B3 are in review as [#119](https://github.com/va1erian/lazyrad/pull/119) (fixes in progress); what it found is in §11.1.* **B5 the mapper** from the RON `Form` (its `bind`, `responsive` and `motion` fields) to `lazyrad-view`'s `Tree`; it needs A1, A2 and A6 and nothing in #119 owns it. **B4 the motion planner:** `Patch::Animate` with `from: Current`, a `finished` callback, presets expanded into animations, the `enter`/`exit` lifecycle (an exit holds the node until it ends; `enter` is gated on node identity so a rebuild does not replay it), the virtual clock, reduced motion |
| **C Desktop** | C1 builder: new kinds and layout fields → `arrange`, layout patches by re-mount (S3). C2 runtime: calls B2 after each turn, `Resize` → `env`, shared `state` handle. C3 lint: bound-property writes, unknown state keys, binding compile errors (`check_project`). C4 the xui renderer's `MotionDriver`, driven by an injected `tick(now)` (`OffscreenBackend::set_timer` returns `TimerId(0)`): opacity and transform through G29, translate through P2d-c's offset layer, reduced motion from the OS. C5 `animate()` in the Rhai bridge: the same `Animate` patch, with a lint when the property is also bound |
| **D Web** | D1 `lazyrad-web` skeleton, bundle loader, web `Platform`. D2 `DomRenderer` and `DomHost`, event delegation. D3 CSS emitter with media queries, driven by the catalog. D4 dev server and *Run in browser*. D5 *Make Web App* and the stub CI job. D6 `.lrp` `targets` and `[web]` (`lazyrad-project`). D7 motion in the DOM renderer: `Animate` patches to the Web Animations API, presets to CSS, `prefers-reduced-motion` into `env` (D3's emitter gains the preset keyframes) |
| **E Designer** | E1 viewport bar and size-class badge. E2 drop indicators and outline drag. E3 property grid: layout fields per parent, sizing control, responsive indicator, binding field. E4 layout-warning badges (from A5's structured report). E5 quick actions (wrap, unwrap, move). E6 motion inspector, play and scrub, reduced-motion toggle and motion lints |
| **F Quality** | F1 `examples/modern_hello` (the demo below). F2 conformance fixtures (fixed-size and text-bearing) and the Chromium and WebKit job. F3 web smoke test (load the bundle, click, assert) plus an axe-core audit of the demo. F4 motion fixtures: golden values per preset at fixed times on both renderers, every preset under reduced motion, and the no-3D lint |

**The demo that closes M8**

1. `lazyrad-player examples/modern_hello` opens a native window at 720 × 560. The cards
   slide up and fade in as it opens. Dragging the window narrower than 600 dp flips the
   two cards from a row to a column. Pressing *Add one* gives a small press-scale on the
   button. (Text is not animatable, so the count itself changes at once.)
2. In the IDE: open `profile_form.lfm`, drag a `Button` from the toolbox into the `cards`
   row, set `fill: 1`, switch the viewport to *Phone*, press F5.
3. *Make Web App*, serve `dist/`, open it in a browser: the same behaviour, the same
   `.rhai` file byte for byte, text is selectable, Tab moves focus in tree order, every
   input has an accessible name (`label_for`), and axe-core reports no critical
   violations. The same entrance and press motion play; with the OS or browser set to
   reduced motion they become a plain fade, and nothing loops.

**Gate to M9 (go / no-go)**

- The demo works on Windows and in Chromium, Firefox and WebKit.
- S3 passed: layout patches keep text, caret, focus and scroll offset.
- S4 passed, or the xui degradation is written down and the web target still ships the
  full presets (a deliberate, visible asymmetry, not a silent one).
- Motion fixtures pass on both renderers: golden values at fixed times, every preset
  under reduced motion, and no 3D transform anywhere.
- Conformance fixtures pass; known deviations are listed, not hidden.
- Web budgets (§5.2) are met, or the miss is understood and costed.
- Classic (absolute) forms still open, run and export unchanged; musl CI green.
- No `if web` anywhere in a sample's script.

### M9 — Compose: components, lists, theming (≈ 5–6 weeks)

Tier 1 components and the adaptive scaffolds; `props`, `Component`, `Repeat`, `Show`;
design tokens (colour roles, type scale, shape, spacing, density) with a theme editor,
mapped to xui's `Theme` and CSS custom properties; extract-component, per-breakpoint
editing, sample data for previews; xui's *Convert to flow layout* assistant. **Motion:**
`Feather`, flat `Turnstile`, `Roll`, `Rotate`, `ProgressDots`, `Continuum` within a form, the
non-finger `Pivot`/`Panorama` slide, **FLIP layout
transitions** (a responsive reflow, a reorder or an insert animates as a transform),
named keyframes (`card.play("pulse")`), motion tokens in the theme editor, and the
designer's scrubber. Evaluate `xui-blitz` as an in-IDE *web preview* tab and as a layout
oracle.

### M10 — Rich SPA (≈ 5 weeks)

Router (`form.navigate`, params, deep links, browser history on web, an in-app stack on
desktop); `http` (callback-based, since Rhai is synchronous) and `json`; persistent
`settings`; validation helpers; i18n; an accessibility audit in the designer (labels,
focus order, `Color::contrast_ratio`); PWA manifest and offline cache; prerendered first
paint; per-route code splitting; performance budgets in CI; a security pass (URL schemes
on `Hyperlink` and `http`, the web `FsPolicy`, no `eval`-like surfaces in bindings).
**Motion:** route transitions (forward and back: flat `Turnstile`, `Slide`, `Continuum`
across routes), with a first paint that carries no motion when prerendered.

### M11 — Breadth (open-ended)

Data table and virtualized lists; dialogs, sheets, snackbars; collapsing room on exit,
inertia and optional spring physics; gesture-driven motion (finger-tracking `Pivot` and
`Panorama`, swipe to dismiss; needs G19); touch and gestures (G19); container queries; `Canvas` on web via `<canvas>`; the
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
| G24 | **No animation driver**: per-widget timers exist (G18), but there is no shared frame clock, easing set or tween helper | Every preset in §4.8 | Upstream a frame clock and an easing library (exponential ease-out with exponent 6, cubic, the ones Metro uses); LazyRAD's `MotionDriver` sits on them and takes an injected `tick(now)`. CSS and the Web Animations API on the web (P2d-a) |
| G25 | **No accessibility bridge on canvas backends** | Desktop screen readers | The model exists in xui-core; a bridge is a backend job. The web is covered by ARIA from the DOM |
| G26 | **The format is a closed enum** (F7): no `register` / `alias`, so LazyRAD-only kinds and user components have no registration path | P2, §4.6 | P2 models the three kinds upstream; components use an instance node. A generic extension node is the alternative (D3) |
| G27 | **No live layout** (F11): the mounted `Group` tree is immutable, layout kinds have no runtime properties | `responsive` patches and any script that wants to change a `gap` or `direction` | Workaround: re-mount the level over shared widgets (S3). Upstream: live layout cells like `layout::Placement`, so a layout field is a runtime property |
| G28 | **No structured layout report** (F12): `layout_report()` is a `String` naming widgets by type and text; `Diagnostic.node` is a widget name only | Designer badges (E4), conformance (F2), agents | Upstream a typed report (node path, control name, rect, warning kind) and `render --size` / `--report json` (A5) |
| G29 | **No per-node opacity or 2D transform** in the backends; a node is a device-pixel rectangle painted in creation order, and the present path allocates and clones a whole frame per present with no region path. Translate works on the backend (parent-relative moves), but `arrange` re-places a node on any relayout; fade, scale and rotate do not exist | `Fade`, `PressScale`, `rotate`, `enter`/`exit` on desktop | Upstream region repaint first, then a node opacity and transform: paint the subtree into a layer and composite it (tiny-skia `draw_pixmap`); transforms are visual only, so hit-testing stays on the layout rectangle (hit-testing through a transform is M11). Sized by spike S4 (P2d-b, P2d-c). Interim: presets degrade to translate-only or instant on xui, declared per preset |
| G30 | **No reduced-motion setting**: nothing reads the OS animation preference (Windows `SPI_GETCLIENTAREAANIMATION`; `prefers-reduced-motion` on the web) | The reduced-motion rule of §4.8 (accessibility) | Upstream `Backend::prefers_reduced_motion()` with a default of `false`. The Win32 backend already calls `SystemParametersInfoW` (`xui-win32/src/sys/dwm.rs`); macOS and Linux are unverified; LazyOS reports its own setting |

---

## 10. Risks, and decisions needed

### Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Layout parity drift** between xui and the browser. Appendix A already lists eight divergences from source | A written semantic subset, conformance fixtures (fixed-size **and** text-bearing) from the structured report (A5), no features only one engine has, "design flexibly" for text |
| R2 | **Scope creep toward "build Flutter"** | Every milestone ends in a demo with a gate; tiers (§7) are ordered; M8 excludes components, lists and navigation on purpose |
| R3 | **Web bundle size and startup** | Budgets measured in S1; no widget code or rasteriser in the wasm; prebuilt stub; lazy assets |
| R4 | **Two binding styles confuse users** (handlers that set properties vs bindings) | One-way only, ownership rule, lint, docs. Imperative code stays legal for anything not bound |
| R5 | **Rhai is synchronous**; web I/O is async. **The operation budget is leaky**: Rhai restarts its operation counter inside callbacks such as `array.map(closure)` (found in lane B), so a runaway there is not bounded by `max_operations` or by the hook's own argument | Callback-style `http`/`file` (the non-blocking `msg_box` pattern); `lazyrad-view` keeps a shared counter that its `on_progress` hook checks (verified, by a probe, to stop a large `map`, `filter` and `some`; `sort` on a constant receiver errors instead, so it is unexercised); `xui-rhai`'s handler budget compares the per-run number the hook is given and has the blind spot the shared counter closes |
| R6 | **Designer complexity** (tree drag and drop in a custom-painted toolkit) | Outline reorder and click-to-insert first; pure, window-free tree ops; reuse overlay and hit-test patterns |
| R7 | **Work stranded on stacked branches** (it already happened, F2) | P1 now. Process fix: retarget stacked PRs to `main` before merging the bottom one, or merge the stack top-down; a CI or bot check that a merged PR's base is the default branch |
| R8 | **Three repos pinned at different xui revs** | xui's W5.5 single pin; adoption (P4) lands against one rev; the musl CI job gates every PR |
| R9 | **LazyOS regressions** | Forms go through the `desktop` / `Platform` seams already in place; no `cfg(target_arch)` outside seams |
| R10 | **Bandwidth** | At most two agents at once; lanes are independent after A1 and B3; work packages are issue-sized for the existing dispatch workflow; xui-side work follows the established xui process |
| R11 | **Layout patches lose widget state** (F11) | Remount over shared widgets, proven by spike S3 before M8 relies on it; patches only at size-class changes; G27 as the upstream fix |
| R12 | **`xui-script` and LazyRAD's runtime diverge again** (F6) | P2b upstreams the deltas before adoption; one Rhai pin; LazyRAD keeps only its stdlib, `FsPolicy` and player |
| R13 | **Motion drops frames on the software-painted xui canvas**, most on LazyOS and for large animated regions (a full-window fade repaints the window every frame) | Transform and opacity only; animate small subtrees; paint dirty rectangles; measure in spike S4; a frame-budget check that degrades a running animation to its end state under load; reduced motion as the escape hatch |
| R14 | **Desktop and web motion disagree** (curves, timing, stagger origin) | One easing set and one definition of each preset; golden values at fixed times from the virtual clock against a paused Web Animation, with a stated tolerance, because an exponential ease-out is not a cubic-bezier and the web uses a CSS `linear()` approximation; the no-3D lint |

### Decisions I need from you

| # | Decision | Recommendation |
|---|---|---|
| D1 | Web renderer: DOM (B) now, canvas (A) later as a fallback | **Yes**, with the §3.3 revisit trigger |
| D2 | **Land the stranded RON work now (P1)**, before anything else | **Yes**: it is the first implementation task, run in a scratchpad clone, never in `E:\repos\xui` |
| D3 | LazyRAD-only kinds (`Timer`, `Canvas`, `PictureBox`): model upstream as enum variants, or add a generic extension node | **Upstream variants** for the three (they are not LazyRAD-specific). Components get an instance node (§4.6) |
| D4 | Bindings and responsive overrides as **common side-table fields** (§4.2), not field-type wrappers | **Yes**, pending spike S5 |
| D5 | One `.lfm` file type; no `.lvw`. A form with `props` is a component | **Yes** |
| D6 | Native Android and iOS in scope? | **No**; PWA first, revisit at M11 |
| D7 | Should existing classic forms be exportable to the web? | **Yes, selectively**: `Absolute` maps to positioned elements, but only forms whose kinds have a DOM mapping export. Of the four samples only `calculator` is tier 0: `hello` needs `ComboBox` (M9), `todo` needs `ListView` (M11), `brickbreaker` needs `Canvas` (M11) |
| D8 | Top-level `let`s visible in handlers (LazyRAD's `36d2d21`; `xui-script` documents the opposite): upstream LazyRAD's behaviour | **Yes** (P2b). Bindings still see only `state`, `props` and `env` (§4.5) |
| D9 | Motion vocabulary: the 2D subset of Windows Phone 7's Metro motion (§4.8), with the 3D (the turnstile and feather rotation, swivel, flip, tilt depth) excluded, `Roll` and `Rotate` kept because they are 2D, and every effect honouring reduced motion | **Yes** (you asked for it). The preset names are my mapping; rename freely |
| D10 | Does desktop ship the full presets in M8, or may it degrade to translate-only while G29 lands | **Full presets**: motion is first class, so P2d precedes the desktop motion lane. Spike S4 can still force a written degradation |
| D11 | What "turnstile minus the 3D" means: (a) a flat squash, `scale_x` from about 0.17 to 1 anchored at the left edge (the orthographic width of the original's 80° turn: a 2D affine transform, but it still reads as a fold), or (b) a plain slide | **(a)**, with (b) one token away. WP7's turnstile has no slide at all, so (b) is a substitute, not a remainder. If a flat fold still feels like 3D to you, take (b) |

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
| RON mechanics (`struct_names`, `Some(None)`, `LAYOUT_FIELDS`, unnamed layouts, diagnostics) | **Adopted** as S5 work (§4.2) |
| Docs, accessibility acceptance, WebKit, window mapping, `.lrp` work package | **Adopted** (P5, §4.2 row 5, gate, §5.2, work package D6 in lane D) |
| "No IME" is false on Win32; `now()` does not use `web-time`; `d798cd4` is the CLI commit; F5 counts | **Corrected** (F5, F9, §5.2) |
| Unverifiable: wasm compile, size and time budgets, third-party framework columns | **Open**: S1 and the budgets in §5.2 |

### 11.1 What implementing and reviewing taught us (13 October 2026)

**Lane B (`lazyrad-view`, [#119](https://github.com/va1erian/lazyrad/pull/119)).**
Implementing the untested design in §3.4 and §4.5 found eight things, folded into those
sections above:

| Finding | Where it landed |
|---|---|
| A property that only a larger-class patch ever set must be *unset* when the window shrinks, and `Set` cannot express "no value" (a property with a base value just gets a `Set` of it) | §3.1, §3.4, §4.5 |
| Rhai restarts its operation counter inside callbacks, so the "operation budget limits runaways" claim was false there | R5 |
| The two triggers collapse into one `update`; skipping state readers on resize needs state-change detection (a version counter) | §4.5 |
| "Typed property" was undefined: the kind must come from the catalog | §4.5 |
| The cascade is per property, as in CSS | §4.5 |
| A user edit with no handler sticks until the bound value next changes (the session diffs against the last applied value and skips a `Set` equal to the live one) | §4.5 |
| Unique node names conflict with unnamed layouts and control arrays | §4.2 (S5) |
| A `(0..N).map(...)` runaway does not exist in Rhai (ranges have no `map`; an expression cannot loop); the real hole is closures | R5 |

**Independent review of #119 (read-only, 13 October).** The session, cascade and diff
logic held up and the shared operation counter genuinely closes the callback hole. It
also found one Blocker and three Majors in the evaluator, now being fixed:

| Finding | Evidence |
|---|---|
| **Blocker: `sleep()` is reachable**, so the "side-effect-free engine" was not sealed. Registering Rhai's `CorePackage` brings in `sleep`, `exit` and `parse_json`; one call is one operation, so no budget stops `sleep(3600)`, and it panics on `wasm32` | `sleep(1)` took 1000 ms. Fix: register only the arithmetic, string, iterator and function packages and pin the function list with a test |
| **Major: a recursion bomb through a closure aborts the process** in a debug build on a 1 MiB stack; the call-depth cap of 16 is not stack-safe | `(\|f, n\| ... f.call(f, n-1) ...).call(<same>, 20)` gives `0xC00000FD` |
| **Major: a closure that mentions `state` fails when its receiver is a `state` path**: `state.items.filter(\|x\| x > state.n)` reports "Data race detected", which is the headline use case | `[1,2].map(\|x\| x + state.n)` works; the path form does not |
| **Major: size limits are checked only after a native call returns** | A 100k-element array through `map` peaked at +640 MB before returning `Limit` |

The lesson for the web target: "no I/O" is a property of the *package list*, not of the
expression syntax, so the allow-list is a tested contract, and the evaluator needs a
small-stack test because wasm threads and the browser main thread have small stacks too.
Minors (state cloned twice per turn, a cache cleared wholesale at 4096 sources, unbounded tree depth, NaN
widths re-evaluating every turn, a lexical-only platform guard) are on the fix list.

**P1 review (xui [#296](https://github.com/va1erian/xui/pull/296), the landing PR).**
The first independent review of the RON code verified that the landing is faithful (the
RON paths are byte-identical to the stack tip; only `Cargo.toml` and `CHANGELOG.md` were
resolved by hand) and found four Major defects, none a regression against `main`:

| Finding | Evidence |
|---|---|
| `migrate` builds grids that do not land on the v1 pixels (only the left padding is checked; nested `Group` content is narrower than the outer size) | A 144-wide form with two buttons renders them 67 and 53 wide; in a `GroupBox`, 64-wide buttons render 60 |
| `migrate` silently drops nodes and can overflow the stack on invalid v1 (duplicate names, a leaf as parent, a cycle); unchecked `i64` arithmetic | `thread 'main' has overflowed its stack` on two nodes named `x` where the second has `parent = "x"` |
| `Form::to_ron` panics or writes a file `check` rejects at RON's recursion limit (about 20 nested panels); the W4 designer would crash on save | 40 nested v1 panels panic; 20 write an unloadable file |
| `array` and `index` are unbounded and accepted on containers | `array: 100000000` took 2.5 s and 770 MB; three nested `Panel(array: 30)` ran past 150 s |

Minors worth a fix with them: non-finite or huge lengths panic in debug builds;
`schema --json` leaks `$crate::` macro internals into 119 type strings; `ProgressBar`
truncates `i64` to `i32`; `fmt --check` is wrong on CRLF checkouts and rejects a BOM;
`migrate X --out X` overwrites its own input; `check` aborts at the first unreadable
file; `Button(name: "form")` passes `check` and then breaks scripts. P1b (§8) tracks
them.

### 11.2 Requirements added after the first draft

| Requirement | Where it landed |
|---|---|
| **Animations are first class**, in the spirit of Windows Phone 7's effects minus the 3D (the owner, 13 October 2026) | Goal 6 and the 3D non-goal (§1); §3.1 and §3.4 (`Patch::Animate`); §4.2 row 6 and §4.8; §5.1 and §5.4; §6.1 and §6.3; §7; spike S4, P2d and the M8 to M11 milestones (§8); G24, G29, G30; R13, R14; D9, D10; Appendix B |

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
| `Overlay` | `overlay()` centres at natural size; `stack()` stretches | A stacked grid cell stretches | S5 picks one meaning |
| `Scroll` | Vertical only (`scrollview/content.rs`) | Either axis | Vertical only in M8 |
| `Grid` | `Auto` ignores spanning items; `Fill` rows are derived from items | `auto` and `fr` size differently | Emit `minmax(0, Nfr)`, `min-width: 0` and explicit rows |
| `Group` | A frame with content insets | `fieldset` has `min-inline-size: min-content` | Emit a `div` with a title |

**Not in the subset (so not used):** baseline alignment, negative margins, `auto`
margins, intrinsic-size cycles, `position: sticky`, and anything CSS grid can do that
`grid(tracks)` cannot.

## Appendix B — Windows Phone 7 motion, 2D subset

Each effect, what is kept and what is ours. The originals were checked against the
Windows Phone Toolkit source (`microsoftarchive/WindowsPhoneToolkit`); the other-stack
columns are for orientation only and were not verified.

| WP7 effect (Toolkit class) | LazyRAD preset | What the original does | Kept / ours | CSS / Web Animations | Compose | Flutter |
|---|---|---|---|---|---|---|
| `SlideTransition` | `Slide(from, distance)` | translate (about 200) plus opacity, ease-out exponent 6 | kept as is | `transform: translate()` + `opacity` | `slideInVertically` + `fadeIn` | `SlideTransition` + `FadeTransition` |
| (the opacity half of Slide and Rotate) | `Fade` | opacity 0 ↔ 1 | kept | `opacity` | `fadeIn` | `FadeTransition` |
| `RollTransition`, `RotateTransition` | `Roll`, `Rotate` | `RotationZ` about the centre (2D); `Rotate` fades, `Roll` does not | kept as is | `rotate` + `opacity` | `Modifier.rotate` | `RotationTransition` |
| `TurnstileTransition` | `Turnstile` (flat) | `RotationY` -80° to 0 about the left edge, 350 ms in and 250 ms out; **no slide and no fade** | **ours:** `scale_x` from about 0.17 to 1 at the left edge (a 2D squash). `Slide` is the alternative (D11) | `transform: scaleX()` with `transform-origin: left` | `Modifier.graphicsLayer { scaleX }` | `ScaleTransition` with `alignment` |
| `TurnstileFeatherEffect` | `Feather(stagger)` | per-item `RotationY` about `CenterOfRotationX = -0.2`, 350 ms, staggered 40 ms in and 50 ms out | the **stagger** kept; the per-item motion is the flat reading | `animation-delay: calc(var(--i) * 50ms)` with `fill-mode: backwards` | `AnimatedVisibility` with a delay per item | staggered `AnimatedList` |
| `TiltEffect` | `PressScale` | tilt of up to 0.3 rad plus up to 25 px of Z depression; release over 100 ms after a 200 ms delay | **ours:** scale to about 0.96; release timing kept | `:active { transform: scale(.96) }` | `Modifier.scale` on press | `ScaleTransition` |
| `PerformanceProgressBar` | `ProgressDots` | indeterminate dots | kept | CSS keyframes | custom | custom |
| the platform's Continuum (not in the Toolkit) | `Continuum(key)` | not verified here | **ours** | View Transitions API (`view-transition-name`) | `SharedTransitionLayout` | `Hero` |
| the platform's Pivot and Panorama (not in the Toolkit; only `LockablePivot`) | `Pivot` / `Panorama` slide | not verified here | **ours** | scroll-snap + parallax transform | `HorizontalPager` | `PageView` |
| `SwivelTransition`, tile flip | *excluded* | `RotationX` (3D) | all of it | n/a | n/a | n/a |

**Timing.** The Toolkit uses `ExponentialEase` (ease-out, exponent 6) throughout, with
durations of 250 to 350 ms for turnstile, 500 ms in and 300 ms out for slide left and right and 600 ms for
roll. The tokens `motion.decelerate`, `motion.accelerate` and `motion.standard` carry the
curves; `motion.fast`, `motion.normal` and `motion.slow` carry the durations and
`motion.stagger` the 40 to 50 ms step. An exponential ease-out is not a cubic-bezier, so
the web uses a CSS `linear()` approximation and the fixtures carry a tolerance.

**Unverified.** The Continuum and Pivot behaviours, and the 0.96 scale (a LazyRAD token,
not a WP7 value).
