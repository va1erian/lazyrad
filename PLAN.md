# LazyRAD — Plan

LazyRAD is a RAD IDE in the spirit of Visual Basic 6. It is written in Rust, uses the
[xui](https://github.com/va1erian/xui) toolkit for its UI, and uses
[Rhai](https://rhai.rs) as its scripting language. You draw a form, double-click a
button, write the handler, press F5, step through it in the debugger, then export a
single `.exe`. [LazyOS](https://github.com/va1erian/lazyos) is a later target.

This document covers the architecture, the key decisions, a milestone plan, and a list
of the **xui gaps** LazyRAD will run into, along with a proposed fix for each.

---

## 1. Guiding decisions

| Decision | Choice | Why |
|---|---|---|
| UI toolkit | `xui-core` plus the `xui-canvas` (tiny-skia) backend, with the Win32/D2D backend optional | Canvas is the backend LazyOS already runs (`lazyos/xui-app`). Developing on it from day one means the port is mostly packaging. |
| Language | Rhai, with the `debugging`, `metadata` and `internals` features, **not** `sync` | `debugging` provides breakpoints, stepping and call stacks. `metadata` gives completion and signature data for the stdlib. `internals` exposes the tokenizer for highlighting. Leaving out `sync` keeps `Rc`, which fits xui's single-threaded widgets. |
| Code editor | **A custom xui widget**, not Scintilla (see §5) | It stays pure Rust, with no C++ toolchain on musl or LazyOS, and it can be tailored to Rhai. |
| Running the user program | **A separate process**: the same "player" binary that export uses, launched with `--debug` | The IDE stays responsive while the program is paused or stuck in a loop. What you debug is exactly what you ship, and a crash never takes down the IDE. |
| Executable format | A prebuilt player stub with the project appended as a payload | It needs no compiler or linker on the user's machine. It works for PE (as an overlay) and ELF alike, including musl builds for LazyOS. |
| File formats | Plain text: TOML for the project and forms, `.rhai` for code | It diffs well, merges well, and can be edited by hand, like VB6's `.vbp`/`.frm`. |

---

## 2. Workspace layout

```text
lazyrad/
├─ Cargo.toml                 (workspace)
├─ crates/
│  ├─ lazyrad-project/        project + form file model (serde), load/save, validation
│  ├─ lazyrad-runtime/        Rhai engine setup, stdlib, form instantiation, event binding
│  ├─ lazyrad-debug-proto/    IDE <-> player debug protocol (JSON lines over stdio)
│  ├─ lazyrad-player/  (bin)  the runtime host: runs a project dir, a payload, or --debug
│  ├─ lazyrad-editor/         xui code-editor widget (buffer, view, highlight, completion)
│  ├─ lazyrad-designer/       xui form-designer surface, toolbox, property grid
│  ├─ lazyrad-packager/       exe export: stub + payload, icon/metadata where possible
│  └─ lazyrad-ide/     (bin)  the IDE shell: windows, menus, project explorer, wiring
└─ examples/                  sample projects (hello, calculator, notepad, todo list)
```

`lazyrad-runtime` is the core. The player uses it to run programs, and the IDE uses it
too: for the designer's live preview, for completion metadata, and for syntax checks.
`lazyrad-runtime` never depends on IDE crates.

Pin xui to a git `rev`, the same way `lazyos/xui-app` does. Keep a `[patch]` path
available so xui fixes can be made in a local checkout and upstreamed.

---

## 3. Project model

```text
MyApp/
├─ MyApp.lrp          # project: name, version, startup form/module, icon, references
├─ frmMain.lfm        # form layout (TOML) — controls, properties, menu tree
├─ frmMain.rhai       # form code-behind: event handlers + form-level functions
├─ modUtil.rhai       # standard module: shared functions, exported as a Rhai module
└─ clsStack.rhai      # class module (see §4.3)
```

A form file looks like this:

```toml
[form]
name = "frmMain"
caption = "Hello"
width = 320
height = 200

[[control]]
type = "CommandButton"
name = "cmdHello"
left = 16
top = 16
width = 120
height = 32
caption = "Say hello"
tab_index = 0
```

**Event binding** follows VB naming conventions. The runtime looks for
`fn <control>_<event>(args…)` in the form's script (`cmdHello_Click`, `Form_Load`,
`txtName_Change`) and connects each one to the matching xui `on_*` closure. When a
function is missing, the event is not wired.

---

## 4. Runtime and language

### 4.1 How scripts see controls (a Rhai gotcha)

Rhai functions are pure: **they cannot see variables from the enclosing scope**, so
`txtName.text = "x"` inside `fn cmdHello_Click()` would normally fail. LazyRAD gets
around this with an `Engine::on_var` resolver. When a name is not found, the resolver
looks it up among the active form's controls, then the form itself (`Me`), then
globals such as `App`, `Screen` and `Clipboard`. Controls are registered custom types
that wrap an `Rc` handle, so a property setter changes the live widget:

```rhai
fn cmdHello_Click() {
    lblOut.caption = `Hello, ${txtName.text}!`;
    Me.caption = "Greeted";
}
```

Form-level state that outlives a single event lives in `Me.state`, an object map. The
alternative is module-level `const` values plus a registered `Static` store.

### 4.2 Execution model

- The player owns the xui event loop. Each widget event is turned into a
  `Msg::Event { form, control, event, args }`. `App::update` then calls the matching
  Rhai function through `Engine::call_fn` on the form's compiled `AST`.
- `Engine::on_progress` implements Ctrl+Break, an operation budget per event (to catch
  runaway loops), and debugger pause checks.
- `Engine::set_max_*` limits apply by default, but are relaxed for exported apps.
- `MsgBox` and `InputBox` use xui `Dialog`. A VB-style blocking `MsgBox` would need
  `Ui::open_modal`, which **does not work on canvas today** (G14). So in Iteration 1,
  `MsgBox` is non-blocking and takes an optional callback. It becomes blocking once
  canvas implements `run_modal`.

### 4.3 Object orientation

Rhai has no `class` keyword. LazyRAD provides objects through two mechanisms:

1. **Native classes (the stdlib).** These are Rust types registered with
   `register_type_with_name`, getters, setters and methods. They are true types with
   `type_of()`, `is`-style checks and `to_string`.
2. **Script classes (class modules).** A `.rhai` class file exports `fn new(...)`, which
   returns an object map. Its methods are `Fn` pointers called with `this` bound, which
   Rhai supports natively. The IDE's class-module template produces this shape:

   ```rhai
   // clsStack.rhai
   fn new() {
       #{ items: [], push: |x| this.items.push(x), pop: || this.items.pop(),
          count: || this.items.len(), __class: "clsStack" }
   }
   ```

   The editor's completion recognises `__class` and offers the map's members.
   Inheritance works by composition: `let o = clsBase::new(); o.extra = …; o`.

### 4.4 Standard library: small but usable

The stdlib is grouped as Rhai static modules and written in Rust. Every item gets doc
comments, which the `metadata` feature surfaces in completion and tooltips.

| Module | Contents |
|---|---|
| Core | `Str` helpers (`Left`/`Mid`/`Right`/`Trim`/`Split`/`Join`/`Format`/`Val`), `Math`, `Rnd`, conversions |
| Collections | `List` (Rhai array plus helpers), `Dictionary`, `Set`, `Queue`, `Stack`, `sort_by` |
| Time | `DateTime` (`Now`, `Date`, formatting, arithmetic), `Timer` control, `Stopwatch` |
| IO | `File` (`read_text`, `write_text`, `append`, `lines`, `exists`, `delete`), `Dir`, `Path`, `TextReader`/`TextWriter` classes |
| App | `App` (`path`, `title`, `version`, `args`, `quit`), `Environ`, `Clipboard` (text only), `Screen` |
| UI | `Form` (`show`/`hide`/`show_modal`/`unload`, `caption`, `left`/`top`/…), `MsgBox`, `InputBox`, `CommonDialog` (open/save/color) |
| Controls | `Label`, `TextBox` (single and multi-line), `CommandButton`, `CheckBox`, `OptionButton` (a `RadioGroup` item), `Frame`, `ListBox`, `ComboBox`, `HScrollBar`/`VScrollBar` (`Slider`), `ProgressBar`, `Timer`, `PictureBox`/`Image`, `TreeView`, `ListView`, `TabStrip`, `Menu` |
| Data (later) | `Json` (parse/stringify to maps), `Csv`, `Ini`/`Settings` (VB `GetSetting`/`SaveSetting`) |

Exported apps must not be surprising, so filesystem access is allowed by default. The
IDE's run configuration can sandbox it.

---

## 5. Code editor: Scintilla or custom?

**Porting Scintilla to xui is feasible but not worth it.** Here is what it would take:

- Implement Scintilla's `Platform.h` layer: `Surface` (about 40 drawing and text calls),
  `Font`, `Window`, `ListBox` (the autocomplete list), `Menu`, `ElapsedTime` and
  `DynamicLibrary`. Most of `Surface` maps onto `xui_core::backend::Canvas`. The two
  exceptions are `MeasureWidths`, which needs per-character positions (see gap G7), and
  font handling.
- Subclass `ScintillaBase` to deliver keys, mouse, timers, clipboard and IME. Clipboard
  and IME are both xui gaps (G2, G3).
- Build C++17 through `cc` for every target, **including static musl C++ for LazyOS**.
  That adds a libstdc++/libc++ dependency LazyOS does not have today, and it brings
  `unsafe` FFI into a codebase whose toolkit forbids `unsafe`.
- Lexilla has no Rhai lexer. The C++ lexer would get close, but not all the way.

The work is about the same as a focused custom editor, and a custom editor gets better
Rhai integration (AST-aware completion, inline diagnostics, breakpoint gutter) with
nothing extra to port to LazyOS. **Recommendation: build a custom editor, with an
optional one-week Scintilla spike only if the custom editor stalls.**

### Custom editor design (`lazyrad-editor`)

- **Buffer:** `ropey` rope, a line index, and a transactional undo/redo stack that
  coalesces typing runs.
- **Rendering:** a single `NodeKind::Custom` node with a `Painter`. Use a **monospace
  grid**: measure the advance once per font and DPI, then draw each token run with
  `Canvas::draw_text` at `col * advance`. This keeps the path fast and simple, and hit
  testing reduces to arithmetic. Only visible lines are painted. The gutter shows line
  numbers, a breakpoint dot, the current-line arrow and folding markers (folding comes
  later).
- **Highlighting:** incremental, per line, using a small hand-written Rhai lexer. It
  needs a state for block comments and multi-line strings, and uses the same token
  classes as Rhai's own tokenizer. Rhai's tokenizer (through `internals`) serves as a
  test oracle. Colours come from theme tokens, with dark mode for free.
- **Diagnostics:** after a debounce, `Engine::compile` runs on a worker thread
  (`Ui::proxy`). Parse errors are drawn as squiggles and listed in the error list.
- **Completion:** a popup node listing:
  - stdlib items from `Engine::gen_fn_metadata_to_json`
  - script functions from `AST::iter_functions`
  - control names from the form model, and after `ctl.`, that control type's
    properties and methods
  - `__class` maps

  Signature help comes from the same metadata. Keywords and snippets such as
  `fn … { }` and `for x in … { }` are included too.
- **Editing features:** caret and selection (shift and mouse), word navigation,
  auto-indent, bracket matching, find/replace (with regex through `regex`), go to line,
  go to definition (within the project), toggling comments, and tabs or spaces.
- **Testing:** the buffer, lexer and view logic are pure and unit-tested. Rendering is
  snapshot-tested with xui's `OffscreenBackend`.

---

## 6. Form designer (`lazyrad-designer`)

xui already has two useful hooks. **`Ui::set_design_mode`** makes widgets ignore their
own input, and the **`Properties`** trait provides a generic property surface. The
designer uses both. Only the portable versions remain; see the note under §10.

- **Surface:** the form is rendered with **real xui widgets** in design mode, inside a
  `Panel`, so what you see matches what you get. A transparent `Custom` overlay node on
  top draws the dot grid, the selection rectangle, eight resize handles, alignment
  guides and the rubber-band marquee. The overlay handles all mouse input: hit-testing
  against the model's rectangles, capture for drags, and `set_cursor` for handles.
- **Model-first:** the designer edits the `lazyrad-project` form model and re-applies
  it to the widgets through `apply_moves` and `set_property`. Undo and redo operate on
  the model.
- **Toolbox:** a grid of control types. You either click a type and draw it, or
  double-click to drop it at a default size. It needs icon buttons (gap G9).
- **Property grid:** a two-column list of name and value with editors per type: text,
  number, bool, enum dropdown, colour, font, and a `…` button for dialogs. It is a new
  widget (gap G5). A `Properties` extension supplies each control's schema (gap G6).
  An object combo above the grid selects the control.
- **Double-click on a control:** this opens the code window at
  `fn <name>_<default event>()`, creating the function if it does not exist, exactly
  as VB does. The procedure combos at the top of the code window list the control's
  events.
- **Other tools:** a menu editor dialog, a tab-order mode, align/size/centre commands,
  snap-to-grid, copy and paste of controls (in-app first; the OS clipboard is gap G2),
  and control arrays (later).

---

## 7. Debugger

The debugger is built on Rhai's `debugging` feature, running in the player process.

```text
IDE  ──spawn── lazyrad-player --debug <project dir>
 │   stdin : {"cmd":"setBreakpoints","file":"frmMain.rhai","lines":[12,30]}
 │           {"cmd":"continue"|"stepInto"|"stepOver"|"stepOut"|"pause"|"stop"}
 │           {"cmd":"evaluate","expr":"txtName.text","frame":0}
 │   stdout: {"event":"stopped","reason":"breakpoint","file":…,"line":12}
 │           {"event":"output","text":"…"}   {"event":"error", …}  {"event":"exited"}
```

- The player registers a debugger callback. When the program stops, it serialises the
  call stack (`Debugger::call_stack`), the locals of the selected frame (from `Scope`)
  and `Me`'s controls. It then **blocks, reading stdin**, until it receives a resume
  command. The paused app freezes, just as it did in VB6. The IDE stays responsive
  because it is a separate process.
- IDE panes:
  - **Immediate window:** runs `evaluate` against the paused frame, or in run mode
    against the global context. `Debug.Print` output lands here.
  - **Locals**
  - **Watches**
  - **Call stack**
  - **Breakpoints list**
  - a current-line highlight in the editor
- **Break on error:** script errors stop the program at the failing position instead
  of exiting.
- **Pause and break all:** these work through the `on_progress` flag and the debugger
  callback.
- **Edit-and-continue:** not planned. Reloading a changed handler function between
  events is a possible later stretch.
- Since the protocol is plain JSON over stdio, a LazyOS build could replace pipes with
  Messenger later.

---

## 8. Exporting self-contained executables

1. **Build:** `lazyrad-player` is built once per target, in release mode (LTO,
   `panic = "abort"`), for:
   - `x86_64-pc-windows-msvc`
   - `x86_64-unknown-linux-gnu`
   - `x86_64-unknown-linux-musl`, which is also the LazyOS target
2. **Pack:** the export step copies the stub and appends a payload with this layout:
   `[zstd(tar(project files + assets))][u64 len][b"LAZYRAD1"]`. Scripts are shipped as
   source, since Rhai's `AST` is not serialisable. They can be minified with
   `Engine::compact_script` and are validated by a compile check at export time.
3. **Run:** at startup the player reads `std::env::current_exe()` and checks the
   trailer. If a payload is present, it loads it into memory and runs the startup form.
   If not, it expects a project path or `--debug`.
4. **Windows details:**
   - Set the icon and version info by patching resources in the copied stub (the
     `editpe` crate or similar).
   - Build the stub with the GUI subsystem so no console appears.
   - Code signing is out of scope, but note in the docs that an appended overlay
     invalidates any existing signature.
5. **Where stubs live:** the IDE ships with its stubs in `stubs/<target>/`. Picking
   another target in *File → Make EXE* is how cross-export works.

---

## 9. IDE shell (`lazyrad-ide`)

The layout follows VB6: a menu bar and toolbar at the top, the toolbox on the left, a
document area in the centre, project explorer and properties on the right, and
immediate/locals/watch panes at the bottom.

- **Layout:** built from xui `Dock` arithmetic, nested `Split`s and `Tabs`. MDI is
  replaced by tabbed documents, with each form offering a designer and a code view.
  Floating or dockable tool windows are a later goal (gap G8).
- **Project explorer:** a `TreeView` of Forms, Modules and Classes, with add, remove,
  rename and "set as startup".
- **Commands:** a central `Command` enum powers the menu, the toolbar, keyboard
  shortcuts (gap G10) and a command palette.
- **Output and error list:** these show compile diagnostics for the whole project.
- **Settings:** theme (light/dark/system), editor font, tab size, and recent projects.
  They are stored in `%APPDATA%`/`~/.config` via `directories`.
- **Templates:** New Project offers Standard EXE and Empty, and provides templates for
  a form, module and class.

---

## 10. xui gaps

These gaps were found by reading xui at `main` (Sept 2026). "Workaround" means
LazyRAD can proceed without an xui change. "Upstream" means the change should be
contributed to xui.

| # | Gap | Impact on LazyRAD | Proposal |
|---|---|---|---|
| G1 | **No first-class custom widget story.** `Control` + `NodeKind::Custom` + `set_painter` + `on_events` are public, but popup and scrollbar painters (`widget::popup`, `widget::scrollbar`) are private | The editor, designer overlay and property grid need scrollbars and popups that match the theme | Upstream: export `ScrollBar` as a widget and a `Popup` helper. Until then, copy the logic |
| G2 | **No portable clipboard.** It exists only in `xui-win32`, not in `xui-core` or `xui-canvas` | Copy and paste in the code editor and the designer, plus the stdlib `Clipboard` | Upstream: add `Backend::clipboard_get_text`/`set_text` with a default of `Unsupported`. Implement it with `arboard` in canvas and with `clipboardd` on LazyOS. Workaround: an in-process clipboard |
| G3 | **No IME or composition events.** `Event` has `Char` and `KeyDown` only | Painted text editors can't enter CJK text. Low priority for v1 | Upstream: add `Event::Ime{Preedit,Commit}` (winit already provides it) and `set_ime_cursor_area` |
| G4 | **No file, folder or colour dialogs** | Open/save project, Make EXE, `CommonDialog` | Workaround: use `rfd` on desktop. LazyOS needs a painted xui file dialog, which can be shared upstream |
| G5 | **No property-grid or editable list.** `ListView` has no in-place cell editing | Properties window | Build it in LazyRAD on `Custom` plus overlay `Edit`/`ComboBox`/`ColorPicker`. It could move upstream later |
| G6 | **`Properties::Value` is limited to Bool, Integer, Float and Text.** It has no enum, colour, font, rectangle or image, and no property schema covering read-only, category, enum values or description | The designer needs typed editors and "Categorized" vs "Alphabetic" views | Upstream: add `Value::{Color, Enum, Font, Image}` and `fn schema() -> Vec<PropertyInfo>`. Workaround: LazyRAD's own schema per control type in `lazyrad-runtime` |
| G7 | **The text layout API has no caret-from-offset query**, only `hit_test_point` and `selection_rects`. Each layout also has a single style, with no styled runs | The editor needs caret placement and multi-colour lines | Workaround: a monospace grid (§5). Upstream: add `caret_rect(byte)` and an attributed layout |
| G8 | **No docking or floating tool-window framework.** `layout::Dock` is arithmetic only, and there is no MDI | VB-style dockable panes | v1 uses a fixed layout with `Split`/`Tabs` and toggleable panes. Later, build a dock manager (possibly upstream) on `open_window` |
| G9 | **`Toolbar` has text labels only**, with no icons, toggle groups or tooltips per item. `TopBar` has vector `Glyph`s but a fixed glyph set | The toolbox and main toolbar need icons | Upstream: `Toolbar` items with `RowIcon` (glyph or `Image`). Workaround: a toolbox grid as a `Custom` node, or `GridView` with icons |
| G10 | **No portable accelerator or shortcut table.** `Event::Accelerator(u16)` exists, but core has no API to register key bindings | Ctrl+S, F5, F8, F9 and similar | Workaround: handle `KeyDown` at window level in LazyRAD's command dispatcher. Upstream: `Ui::set_accelerators(&[(KeyChord, Msg)])` |
| G11 | **Menu item shortcut text and dynamic menus are unverified.** It is unclear whether menus can display "Ctrl+S" and be rebuilt at runtime (for recent files, and for user programs' menus built from the form's menu tree) | IDE menus and the runtime `Menu` control | Spike in M0. Upstream if it's missing |
| G12 | **No drag and drop of OS files** | Dropping a `.lrp` file onto the IDE. Nice to have | Upstream later, via winit `DroppedFile` |
| G13 | **No image or picture widget and no `Image` loading helper** in the catalogue (only `draw_image`) | `PictureBox`, `Image`, form icons, toolbox icons | Build a small `Picture` widget on `Custom` + `draw_image`, and decode with the `image` crate (PNG/BMP/JPG) |
| G14 | **Modal windows don't work on canvas** (confirmed: xui#146, tracked in emusic#417). `run_modal` returns `Unsupported`, so `Ui::open_modal` closes the child window immediately | Blocking `MsgBox`/`InputBox` and `Form.show_modal` | Iteration 1: `MsgBox` is an in-window `Dialog` and is **non-blocking** (with an optional callback). Upstream: implement `run_modal`/`set_window_enabled` on `WinitBackend` (a nested loop), then make `MsgBox` blocking |
| G15 | **`xui-canvas` hard-depends on `winit`, `softbuffer` and `glutin`.** LazyOS vendors a patched copy (`set_default_font`, `Surface::pixels`) | The LazyOS build of the player and IDE | Track the `xui-skia` split proposed in `lazyos/docs/xui-plan.md`. LazyRAD adds nothing new beyond it |
| G16 | **Single window per task on LazyOS** (see `lazyos/docs/xui-plan.md`) | Multi-form apps and IDE secondary windows | On LazyOS, show secondary forms as in-window `Dialog`-style surfaces until multi-window lands |
| G17 | **No per-control font, colour or back-colour overrides.** Widgets draw only from theme tokens | VB users expect `ForeColor`, `BackColor` and `Font` properties | Upstream: optional per-node style overrides. For v1, support `Font.Size`/`Bold` on `Label` only and leave colours theme-driven (a deliberate trade-off) |

**G11** is worth a one-day spike before M0 ends. **G14** is confirmed and shapes the `MsgBox` API. The rest have workarounds and are not blockers.

**Designer seam status.** The Win32 clean-up (xui#173, tag `pre-win32ui-controls-removal`) deleted the *native-layer* `Properties` impls in `xui-win32/src/properties.rs`. The portable seam survives in `xui-core`: `Ui::set_design_mode`, which about 25 widgets honour by ignoring input, and `Properties` impls on about 25 widgets. Those impls report only a few names (`text`, `enabled`, `checked`, `value`, `selected`…), with no bounds, font or schema. Per xui#16, selection and handle painting, serialisation and schemas belong to the RAD app, not to xui. So LazyRAD owns the designer overlay and the property schema (G6), and uses xui's `Properties` only to push values into live widgets.

---

## 11. Milestones

Each milestone ends in something demoable. Sizes are rough estimates for one developer.

| M | Name | Deliverable | Size |
|---|---|---|---|
| M0 | Skeleton | Workspace; xui pinned; IDE window with menu, toolbar and split panes; open/save `.lrp`; project tree. Spike for G11 | 1–2 wk |
| M1 | Runtime | `lazyrad-runtime` + player: loads a hand-written project, builds forms from `.lfm`, wires `Control_Event` handlers, `on_var` control resolution, `MsgBox`, 8 basic controls, Core/Collections stdlib. "Hello" and "Calculator" samples run | 2–3 wk |
| M2 | Editor v1 | Custom editor: rope, caret, selection, undo, scroll, highlight, gutter, find, compile diagnostics, run with F5 (spawning the player) | 3–4 wk |
| M3 | Designer v1 | Design surface, toolbox, select/move/resize, property grid (text/num/bool/enum), double-click to create a handler, save `.lfm` | 3–4 wk |
| M4 | Debugger | Debug protocol, breakpoints, step into/over/out, call stack, locals, immediate window, break on error | 2–3 wk |
| M5 | Export | Payload packer, Windows/Linux stubs, icon and version resources, *File → Make EXE* | 1 wk |
| M6 | Completion & polish | Completion and signature help from metadata and AST, go to definition, class modules, menu editor, tab order, remaining stdlib (IO, Time, App, CommonDialog), themes, samples, docs | 3–4 wk |
| M7 | LazyOS | musl player running exported apps on LazyOS (needs xui-skia and the LazyOS threads and `fs` layers), then the IDE itself (needs process spawn and pipes, i.e. LazyOS L4, or an in-process debug fallback) | later |

**v1.0 = M0–M6.** Scope cuts, if needed: folding, watches, control arrays and the menu
editor move to 1.x.

---

## 12. LazyOS notes (for later)

- LazyOS runs xui apps as static `x86_64-unknown-linux-musl` binaries on its Linux ABI
  shim (`lazyos/xui-app`). LazyRAD's player is exactly that kind of binary, so
  **exported apps are the first thing to run there**. The IDE comes second.
- Requirements: `std` threads (L3, which the debugger's compile worker needs), `fs`
  (to load projects and write exports), anonymous `mmap` only (so embed the font, as
  xui-app does), and for the IDE's run/debug, `fork`/`execve`/pipes (L4).
  - **Fallback when L4 is missing:** run the program in-process on a nested xui window,
    with the debugger callback pumping a mini event loop. This is less robust, but it
    has no dependency on process spawning.
- Keep all platform assumptions behind a small `lazyrad-runtime::platform` module:
  paths, config directory, clipboard, file dialogs and spawning. That way the LazyOS
  port is one new implementation of that module.

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Rhai's C-like syntax feels nothing like BASIC to VB users | Accept it and embrace it: VB-style *names* (`Form_Load`, `MsgBox`, `Left$`-like helpers) and good completion. Do not build a BASIC-to-Rhai transpiler |
| Rhai performance for tight loops | Fine for RAD apps. Ship native stdlib types for heavy work (sort, string building, `Dictionary`) |
| Scope creep in the editor | A monospace-only v1, no word wrap, no proportional fonts |
| xui churn (the project is young) | Pin the revision, keep a patch fork, upstream small PRs for G1/G2/G9/G10 |
| Blocking `MsgBox` on canvas (G14, confirmed) | Ship a non-blocking `MsgBox` with a callback now, and fix `run_modal` upstream |

---

## 14. Immediate next steps

1. Convert the repo into the §2 workspace. Add xui (pinned) and `rhai` with the
   `debugging`, `metadata` and `internals` features.
2. Spike G11 (menu shortcuts and rebuilds). Write the result into §10.
3. Write `lazyrad-project` (the `.lrp`/`.lfm` serde model) plus the "Hello" sample,
   then implement M1 so the player runs it.
