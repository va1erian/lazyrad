# xui-code-editor

A reusable code-editor widget for [xui](https://github.com/va1erian/xui).

It is one `xui-core` `Custom` node with a painter and an event mapper, built
over pure modules: a `ropey`-backed text buffer with transactional undo, a
monospace-grid view, caret and selection, scrolling with scrollbars,
find/replace (plain or regex), diagnostics markers and incremental syntax
highlighting. It stays pure Rust, so it needs no C++ toolchain, and it can run
without any OS clipboard.

Highlighting is pluggable. The editor holds a `Highlighter` behind a box, so it
is not generic over the language. `PlainText` is the default and emits no
tokens; the hand-written, line-incremental Rhai lexer ships as `RhaiHighlighter`
behind the `rhai-syntax` feature. Bracket matching works with any highlighter.

## Embedding

```rust,no_run
use xui_core::app::Ui;
use xui_core::geometry::Rect;
use xui_code_editor::Editor;

fn build<M: 'static>(ui: &Ui<M>) -> xui_core::backend::Result<()> {
    let editor = Editor::new(ui, Rect::new(0, 0, 640, 400))?
        .on_change(|text| {
            // Return `Some(message)` to raise an app message, or `None`.
            let _ = text;
            None
        });
    editor.set_text("fn main() {\n}\n");
    editor.goto(1, 0);
    Ok(())
}
```

`Editor::new` gives a plain-text editor. To colour Rhai, enable the feature and
select the highlighter:

```rust,no_run
# use xui_core::app::Ui;
# use xui_core::geometry::Rect;
# use xui_code_editor::{Editor, RhaiHighlighter};
# fn build<M: 'static>(ui: &Ui<M>) -> xui_core::backend::Result<()> {
let editor = Editor::new(ui, Rect::new(0, 0, 640, 400))?
    .with_highlighter(RhaiHighlighter);
# let _ = editor;
# Ok(())
# }
```

Run the plain-text example on the canvas backend:

```text
cargo run -p xui-code-editor --example notepad
```

## Features

* `system-clipboard` (default): copy and paste through the OS clipboard with
  `arboard`.
* `rhai-syntax`: the `RhaiHighlighter`.

Build with `default-features = false` for a platform with no OS clipboard: the
editor then uses an in-process clipboard, which is enough for a single app (and
is what the tests use). To use your own clipboard, implement the `Clipboard`
trait and pass it through the editor's state; the `platform` module documents
the seam.

## Writing a highlighter

Implement `Highlighter`. It is line-incremental: given the `LineState` carried
from the previous line and the line's text, return this line's `Token`s and the
state to carry on. Pack your state into the opaque `LineState` (a raw `u64`);
the cache compares states to decide when a re-lex has settled into state the
rest of the file already had. Then select it with `Editor::with_highlighter` (or
`Editor::set_highlighter` on a live editor, which re-lexes the whole buffer).

`PlainText` is the whole interface in six lines of logic and a good template.

## Porting checklist

An app that embeds the editor needs to provide:

* An xui backend. `xui-canvas`'s `WinitBackend` on a desktop, or another
  `Backend` implementation on a smaller target.
* One monospace font, through `FontConfig`. The grid measures one advance and
  one line height per font and DPI; a proportional font will not line up.
* Timers for the caret blink. The editor starts its own per-widget timer
  (`Control::set_timer`) and stops it on drop; the host forwards nothing.
* Keyboard events with modifiers. The editor handles `KeyDown` for navigation,
  selection, editing and the usual shortcuts, and needs the backend to report
  Ctrl and Shift.
* A clipboard, or `default-features = false` for the in-process one, or your own
  `Clipboard` implementation.

## Limitations

* A monospace grid only; proportional fonts do not line up.
* No word wrap.
* No IME or composition events, so it cannot enter CJK text.
* Columns are char counts, so wide CJK characters and emoji do not line up with
  the grid; use a monospace font with predictable advances.
