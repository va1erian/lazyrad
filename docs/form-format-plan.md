# LazyRAD — the new form format, and a complete palette

**Status: draft, 22 October 2026.** Replaces the M8–M11 "modern UI and web" plan, which is
parked in [deferred-modern-ui-and-web.md](deferred-modern-ui-and-web.md). Companion to
[PLAN.md](../PLAN.md). "Verified" means read from source or GitHub on 22 October;
"to verify" names the check that settles it.

LazyRAD stays a VB6-style RAD tool: absolute-positioned controls, a Rhai handler per event,
an `.exe` or `.lzp` out. This stage changes two things underneath and one thing in the UI:

1. **Move to xui's `.lfm` format 2** (a RON tree) and drop LazyRAD's own flat-TOML copy of
   `xui-form`.
2. **Put every xui widget the form model has into the designer's palette.**

## 1. Scope

| In | Out (parked, see the deferred plan) |
|---|---|
| Format 2 adoption: one xui pin, LazyRAD's `xui-form` and `xui-rhai` copies deleted, designer / runtime / IDE / project ported, samples and templates migrated | Web target, DOM renderer, wasm, `lazyrad-web`, *Make Web App* |
| Modelling `Timer`, `Canvas`, `PictureBox` and `Button.icon` in the format | Responsive layouts, size classes, `responsive` patches |
| A path-addressed tree API for the designer | `bind` and state bindings (xui#306), the `lazyrad-view` crate as a dependency |
| The palette: every form-model widget, then container and further widgets | Motion and animation (P2d-c, `MotionDriver`, `animate()`), "modern" components and theming, flow-layout designer features |
| Docs and samples for the new format | Router, http, PWA, components, lists, conditionals |

`crates/lazyrad-view` (merged in #119 and #122) is **parked**: it has no consumer and costs
nothing to keep. Nothing in this plan depends on it, and nothing in it should be extended
until the web or modern-UI work is picked up again.

## 2. Where things stand (verified, 22 October)

Merged since the old plan was written, none of it reflected in PLAN.md:

| What | Where | Effect on this plan |
|---|---|---|
| **P1: format 2 and the `xui-form` CLI are on xui `main`** | xui#296 | Done. The CLI gives `check`, `render`, `fmt`, `migrate`, `schema --json` |
| **P1b: hardening** (grid fidelity, invalid v1, one recursion limit, `array`/`index` caps, minors) | xui#305, closes #297–#301 | Done. Adoption needs the breaking changes below |
| Easing set, `Tween`, `Clock`, reduced-motion setting | xui#304 | Not needed now. Stays in xui |
| Canvas repaints only the dirty region | xui#308 | Not needed now; a free speed-up for the designer |
| `aspect` layout field, **typed property surface** (`Properties::schema()`: enum, colour, font, category, read-only, description), `Label.font`, `XUI_LAYOUT_REPORT` | xui#309 | The typed surface is what PLAN.md calls G6; `Label.font` is G17's first step. The designer's property grid can read it instead of its own schema |
| `lazyrad-view` (size classes, bindings, patches, motion planner) | lazyrad#119, #122 | Parked |

xui `main` is `71dc3b5` (#310). LazyRAD pins `93592fd`, so the pin is **19 commits behind**.
Still open on xui: **#306** (`bind`, `responsive`, `motion` as common fields) is the
dropped scope. It is the owner's call whether to close it or leave it for later.

**What format 2 gives LazyRAD today** (xui `docs/forms.md`): layouts `Row`, `Column`, `Wrap`,
`Grid`, `Absolute`; containers `Panel`, `Group`, `Tabs`; widgets `Label`, `Button`,
`Hyperlink`, `CheckBox`, `ToggleButton`, `Edit`, `MultilineEdit`, `NumberField`, `Slider`,
`ProgressBar`, `RadioGroup`, `ComboBox`, `ListView`, `Separator`; control arrays;
byte-stable output; located errors with "did you mean".

**Breaking changes adoption must absorb** (xui#305, #309): `Form::to_ron()` returns
`Result`; a tree nests at most `MAX_DEPTH = 16` levels, so a v1 form migrates only up to 7
nested containers; `array` ≤ 1024; `form` is a reserved control name; `check` is stricter
(`array: 0`, `min > max`, `selected` out of range); `Split.position` is a `Dip`.

**What LazyRAD has that format 2 lacks** (verified): `Timer`, `Canvas`, `PictureBox`, and
`Button.icon`. They are 3,000-odd lines of factories in LazyRAD's own `xui-form` (`canvas.rs`
1,214, `picture/` 1,789, `xui_factories.rs` 1,133 for all kinds).

**The palette today** is 11 tiles (`CONTROL_KINDS` in
[toolbox.rs](../crates/lazyrad-designer/src/toolbox.rs)): `Button`, `Edit`, `Label`,
`CheckBox`, `RadioGroup`, `GroupBox`, `ListView`, `ComboBox`, `Canvas`, `PictureBox`,
`Timer`. LazyRAD's catalog has 19 kinds, and factories exist for all of them, so **eight
are buildable but not offered**: `Hyperlink`, `MultilineEdit`, `NumberField`, `Panel`,
`ProgressBar`, `Separator`, `Slider`, `ToggleButton`.

## 3. Work

Two tracks. The palette track does not wait for format 2.

### Track A — the palette

| # | Work | Needs | Size |
|---|---|---|---|
| A1 | **Offer the eight missing kinds** on today's model: add them to `CONTROL_KINDS`, a Lucide icon each (`Link`, `FileText`, … from the table the designer already has), default sizes, a designer test that creates and saves each. Add a guard test that **every catalog kind is in the toolbox** unless listed as deliberately hidden, so the palette cannot drift again. The tile grid is 28 dp per row: 20 tiles in two columns is about 300 dp, so check the pane fits or scrolls at the IDE's default size | nothing | days |
| A2 | **`Tabs`**, the one container the format has and the palette lacks. Needs the P3 tree (a page is a child level) | B3, B4 | ≈ 1 wk |
| A3 | **Further xui widgets, chosen by you** (decision D2). Candidates not in the format yet: `TreeView`, `ColorPicker`, `IconView`/`GridView`, `Split`, `ScrollView`, `StatusBar`. Each is: a model variant and schema upstream, a factory, Rhai property and event surface, a palette tile, a sample or test. The typed property surface (xui#309) means no per-kind editor code | B2 for the pattern | ≈ 3–5 days per widget |

### Track B — format 2

Order is by dependency. Sizes assume at most two agents at once (the dispatch workflow's
cap) and are rough.

| # | Work | Repo | Size | After |
|---|---|---|---|---|
| B1 | **One xui pin.** Move every LazyRAD pin to a current `main` rev (the lazyOS repo's `tools/xui/check_pin.py` checks pins; LazyOS's `xui-app` and `lazyrad-os` pins must move with it). Fix the fall-out: `Split.position`, anything the 19 PRs changed. Keep LazyRAD's own `xui-form` copy for now | LazyRAD (+ lazyos) | days | — |
| B2 | **Model `Timer`, `Canvas`, `PictureBox` and `Button.icon` in xui's format.** Only the **model variants and schema** go upstream (small). The factories (the 3,000 lines) **stay in LazyRAD** and are registered into `Factories`. To verify: that a consumer-registered `WidgetFactory` can serve a kind that lives in xui's model (xui `docs/forms.md` says kinds belong in the model, factories register: so it should). If not, move the factories upstream with the variants | xui, then LazyRAD | 1–2 wk (3 wk if the factories must move) | B1 |
| B2b | **Upstream LazyRAD's `xui-rhai` deltas** into `xui-script`: `build_deferred`, `reload`, `state`/`set_state`, `StopHandle`, `is_host_handle`, `control_methods`, `to_arg_value`, `source_name`, and the top-level-`let` visibility of `36d2d21`. Needed for hot reload and the debugger. To verify first: the list was read on 13 October; xui#304–#310 may have changed some | xui | ≈ 1 wk | B1 |
| B3 | **Tree API in `xui-form`:** a path-addressed `NodePath` with `insert_at`, `move_to`, `remove`, `rename`, `wrap`, `unwrap`, each returning its inverse for the designer's `History`. Scoped to what the designer uses: wrap and unwrap can wait for A2 if they add risk | xui | 1–2 wk | B1 |
| B4 | **Adopt.** Delete LazyRAD's `xui-form` and `xui-rhai`; port the `FormDoc` sites (about 80 reference lines across designer 29, runtime 27, IDE 12, project 11, rhai 3, plus about 70 in tests and examples); designer onto the B3 tree, keeping every VB6 interaction on `Absolute` containers; `xui-form migrate` the four samples and the IDE templates; keep hot reload, handler rename and the debugger working | LazyRAD | 3–4 wk | B2, B2b, B3 |
| B5 | **Docs:** PLAN.md §1 (TOML), §1.1 (`GroupBox` becomes `Group`), §2, README, the samples, the tutorial | LazyRAD | days | B4 |

**What the designer does with other layouts.** The designer places controls on `Absolute`
containers, exactly as it does now, nested inside `Group`, `Panel` and `Tabs`. A form that
uses `Row`, `Column`, `Wrap` or `Grid` (hand-written, or from a migrated form where the
migrator chose a grid) **opens, renders, runs and has its properties edited**, but you move
those controls by editing the text. Flow-layout drag and drop (drop indicators, quick
actions) is out of this stage. (Decision D3.)

**Gate for track B:**

- the four samples and the IDE templates run, debug and export from format-2 forms;
- hot reload and the debugger work on format-2 forms;
- the designer passes today's designer tests re-based on the tree, plus a round trip
  through `xui-form fmt --check`;
- `Timer`, `Canvas` and `PictureBox` forms (brick breaker, the picture viewer) behave as before;
- the musl CI job is green, and LazyOS builds against the same xui rev.

## 4. Order

```text
A1 (palette, 8 kinds) ───────────────────────────────────────────────►  ships first, alone
B1 pin ─┬─ B2  Timer/Canvas/PictureBox/icon ─┐
        ├─ B2b xui-script deltas ────────────┼─► B4 adopt ─► B5 docs ─► A2 Tabs ─► A3 widgets
        └─ B3  tree API ─────────────────────┘
```

B2, B2b and B3 are independent after B1, so two agents can run two of them at once. A1 is
small enough to run beside any of them. Whole of track B: about **6–8 weeks** wall-clock
with two agents (a rough estimate), against 10 to 12 for the old Phase P, because motion (P2d-c), the wasm
factoring (P2c) and the format additions (S5) are gone.

## 5. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Depth limit.** `MAX_DEPTH = 16`, and a migrated v1 form fits only 7 nested containers | The four samples are small (19 to 159 lines of TOML) but their depth is to verify; B4's gate migrates every sample and template; if the designer needs more, xui's PR #305 records that 24–32 is possible after cutting per-level stack cost |
| R2 | **A closed model.** Every new kind needs an xui model variant (no `register`/`alias`) | Variants are small and go through xui's normal process; factories stay with LazyRAD. This is why A3 is priced per widget |
| R3 | **Work stranded on stacked branches** (it already happened to #275/#276) | Retarget stacked PRs to `main` before merging the bottom one; check the base branch of every merged PR |
| R4 | **Pins drift again** (LazyRAD, LazyOS and xui at three revs) | B1 moves all of them and uses `check_pin.py`; one rev per PR |
| R5 | **Adoption touches the designer, runtime, IDE and project crates at once** | B4 is split by crate into issue-sized packages; the old crate stays buildable until the last site moves |
| R6 | **`xui-script` and LazyRAD's runtime diverge again** | B2b upstreams the deltas before B4; one Rhai pin |

## 6. Decisions I need from you

| # | Decision | Recommendation |
|---|---|---|
| D1 | Is `lazyrad-view` kept parked in the workspace, or removed? | **Keep, parked.** It is merged, tested, and removing it throws away the binding sandbox work. It is not wired into anything |
| D2 | Which further xui widgets after `Tabs` (A3)? | **`TreeView` and `ScrollView`** first (a tree and a scrollable region are the two most missed in VB6 terms); the rest on demand. Tell me if you meant a different set |
| D3 | The designer places controls only on `Absolute` containers in this stage; other layouts are edit-in-text | **Yes.** Flow-layout drag and drop was most of the old designer work |
| D4 | Keep factories in LazyRAD and upstream only the model variants for `Timer`, `Canvas`, `PictureBox` (B2)? | **Yes**, pending the factory-registration check. It cuts about 3,000 lines from the upstream work |
| D5 | xui#306 (`bind`/`responsive`/`motion` fields): close, or leave open for the parked plan? | **Leave open, relabel as parked.** Closing loses the discussion; leaving it open costs nothing |
