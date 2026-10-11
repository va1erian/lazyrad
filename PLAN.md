# LazyRAD — Plan

LazyRAD is a RAD IDE with the development workflow of Visual Basic 6. It is written in Rust, uses the
[xui](https://github.com/va1erian/xui) toolkit for its UI, and uses
[Rhai](https://rhai.rs) as its scripting language. You draw a form, double-click a
button, write the handler, press F5, step through it in the debugger, then export a
single `.exe`. [LazyOS](https://github.com/va1erian/lazyos) is a later target.

This document covers the architecture, the key decisions, a milestone plan, and a list
of the **xui gaps** LazyRAD will run into, along with a proposed fix for each.

## 0. Status (October 2026)

LazyRAD is past its first usable cut: you can draw a form, write handlers, press F5,
stop on a breakpoint and export a `.exe` or a LazyOS `.lzp`. The large piece still
open is polishing **LazyOS** (M7); the debugger (M4) has landed. Per milestone (§11
has the detail):

| M | State |
|---|---|
| M0 Skeleton | Done |
| M1 Runtime | Done: player, forms, event binding, `on_var`, non-blocking `msg_box`, four samples |
| M2 Editor | Done: a reusable `xui-code-editor`, highlighting, diagnostics, find/replace, go to line, tab stops, completion |
| M3 Designer | Done: surface, toolbox, property grid, double-click to handler. One PR open (#114: clip forms, scrollbars) |
| **M4 Debugger** | **Done, minus watches.** `lazyrad-debug-proto`, `lazyrad-player --debug`, F9 breakpoints with a gutter dot and a stop-line tint, Continue / Step Into / Over / Out / Break / End, break on error, and Locals, Call Stack and Immediate tabs (§7). Watches and a Breakpoints list are 1.x |
| M5 Export | Done: stub + payload, PE icon/version/subsystem, *Make EXE*, `.lzp` packages and `lazyrad-pack` |
| M6 Polish | Partial. See §11 for what landed and what is left |
| M7 LazyOS | **Running.** The player and the IDE both run on LazyOS as `xuid` clients (`lazyos/lazyrad-os`, core package `os.lazy.lazyrad`), including *Make LazyOS App*. The debugger runs there through the launcher's stdin queue (§12); rough edges are listed there too |

Beyond the original plan, these landed: hot reload (`--watch`, §4.2), script errors as
first-class failures with a host test kit and a Rhai lint (`check_project`), a `Canvas`
control with a frame loop (`examples/brickbreaker`), a `Timer` control, binary file
access, `open_file_dialog`, project assets, a `PictureBox` and `Value::Bytes`, and a
Button `icon` property (Lucide outlines).

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
| VB6's role | **The workflow, not the code.** Draw a form, double-click a control to jump to its handler, F5 to run, an integrated debugger: that is what LazyRAD takes from VB6. Everything in code follows modern conventions (§1.1) | The goal is VB6's ease of use in a modern tool, not a VB6 clone. |

### 1.1 Code conventions

These apply to everything a user writes or sees in code, and to LazyRAD's own APIs:

- **Control kinds and properties are xui's names**, with no VB aliases: `Button`, `Edit`, `Label`, `CheckBox`, `RadioGroup`, `GroupBox`, `ListView`, `ComboBox`; `text`, `selected`, `checked`, `enabled`, `visible`, `left`/`top`/`width`/`height`.
- **Names are snake_case.** Default control names are the kind plus a number (`button1`, `edit1`); projects, forms and modules too (`main_form`, `util`).
- **Handlers are `fn <control>_<event>(args…)`** in snake_case, with xui's event names lowercased: `fn hello_button_click()`, `fn name_edit_change(text)`. The form's own events use the `form` prefix: `fn form_load()`, `fn form_close()`.
- **The current form is `form`** in scripts (`form.title`, `form.state`, `form.show()`), replacing VB's `Me`.
- **The stdlib is snake_case, 0-based and small.** It leans on Rhai's built-in string, math and collection functions and only adds what they lack (`msg_box`, `app`, `debug`, time). There are no VB-compatibility helpers (`Left$`, `Mid$`, `Val`…).

---

## 2. Workspace layout

```text
lazyrad/
├─ Cargo.toml                 (workspace)
├─ crates/
│  ├─ xui-form/               declarative forms for xui: schema, document, validation, builder
│  ├─ lazyrad-project/        .lrp project files + the LazyRAD control catalog
│  ├─ lazyrad-runtime/        Rhai engine setup, stdlib, form instantiation, event binding
│  ├─ xui-rhai/               Rhai <-> xui bridge: control wrappers, `on_var`, messages
│  ├─ lazyrad-debug-proto/    IDE <-> player debug protocol: serde types and a line codec (no OS deps)
│  ├─ lazyrad-player/  (bin)  the runtime host: runs a project dir or a payload; `--watch` reloads, `--debug` debugs
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
my_app/
├─ my_app.lrp         # project: name, version, startup form/module, icon, references
├─ main_form.lfm      # form layout (TOML) — controls, properties, menu tree
├─ main_form.rhai     # form code-behind: event handlers + form-level functions
├─ util.rhai          # module: shared functions, exported as a Rhai module
└─ stack.rhai         # class module (see §4.3)
```

A form file is an [`xui-form`](crates/xui-form) document (`.lfm`), a flat list of
nodes with `parent` references and a typed schema. The schema is `xui-form`'s own,
read through `lazyrad_catalog()`, with no aliases. It looks like this:

```toml
format = 1

[window]
name = "main_form"
title = "Hello"

[[node]]
kind = "Button"
name = "hello_button"
left = 16
top = 16
width = 120
height = 32
text = "Say hello"
```

`xui-form` owns the schema, the `FormDoc` document, validation, and the builder
that turns a document into live `xui` widgets. `lazyrad-project` keeps only the
`.lrp` project file and the LazyRAD catalog; it has no form model of its own.

**Event binding** is by naming convention (§1.1). The runtime looks for
`fn <control>_<event>(args…)` in the form's script (`hello_button_click`, `form_load`,
`name_edit_change`) and connects each one to the matching xui `on_*` closure. When a
function is missing, the event is not wired.

---

## 4. Runtime and language

### 4.1 How scripts see controls (a Rhai gotcha)

Rhai functions are pure: **they cannot see variables from the enclosing scope**, so
`name_edit.text = "x"` inside `fn hello_button_click()` would normally fail. LazyRAD
gets around this with an `Engine::on_var` resolver. When a name is not found, the
resolver looks it up among the script's top-level `let`/`const` variables (kept after
the top-level code runs, as shared values so an assignment in one handler is seen by
all), then the active form's controls, then the form itself (`form`), then globals
such as `app`. Controls are registered custom types that wrap an `Rc`
handle, so a property setter changes the live widget. (A value *returned* by `on_var`
is read-only in Rhai, so the resolver pushes the control into the scope instead.)

The resolver **pushes the value into the scope** rather than returning it from the
callback: Rhai marks a value returned by `on_var` read-only, so a setter such as
`lbl.caption = …` would fail with a "cannot modify property of constant" error,
while a pushed variable is an ordinary mutable entry.

```rhai
fn hello_button_click() {
    result_label.text = `Hello, ${name_edit.text}!`;
    form.title = "Greeted";
}
```

Form-level state that outlives a single event lives in `form.state`, an object map.

### 4.2 Execution model

- The player owns the xui event loop. Each widget event is turned into a
  `Msg::Event { form, control, event, args }`. `App::update` then calls the matching
  Rhai function through `Engine::call_fn` on the form's compiled `AST`.
- `Engine::on_progress` implements Ctrl+Break and an operation budget per event (to
  catch runaway loops). Under `--debug` the debugger's own callback also reads pause
  and stop requests (§7).
- **Hot reload.** The player's `--watch` flag polls the project's `.lrp`, `.lfm` and
  `.rhai` files every 500 ms. On a change it checks and loads a fresh runtime first, and
  on success rebuilds each open form in place (releasing the old form's event sources and
  running `form_load` again); on failure it keeps the running forms and shows the
  diagnostics in a banner. A script can keep its place across a reload by defining
  `fn form_reload(old_state)`, which runs after `form_load` with the old `form.state`.
  The IDE passes `--watch` when it runs a project.
- `Engine::set_max_*` limits apply by default, but are relaxed for exported apps.
- `msg_box` and `input_box` use xui `Dialog`. A blocking `msg_box` would need
  `Ui::open_modal`, which **does not work on canvas today** (G14). So in Iteration 1,
  `msg_box` is non-blocking and takes an optional callback. It becomes blocking once
  canvas implements `run_modal`.

### 4.3 Object orientation

Rhai has no `class` keyword. LazyRAD's planned object model provides objects through
two mechanisms (the native classes exist; class modules, their template and `__class`
completion are M6 work, not built yet):

1. **Native classes (the stdlib).** These are Rust types registered with
   `register_type_with_name`, getters, setters and methods. They are true types with
   `type_of()`, `is`-style checks and `to_string`.
2. **Script classes (class modules, planned).** A `.rhai` class file will export `fn new(...)`, which
   returns an object map. Its methods are `Fn` pointers called with `this` bound, which
   Rhai supports natively. The IDE's class-module template will produce this shape:

   ```rhai
   // stack.rhai
   fn new() {
       #{ items: [], push: |x| this.items.push(x), pop: || this.items.pop(),
          len: || this.items.len(), __class: "stack" }
   }
   ```

   The editor's completion will recognise `__class` and offer the map's members.
   Inheritance works by composition: `let o = base::new(); o.extra = …; o`.

### 4.4 Standard library: small but usable

The stdlib is written in Rust and registered as snake_case Rhai functions and static
modules. It relies on Rhai's own built-ins for strings (`len`, `sub_string`,
`index_of`, `trim`, `to_upper`, `split`…), maths and collections, and only adds what
they lack. Every item gets doc comments, which the `metadata` feature surfaces in
completion and tooltips.

**Status.** Built: `msg_box`, `open_file_dialog`, `file_*`/`dir_*` (text and bytes,
behind the platform's `FsPolicy`), `app` (`title`, `path`, `documents`, `quit()`),
`now()`/`today()`, `random*`, `array.join`, `rgb`/`clamp`/`rects_overlap`, the controls
below, `Timer`, `PictureBox` and `Canvas`. **Not built yet** (the table lists them as
target design): `input_box`, `form.show_modal`, save-file and colour dialogs, the
`DateTime` type and `stopwatch`, `env`, `clipboard`, `screen`, the
`set`/`queue`/`stack` helpers and `sort_by`, and the whole *Data* row. The control
catalog also has no `TreeView`, `Tabs` or `Menu` kinds yet; it has `Hyperlink`,
`NumberField`, `Panel`, `Separator` and `ToggleButton`.

| Module | Contents |
|---|---|
| Core | `debug(value)` to the Output pane, formatting helpers, `random`/`random_range`, conversions Rhai lacks |
| Collections | Rhai arrays and maps, plus `set`, `queue`, `stack` helpers and `sort_by` |
| Time | `now()`, `today()`, a `DateTime` type (formatting, arithmetic), a `Timer` control, `stopwatch` |
| IO | `file` (`read_text`, `write_text`, `append`, `lines`, `exists`, `delete`), `dir`, `path` |
| App | `app` (`path`, `title`, `version`, `args`, `quit()`), `env`, `clipboard` (text only), `screen` |
| UI | `form` (`show`/`hide`/`show_modal`/`close`, `title`, `left`/`top`/…), `msg_box`, `input_box`, `dialogs` (open/save/colour) |
| Controls | xui's kinds: `Label`, `Edit`/`MultilineEdit`, `Button`, `CheckBox`, `RadioGroup`, `GroupBox`, `ListView`, `ComboBox`, `Slider`, `ProgressBar`, `TreeView`, `Tabs`, `Menu`, plus a `Timer`, a `PictureBox` (PNG, JPEG, BMP and GIF with fit, zoom, rotation and panning) and a `Canvas` (drawing methods, a frame loop and raw input, for games; see `examples/brickbreaker`) |
| Data (later) | `json` (parse/stringify to maps), `csv`, `settings` (per-app key/value storage) |

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

### Custom editor design (`xui-code-editor`)

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
- **Model-first:** the designer edits the `xui-form` [`FormDoc`](crates/xui-form)
  and re-applies it to the live widgets through `LiveForm::set`, which places them
  again in the form's `absolute()` layout.
  Undo and redo operate on the model. The preview is built with
  `build_with(.., BuildOptions { design_mode: true })`, which consults no event
  binder, while the designer has put its own host node in design mode.
- **Toolbox:** a grid of control types. You either click a type and draw it, or
  double-click to drop it at a default size. It needs icon buttons (gap G9).
- **Property grid:** a two-column list of name and value with editors per type: text,
  number, bool, enum dropdown, colour, font, and a `…` button for dialogs. It is a new
  widget (gap G5). The `xui-form` `Catalog` supplies each control's typed schema,
  with its category, description and access rule. An object combo above the grid
  selects the control.
- **Double-click on a control:** this opens the code window at
  `fn <name>_<default event>()`, creating the function if it does not exist, exactly
  as VB does. The procedure combos at the top of the code window list the control's
  events.
- **Other tools:** a menu editor dialog, a tab-order mode, align/size/centre commands,
  snap-to-grid, copy and paste of controls (in-app first; the OS clipboard is gap G2),
  and control arrays (later).

---

## 7. Debugger

> **Status: built (M4), minus watches and a Breakpoints list.** The pieces:
> `crates/lazyrad-debug-proto` (the protocol), `lazyrad_runtime::debug` (the session
> and Rhai callback), `lazyrad-player --debug` (the stdin/stdout transport), and the
> IDE's `debugging.rs`, `app/debug.rs` and the launcher seam in `run.rs`.
> Deviations from the first sketch are marked *(changed)* below.

The debugger is built on Rhai's `debugging` feature, running in the player process.

```text
IDE  ──spawn── lazyrad-player --debug <project dir>      (also hot-reloads, like --watch)
 │   stdin : {"v":1,"cmd":"set_breakpoints","file":"main_form.rhai","lines":[12,30]}
 │           {"v":1,"cmd":"continue"|"step_into"|"step_over"|"step_out"|"pause"|"stop"}
 │           {"v":1,"cmd":"evaluate","expr":"name_edit.text","frame":0,"id":7}
 │   stdout: {"v":1,"event":"ready","player":"lazyrad-player 0.1.0"}
 │           {"v":1,"event":"stopped","reason":"breakpoint","file":…,"line":12,
 │            "stack":[{"function":…,"file":…,"line":…}],"locals":[{"name":…,"type":…,"value":…}]}
 │           {"v":1,"event":"resumed"}   {"v":1,"event":"evaluated","id":7,"ok":true,…}
 │           {"v":1,"event":"output",…}  {"v":1,"event":"error",…}  {"v":1,"event":"exited","code":0}
```

- **Protocol.** One JSON object per line, every message carrying `"v"` (a different
  version is refused and the IDE prints a notice). *(changed)* Command names are
  snake_case, `evaluate` carries an `id` echoed in `evaluated`, and the call stack
  and locals ride inside the `stopped` event so a stop is one atomic message. Lines
  are one-based; files are project-relative with `/` (`normalize_file`). A line that
  is not a command (bad JSON, unknown name, wrong version, oversized, invalid UTF-8)
  is answered with an `error` event and ignored; the reader caps a line at 256 KiB
  and a `stopped` event is trimmed to 40 KiB (`truncated`).
- **Player.** When the program stops it serialises the call stack and the innermost
  frame's locals, then **blocks, reading its transport**, until a resume command: the
  paused window freezes as it did in VB6, and a window manager may call it "not
  responding" until the IDE resumes it. The IDE stays responsive because it is
  another process. *(changed)* The callback asks Rhai for every node, which is what
  makes breakpoints, the three steps, break on error and pause one uniform rule at
  a small per-node cost; the normal (non-`--debug`) path installs no callback and
  pays nothing.
- **Breakpoints** are `(file, line)` pairs held by the session, not by a compiled
  script, so they survive a hot reload unchanged: the new engines get the callback
  and the pairs still match by file and line. A line past the end never matches.
  The IDE re-sends its set on every save, which is when the player's text matches
  the editor's again. A breakpoint fires once when a line is reached, not once per
  expression on it (a call level's last line is tracked), so a one-line loop body
  stops once per entry, not once per iteration.
- **Stepping.** Over stays at or above the starting call depth, out waits for a
  lower one. Stepping past the end of a handler, or out of the outermost frame, ends
  the stop and the program is idle again: there is no "stop in the next handler".
  Step out lands on the line after the call.
- **Break on error.** A script error stops at the failing position, in the frame
  that raised it, with `reason: "exception"`, the message and that frame's locals
  (Rhai rewinds a function's scope before reporting its exit, so the callback keeps a
  once-per-line note of the locals for this). Resuming lets the error take its
  normal route (the player's message box). Errors in a script's top-level code run
  before any handler and are reported as startup failures instead. A runaway loop
  ends at the operation budget as such an error.
- **Pause and Break** are read by the callback every 128 nodes, so a loop that never
  returns can be interrupted. When no script runs a pause stays armed and stops at the
  first line of the next handler: the IDE says so, since a program that is idle
  between events cannot stop earlier. *(changed)* Pause does not use `on_progress`
  (it cannot enter the debugger); `stop` does use `EngineHost::stop` through a stop
  handle on every attached engine, then exits the process.
- **Evaluate** compiles the text against the stopped frame's scope, with the script's
  functions and the form's controls in reach, and may assign. Only frame `0` has a
  scope: Rhai keeps none for outer frames, so the Call Stack lists them (activating a
  row opens its file and line) but their locals are not available.
- **IDE.** F9 toggles a breakpoint on the caret's line (a gutter dot; the editor has
  no arrow glyph, so the stop line is a whole-line tint: warning colour for a step or
  breakpoint, error colour for an error). *Start* (F5) runs under the debugger when
  breakpoints exist and as a normal run with hot reload when they do not; *Start with
  Debugging* (Ctrl+F5) always debugs. While stopped F5 continues, F8 / Shift+F8 /
  Ctrl+Shift+F8 step into / over / out, Ctrl+Pause breaks, Shift+F5 ends. The bottom
  area has Output, Locals, Call Stack and Immediate tabs. The Immediate pane has an
  Evaluate button (xui's `Edit` has no submit event yet); when nothing is stopped it
  says so instead of evaluating. Breakpoints belong to the IDE session, keyed by
  project item: they follow renames, go with a removed item, move with edits (a diff
  of the text before and after, so inserting lines above carries them down and a
  deleted line takes its breakpoint with it), never dirty a document or touch its
  undo, and are not written to the project.
- **Transport.** The IDE sends commands with `ChildProcess::send_line` (non-blocking;
  the desktop launcher uses a writer thread, a polling launcher queues the bytes) and
  receives events through the same sink as output (`RunEvent::Debug`), so no IDE-side
  reader thread is needed. The player reads stdin on a reader thread on the desktop;
  a host without that (LazyOS) installs its own transport with
  `lazyrad_player::debug::install_transport`, and a thread that cannot read stdin
  falls back to blocking reads at each stop. When the pipe closes, a stopped player
  **detaches**: it resumes and runs on with the debugger off, so it is never left
  blocked on a dead pipe.
- **Edit-and-continue:** not planned. Reloading a changed handler function between
  events is a possible later stretch. **Watches** and a **Breakpoints list** are 1.x.
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
   - No console window: the player is built once, as a console program (the IDE reads
     a running project's output through pipes). Export flips the *copy's* PE subsystem
     to GUI (`editpe`), so there is no second binary. An exported app reports startup
     failures in a native message box. The player embeds LazyRAD's icon as the default;
     the optional `icon` field of the `.lrp` (a plain `.ico` file name in the project
     folder) replaces it, and the version resource comes from `name` and `version`.
   - The payload is `stub | uncompressed archive | 40-byte footer` (magic, version,
     entry count, offset, length, FNV-1a checksum); see `lazyrad-packager`. It holds
     only plain-named `.lrp`, `.lfm` and `.rhai` files. Export writes through a
     temporary file and renames it into place.
   - Code signing is out of scope, but note in the docs that an appended overlay
     invalidates any existing signature.
5. **LazyOS packages (`.lzp`).** For LazyOS the export is not a stub with an appended
   payload but a standard application package (`docs/packages.md` in the LazyOS
   repo): a zip with a generated `manifest.toml`, the LazyOS player as
   `bin/lrplay.elf`, `icons/app-{16,32,128}.png` and the project under
   `resources/project/`. `lazyrad_packager::lzp` writes it (`build_package`, a zip
   writer that emits exactly the subset LazyOS's `lazypkg` reads, default icons,
   permissions derived from the scripts) and the `lazyrad-pack` command wraps it:
   `lazyrad-pack <project> --player lrplay.elf --out dist`. Installing goes through
   the `Installer` trait: `pkgd` when LazyOS has it, otherwise `DevInstaller` saves
   the package ("saved, not installed"). The manifest's `entry.args` is
   `["--project", "resources/project"]`, which the player resolves against its own
   install directory. `lazyrad-packager` tests re-read every package with an
   independent verifier; LazyOS's tests re-open the same packages with `lazypkg`.
6. **Where stubs live:** the IDE ships with its stubs in `stubs/<target>/`. Picking
   another target in *File → Make EXE* is how cross-export works.

---

## 9. IDE shell (`lazyrad-ide`)

The layout follows VB6: a menu bar and toolbar at the top, the toolbox on the left, a
document area in the centre, project explorer and properties on the right, and
immediate/locals/watch panes at the bottom.

- **Layout:** one xui `arrange` layout: a column of the menu bar, the toolbar,
  nested `split`s and the status line, with `tabs` for the documents. MDI is
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

**Re-audit (October 2026, against what LazyRAD's code does today; xui itself was not
re-read).** The table below keeps the original text; read it with this overlay:

| Gap | Today |
|---|---|
| G1 | `ScrollBar` is exported and used. The code editor and scrollbar moved into xui (issue #72). A `Popup` helper: unchecked |
| G2 | Resolved for the IDE: the clipboard goes through the window backend (`Ui::clipboard_text`), and the editor no longer uses `arboard`. The stdlib `clipboard` is not built |
| G4 | Worked around with `rfd` (desktop) and in-window dialogs (`Platform` trait). A painted file dialog for LazyOS is LazyOS's to supply |
| G5 | Built in LazyRAD (`property_grid.rs`, with keyboard navigation and a scrollbar) |
| G6 | Worked around: `xui-form`'s own typed `Catalog` schema |
| G7 | Worked around by the monospace grid; the editor is now upstream in xui |
| G9 | Toolbox built; `Button` gained an `icon` property. Per-item toolbar icons: unchecked |
| G10 | Worked around: IDE-level shortcut dispatch (#69) |
| G11 | Open (xui#185). The IDE still has no menu-editor and a user program cannot declare a menu: there is no `Menu` kind in the catalog |
| G13 | Resolved in LazyRAD: `PictureBox` (PNG/JPEG/BMP/GIF) and `Value::Bytes` |
| G14 | **Still open.** `msg_box` is non-blocking, there is no `input_box` and no `form.show_modal`. Re-check xui#146 before M6 closes |
| G15 | Handled by the `desktop` feature split (§12) |
| G17 | Still open. Colours stay theme-driven |
| G18 | Resolved upstream (xui#204) |
| G3, G8, G12, G16 | Unchanged, not blocking |

| # | Gap | Impact on LazyRAD | Proposal |
|---|---|---|---|
| G1 | **No first-class custom widget story.** `Control` + `NodeKind::Custom` + `set_painter` + `on_events` are public, but popup and scrollbar painters (`widget::popup`, `widget::scrollbar`) are private | The editor, designer overlay and property grid need scrollbars and popups that match the theme | Upstream: export `ScrollBar` as a widget and a `Popup` helper. `ScrollBar` is now exported from `xui_core::widget::scrollbar` (xui#231); the `Popup` helper is still open |
| G2 | **No portable clipboard.** It exists only in `xui-win32`, not in `xui-core` or `xui-canvas` | Copy and paste in the code editor and the designer, plus the stdlib `Clipboard` | Upstream: add `Backend::clipboard_get_text`/`set_text` with a default of `Unsupported`. Implement it with `arboard` in canvas and with `clipboardd` on LazyOS. Workaround: an in-process clipboard |
| G3 | **No IME or composition events.** `Event` has `Char` and `KeyDown` only | Painted text editors can't enter CJK text. Low priority for v1 | Upstream: add `Event::Ime{Preedit,Commit}` (winit already provides it) and `set_ime_cursor_area` |
| G4 | **No file, folder or colour dialogs** | Open/save project, Make EXE, `CommonDialog` | Workaround: use `rfd` on desktop. LazyOS needs a painted xui file dialog, which can be shared upstream |
| G5 | **No property-grid or editable list.** `ListView` has no in-place cell editing | Properties window | Build it in LazyRAD on `Custom` plus overlay `Edit`/`ComboBox`/`ColorPicker`. It could move upstream later |
| G6 | **`Properties::Value` is limited to Bool, Integer, Float and Text.** It has no enum, colour, font, rectangle or image, and no property schema covering read-only, category, enum values or description | The designer needs typed editors and "Categorized" vs "Alphabetic" views | Upstream: add `Value::{Color, Enum, Font, Image}` and `fn schema() -> Vec<PropertyInfo>`. Workaround: LazyRAD's own schema per control type in `lazyrad-runtime` |
| G7 | **The text layout API has no caret-from-offset query**, only `hit_test_point` and `selection_rects`. Each layout also has a single style, with no styled runs | The editor needs caret placement and multi-colour lines | Workaround: a monospace grid (§5). Upstream: add `caret_rect(byte)` and an attributed layout |
| G8 | **No docking or floating tool-window framework.** `layout::Dock` is arithmetic only, and there is no MDI | VB-style dockable panes | v1 uses a fixed layout with `Split`/`Tabs` and toggleable panes. Later, build a dock manager (possibly upstream) on `open_window` |
| G9 | **`Toolbar` has text labels only**, with no icons, toggle groups or tooltips per item. `TopBar` has vector `Glyph`s but a fixed glyph set | The toolbox and main toolbar need icons | Upstream: `Toolbar` items with `RowIcon` (glyph or `Image`). Workaround: a toolbox grid as a `Custom` node, or `GridView` with icons |
| G10 | **No portable accelerator or shortcut table.** `Event::Accelerator(u16)` exists, but core has no API to register key bindings | Ctrl+S, F5, F8, F9 and similar | Workaround: handle `KeyDown` at window level in LazyRAD's command dispatcher. Upstream: `Ui::set_accelerators(&[(KeyChord, Msg)])` |
| G11 | **Menu shortcut text and runtime rebuilds are missing (spike result, xui#185).** `MenuScope::item` takes only an id and label and `Node` has no accelerator field, so an item cannot display `Ctrl+S`. `Menu::build` consumes the menu and fills a private model with no public rebuild call, so menus cannot change at runtime (the Recent list, or a user program's menu tree); `set_enabled`/`set_checked` are the only mutations | IDE menus and the runtime `Menu` control | Workaround: build the menu once, keep the accelerator table in the command dispatcher, and replace the whole `Menu` when the recent list changes. Upstream (xui#185): an optional shortcut on each entry that the popup painter right-aligns, plus `Menu::rebuild` |
| G12 | **No drag and drop of OS files** | Dropping a `.lrp` file onto the IDE. Nice to have | Upstream later, via winit `DroppedFile` |
| G13 | **No image or picture widget and no `Image` loading helper** in the catalogue (only `draw_image`) | `PictureBox`, `Image`, form icons, toolbox icons | Build a small `Picture` widget on `Custom` + `draw_image`, and decode with the `image` crate (PNG/BMP/JPG) |
| G14 | **Modal windows don't work on canvas** (confirmed: xui#146, tracked in emusic#417). `run_modal` returns `Unsupported`, so `Ui::open_modal` closes the child window immediately | Blocking `msg_box`/`input_box` and `form.show_modal` | Iteration 1: `msg_box` is an in-window `Dialog` and is **non-blocking** (with an optional callback). Upstream: implement `run_modal`/`set_window_enabled` on `WinitBackend` (a nested loop), then make `msg_box` blocking |
| G15 | **`xui-canvas` hard-depends on `winit`, `softbuffer` and `glutin`.** LazyOS vendors a patched copy (`set_default_font`, `Surface::pixels`) | The LazyOS build of the player and IDE | Track the `xui-skia` split proposed in `lazyos/docs/xui-plan.md`. LazyRAD adds nothing new beyond it |
| G16 | **Single window per task on LazyOS** (see `lazyos/docs/xui-plan.md`) | Multi-form apps and IDE secondary windows | On LazyOS, show secondary forms as in-window `Dialog`-style surfaces until multi-window lands |
| G17 | **No per-control font, colour or back-colour overrides.** Widgets draw only from theme tokens | VB users expect `ForeColor`, `BackColor` and `Font` properties | Upstream: optional per-node style overrides. For v1, support `Font.Size`/`Bold` on `Label` only and leave colours theme-driven (a deliberate trade-off) |
| G18 | ~~**No per-widget timers.**~~ **Resolved** in xui#204: `Control::set_timer`/`kill_timer`, and canvas timers now repeat as documented | The editor's caret blink | The editor can own its blink timer instead of the host forwarding ticks |

**G11** is worth a one-day spike before M0 ends. **G14** is confirmed and shapes the `msg_box` API. The rest have workarounds and are not blockers.

**Resolved upstream (2026-09-28):** scoped design mode (xui#192), `Ui::set_window_title` (xui#194), per-widget timers (xui#204, G18), `TreeView::on_activate` (xui#196), `Tabs` add/remove/rename pages (xui#198), diagonal resize cursors (xui#200) and `OffscreenBackend::raise` (xui#202).

**Designer seam status.** The Win32 clean-up (xui#173, tag `pre-win32ui-controls-removal`) deleted the *native-layer* `Properties` impls in `xui-win32/src/properties.rs`. The portable seam survives in `xui-core`: `Ui::set_design_mode`, which about 25 widgets honour by ignoring input, and `Properties` impls on about 25 widgets. Those impls report only a few names (`text`, `enabled`, `checked`, `value`, `selected`…), with no bounds, font or schema. Per xui#16, selection and handle painting, serialisation and schemas belong to the RAD app, not to xui. So LazyRAD owns the designer overlay and the property schema (G6), and uses xui's `Properties` only to push values into live widgets. **Update:** xui#192 scopes design mode to a container (`Panel::set_design_mode`, and the same on `ScrollView`, `Split` and `Tabs`), so the designer can live in the IDE window while the rest of the IDE stays live.

---

## 11. Milestones

Each milestone ends in something demoable. Sizes are rough estimates for one developer.

| M | Name | Deliverable | Size |
|---|---|---|---|
| M0 | Skeleton | Workspace; xui pinned; IDE window with menu, toolbar and split panes; open/save `.lrp`; project tree. Spike for G11 | 1–2 wk |
| M1 | Runtime | `lazyrad-runtime` + player: loads a hand-written project, builds forms from `.lfm`, wires `control_event` handlers, `on_var` control resolution, `msg_box`, 8 basic controls, Core/Collections stdlib. "Hello" and "Calculator" samples run | 2–3 wk |
| M2 | Editor v1 | Custom editor: rope, caret, selection, undo, scroll, highlight, gutter, find, compile diagnostics, run with F5 (spawning the player) | 3–4 wk |
| M3 | Designer v1 | Design surface, toolbox, select/move/resize, property grid (text/num/bool/enum), double-click to create a handler, save `.lfm` | 3–4 wk |
| M4 | Debugger | Debug protocol, breakpoints, step into/over/out, call stack, locals, immediate window, break on error | 2–3 wk |
| M5 | Export | Payload packer, Windows/Linux stubs, icon and version resources, *File → Make EXE* | 1 wk |
| M6 | Completion & polish | Completion and signature help from metadata and AST, go to definition, class modules, menu editor, tab order, remaining stdlib (IO, Time, App, CommonDialog), themes, samples, docs | 3–4 wk |
| M7 | LazyOS | musl player running exported apps on LazyOS (needs xui-skia and the LazyOS threads and `fs` layers), then the IDE itself (needs process spawn and pipes, i.e. LazyOS L4, or an in-process debug fallback) | later |

**Status per milestone (October 2026)**

| M | State | Evidence and what remains |
|---|---|---|
| M0 | Done | Workspace, pinned xui, IDE shell, `.lrp` load/save, explorer, start page, settings, theme |
| M1 | Done | Samples: hello, calculator, todo, brickbreaker. Core stdlib done; Collections helpers (`set`/`queue`/`stack`, `sort_by`) not |
| M2 | Done | Find, Find Next, Replace, Go to Line, completion (`completion.rs`), tab stops, diagnostics. Folding was already cut to 1.x |
| M3 | Done | Open PR #114. The designer's menu editor and tab-order *mode* are M6 items (below) |
| M4 | Done (no watches) | `lazyrad-debug-proto`, `lazyrad-player --debug`, breakpoints (F9), Continue, Step Into/Over/Out, Break, End, break on error, Locals, Call Stack and Immediate panes. Not done: watches, a Breakpoints list, an arrow glyph for the stop line (needs an xui editor change), evaluate in outer frames, Return-to-evaluate in the Immediate pane (xui `Edit` has no submit event). The LazyOS launcher side is in the LazyOS repo (`lazyrad-os/src/launcher.rs`) |
| M5 | Done | Stub + payload packer, `editpe` icon and version, subsystem flip, `lazyrad-pack`, `.lzp` packages. Cross-target stub picking in *Make EXE*: unchecked |
| M6 | Partial | **Done:** themes, samples, standard shortcuts, start page and tutorial, most of IO and App, `Timer`, dialogs for open-file, hot reload, lint. **Not done:** go to definition, signature help, class modules (`__class` completion and template), menu editor, tab-order mode, `input_box`/save/colour dialogs, Time (`DateTime`, `stopwatch`), `clipboard`, `env`, `screen`, docs |
| M7 | Running, janky | Player and IDE run on LazyOS (`lazyos/lazyrad-os`, plan phases P0–P4 done; the LazyOS-side plan is `lazyos/docs/lazyrad-plan.md`). Spawning the player over pipes works (polled, §12). The debugger's LazyOS side passed a QEMU session once (§12; Call Stack, Immediate, break on error and the F8 keys are unverified there). Outstanding: IDE responsiveness, the in-window file dialog panic, Manage Apps |

**v1.0 = M0–M6.** Scope cuts, if needed: folding, watches, control arrays and the menu
editor move to 1.x. As of October 2026 the realistic v1.0 cut is: M4 (without watches),
the M6 items under "Not done" except the menu editor, and the docs. Everything below
that line is 1.x.

**1.x backlog** (carried here so it is not lost): code folding; debugger watches;
control arrays; the menu editor and the `Menu`, `TreeView` and `Tabs` control kinds;
the Data stdlib (`json`, `csv`, `settings`); per-control colours and fonts (G17);
IME (G3); OS file drag-and-drop onto the IDE (G12); a docking manager (G8).

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
    has no dependency on process spawning. **Not needed:** L4 landed, and LazyOS
    spawns the player with pipes.
- Platform assumptions live behind one seam module per crate, so the LazyOS port is a
  new implementation of those modules only. See "Porting LazyRAD" below.

### Findings from the LazyOS port (October 2026)

The IDE and player run on LazyOS (`lazyos/lazyrad-os`, packaged as `os.lazy.lazyrad`;
full notes in `lazyos/docs/lazyrad-plan.md` §8–§9). It lives in the LazyOS repo and
depends on these crates by git `rev`, with `default-features = false`. What it
needed from LazyRAD, and what it found:

- **Seams held.** `Platform`, `run_with_backend` and the `desktop` feature were enough;
  LazyOS supplies its own `Platform` (script sandbox, config dir, player beside the
  IDE, painted file dialog) with no LazyRAD changes beyond the seam.
- **Make LazyOS App** goes through the `Installer` seam, using the Installer and
  `mimed` rather than `pkgd` (labelled callers are refused). The IDE blocks while the
  user finishes in the Installer.
- **Debugger.** Pipes to a spawned player work (polled non-blocking pipes; threads
  there do not share a descriptor table, so reader threads lose the pipe), so M4 reads
  the player's stdout on a timer, not on a thread, and the launcher seam
  (`Launcher::launch_with`, `ChildProcess::send_line`) lets a polling launcher write
  the player's stdin from that same timer. The LazyOS launchers implement both and
  start the player with `--debug`; the player side installs a non-blocking stdin
  transport with `lazyrad_player::debug::install_transport` because a reader thread
  there cannot see the process's stdin. See `lazyos/docs/lazyrad-plan.md` §8.
- **`std::env::current_exe()` is wrong on LazyOS** (it reports `/busybox`). Anything
  in LazyRAD that locates itself must go through the `Platform`'s player lookup, not
  `current_exe` (the packager and `player_beside` already do).
- **Open bugs seen there, not reproduced on the host:** typing a path into the painted
  `FileDialog` panics (`xui-core` `listview/api.rs`, `RefCell already borrowed`); the
  property grid did not repaint rows while scrolling in one probe; the IDE is slow on
  a large window (input reflected after seconds), though typing into a 2000-line module
  stays at 20–270 ms per key.
- **Not done there:** *Manage Apps* (list/remove produced apps), the `notify` and PCM
  `audio.beep` script modules, multi-window forms (G16).
- **LazyOS-only script modules** (`msg`/`sys::*` Messenger, `modplay`) are registered
  through `lazyrad_runtime::extensions::add_scoped` and stay in the LazyOS repo, which
  keeps LazyRAD platform-neutral.

### Porting LazyRAD

The rule: **no `cfg(target_os)` / `cfg(windows)` / `cfg(unix)`, no OS-specific crate and
no `std::os::*` outside the seam modules below and the `build.rs` files.** Everything
else calls the seam. Each seam has a portable fallback (what a new platform gets before
it implements anything), so a port compiles and runs first, then improves one module at a
time. CI checks Windows, Linux and macOS (`.github/workflows/ci.yml`).

**Seam modules and what a port must provide**

| Seam | Provides | Portable fallback |
|---|---|---|
| `lazyrad-ide/src/platform/dialogs.rs` | file open/save, folder dialogs, fatal-error box (`rfd`, feature `native-dialogs`) | every dialog answers "cancelled"; errors go to stderr. LazyOS supplies a painted xui dialog here |
| `lazyrad-ide/src/platform/theme.rs` | `system_prefers_dark()` (`dark-light`, Windows and macOS only) | light |
| `lazyrad-ide/src/platform/process.rs` | `hide_console_window` (`CREATE_NO_WINDOW`), `executable_file_name` | no flags, no suffix |
| `lazyrad-ide/src/platform/config.rs` | `config_dir` / `settings_file` (`directories`), default monospace font | no config dir (settings stay in memory), DejaVu Sans Mono |
| `lazyrad-project/src/fs_safety.rs` | `write_no_follow`, `is_symlink`: saves never follow a link out of the project (`O_NOFOLLOW` on Unix, reparse points on Windows) | check-then-open (slightly racy) |
| `lazyrad-packager/src/platform.rs` | `make_executable` (mode `0o755` on Unix) | no-op |
| `lazyrad-player/src/platform.rs` | `show_error`: why an exported GUI app did not start (`rfd`, feature `native-dialogs`) | stderr |
| `lazyrad-player/src/debug.rs` (`install_transport`) | how a `--debug` player reads commands: a `DebugTransport` over stdin/stdout | a reader thread on `std::io::stdin`, falling back to blocking reads on the calling thread when that thread cannot read (LazyOS installs a non-blocking one) |
| `lazyrad-ide/src/run.rs` (`Launcher::launch_with`, `ChildProcess::send_line`) | starting the player with `--debug` and writing its stdin without blocking | the default refuses `--debug` and cannot send, so a launcher that predates debugging still compiles and the IDE falls back to a plain run |
| `xui-code-editor` platform module | system clipboard (`arboard`) | in-process clipboard. Moving to the xui repo in issue #72 |

`--no-default-features` on `lazyrad-ide` and `lazyrad-player` builds without `rfd`, which
exercises the fallbacks.

**The `Platform` trait and the `desktop` features (LazyOS P0).** The seam modules
above no longer hard-code the host: they ask `lazyrad_runtime::platform::current()`, a
`Platform` trait (dialogs, config dir, default monospace font, theme preference, where
the player lives and how its `Command` is prepared, and the script file-access
policy). The desktop implementation is `lazyrad-ide/src/platform/host.rs`
(`HostPlatform`, the only file that names `rfd`, `directories` and `dark-light`); a port
installs its own with `platform::install` before starting. The system clipboard is not
part of the trait: xui routes it through the window backend (`Ui::clipboard_text`).
Everything that needs a desktop OS sits behind a `desktop` feature (default on) of
`lazyrad-runtime`, `lazyrad-player` and `lazyrad-ide`: the `winit` window backend
(`xui-canvas` is now `default-features = false`), `time`'s `local-offset`, `rfd`,
`directories` and `dark-light`. Without it the crates build for
`x86_64-unknown-linux-musl` and expose backend-taking entry points
(`lazyrad_player::run_with_backend`, `lazyrad_ide::run_with_backend`,
`lazyrad_runtime::shell::run_empty_window_with`). Rhai is pinned `=1.26.1` with
`default-features = false` (the exact pin LazyOS's `rhai-lazy` uses); `metadata`
(completion) is the runtime feature of that name and stays off in the LazyOS player.
Scripts reach files only through the platform's `FsPolicy` (`file_*`/`dir_*` stdlib
functions): unrestricted on the desktop, a sandbox on LazyOS. CI job `musl` checks all
of this.

**Inventory of platform-specific items**

- `cfg` attributes: `lazyrad-ide` `platform/{theme,process,config}.rs`;
  `lazyrad-project` `fs_safety.rs`; `lazyrad-packager` `platform.rs`. Windows-only resource
  embedding in `lazyrad-ide/build.rs` and `lazyrad-player/build.rs` (allowed).
- `#![windows_subsystem = "windows"]` in `lazyrad-ide/src/main.rs`: a crate attribute the
  compiler ignores elsewhere. It stays in the binary's entry point.
- Target-specific dependencies (`Cargo.toml` only): `dark-light` on Windows/macOS (it pulls
  D-Bus on Linux), `libc` on Unix (`O_NOFOLLOW`), `embed-resource` build-dependency on
  Windows, and `rfd` behind `native-dialogs` in the IDE and the player.
- OS calls: `CREATE_NO_WINDOW` (process seam), `O_NOFOLLOW` / `FILE_FLAG_OPEN_REPARSE_POINT`
  (fs seam), Unix file mode (packager seam), the native message box (dialog seams).
- Path and file-system assumptions: the executable suffix goes through
  `executable_file_name` and `EXE_EXTENSION`; the player is found next to the IDE
  executable; settings are atomic write-and-rename in the config directory; the export is
  an atomic write next to the destination. Patching a Windows stub's PE headers and
  resources uses `editpe`, pure Rust, so it runs on every host and needs no seam.
- Process creation: `lazyrad-ide/src/run.rs` spawns `lazyrad-player` with piped stdio through
  the process seam. Without process spawning (LazyOS before L4) see the in-process
  fallback above.
- `xui-code-editor` and its `arboard` dependency: handled by issue #72, not here.
- Test-only gating: symlink and Windows-resource tests under each crate's `tests/`
  use `cfg(unix)` / `cfg(windows)` because they exercise the platform directly.

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Rhai's C-like syntax feels nothing like BASIC to VB users | Accept it: LazyRAD takes VB6's workflow, not its language (§1.1). Good completion, templates and the double-click-to-handler flow carry the ease of use. Do not build a BASIC-to-Rhai transpiler |
| Rhai performance for tight loops | Fine for RAD apps. Ship native stdlib types for heavy work (sort, string building, `Dictionary`) |
| Scope creep in the editor | A monospace-only v1, no word wrap, no proportional fonts |
| xui churn (the project is young) | Pin the revision, keep a patch fork, upstream small PRs for G1/G2/G9/G10 |
| Blocking `msg_box` on canvas (G14, confirmed) | Ship a non-blocking `msg_box` with a callback now, and fix `run_modal` upstream |

---

## 14. Immediate next steps

The original steps (workspace, G11 spike, `lazyrad-project` and the Hello sample, M1)
are all done. What comes next, in order:

1. **M4 follow-ups** (the debugger itself is done, §7): watches and a Breakpoints list
   (1.x); a stop-line arrow glyph and Return-to-submit in `Edit` (both need xui
   changes); locals for outer frames (Rhai keeps no per-frame scope; it would need
   the callback to snapshot at call time); a way to stop a program that is idle
   between events (today Break waits for the next handler); a time budget on the
   per-node callback if a debug-mode game proves too slow. Exercise the rest of the
   debugger on a booted LazyOS image (§12 lists what its session did not).
2. **Finish M6 for v1.0:** go to definition and signature help; class modules and their
   template; `input_box` and the save/colour dialogs; Time (`DateTime`, `stopwatch`) and
   `clipboard`; the tab-order mode. Re-check G14 (xui#146) first, since it decides
   whether `msg_box` and `input_box` can block.
3. **Docs and housekeeping:** add `xui-form` and `xui-rhai` to the README's crate
   table; write a language and stdlib reference from the `metadata` doc comments.
4. **LazyOS (M7), now that the IDE runs there:** keep the xui pin in step with
   `lazyos/xui-app` (both repos bump together), and bump the LazyRAD `rev` in
   `lazyrad-os/Cargo.toml` to a commit that has the debugger (`python
   tools/xui/bump_pin.py`). Triage the findings in §12 ("Findings from the LazyOS
   port"); the file-dialog panic and the IDE's input latency are the ones a LazyOS
   user hits first. The debugger's transport is the launcher seam (§7, §12).
