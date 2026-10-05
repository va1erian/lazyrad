# xui-form

Declarative forms for [xui](https://github.com/va1erian/xui).

`xui-form` describes a window's widgets, their properties, layout and event
bindings as data, validates that data against a typed schema, and builds live
`xui-core` widgets from it.

```text
            schema                 document                live widgets
Catalog ──────────────► FormDoc ──────────────► build() ──────────────► LiveForm
(what a kind     validate()   (plain TOML,     (Factories + Binder,
  supports)                     byte-stable)     absolute() layouts)
```

## Why it is separate

The crate is designed to be **moved into the xui repository unchanged** once it
stabilises. It depends only on:

* `xui-core` — for the portable widgets, `Ui`, `Anchor` and `Color`;
* `serde` and `toml` — for the document and the serialisable catalog;
* `thiserror` — for typed errors.

It knows nothing about any particular application. A consumer adds
application-specific vocabulary by registering its own widget kinds or by
aliasing a built-in one:

```rust
let mut catalog = Catalog::xui();
catalog.alias("CommandButton", "Button"); // expose `Button` under a VB name
```

## What it provides

| Piece | Type | Role |
|---|---|---|
| Values | `Value`, `ValueType` | Schema-guided decoding of plain TOML literals |
| Schema | `Catalog`, `WidgetSpec`, `PropertySpec`, `EventSpec` | One source of truth for the designer, validation and completion |
| Document | `FormDoc`, `Node`, `WindowNode` | Flat, `parent = "…"`-referenced, byte-stable round trip |
| Validation | `FormDoc::validate`, `Diagnostic` | Every problem at once, with severities |
| Building | `build`, `Factories`, `Made`, `Binder`, `LiveWidget`, `LiveForm` | Live widgets, their layout and event wiring |

## Example

```rust,no_run
use xui_form::{Catalog, FormDoc, Factories, Binder, build};

let catalog = Catalog::xui();
let doc = FormDoc::from_toml(text, &catalog)?;
let factories = Factories::xui();
// let form = build(ui, &doc, &catalog, &factories, &my_binder)?;
```

A complete, runnable form is in [`examples/login.rs`](examples/login.rs).

## The document format

```toml
format = 1

[window]
name = "frmMain"
title = "Hello"

[[node]]
kind = "Button"
name = "cmdGo"
text = "Go"
```

* The list of nodes is **flat**; a child names its container with
  `parent = "panMain"`. Flat lists diff far better than nested tables.
* Saving is deterministic: fixed key order (`kind`, `name`, `parent`, then the
  common properties, then the rest alphabetically) and **only non-default
  values**. A value written explicitly equal to its default is dropped on save
  and is re-supplied by the schema on load.
* Loading and saving a canonical file is byte-identical.

## Using the live form

`LiveForm` owns every widget:

```rust,no_run
# use xui_form::{LiveForm, Value, SetError};
# fn demo(form: &LiveForm<()>) -> Result<(), SetError> {
form.set("cmdGo", "text", &Value::Text("Go!".into()))?;
form.set("cmdGo", "left", &Value::Int(24))?; // moves the button
let text = form.get("cmdGo", "text");
# Ok(())
# }
```

The form is mounted as `xui_core::arrange::absolute()` layouts: one for the
window (or the container node named by `BuildOptions::container`) and one inside
each container node. Every node sits at its `left`/`top`/`width`/`height` and
follows its `anchor` as the window or container grows or shrinks from its
design size, so the form re-anchors itself on every resize. A geometry or
anchor edit through `LiveForm::set` mounts the affected level again over the
same widgets; `LiveForm::batch` applies a run of edits in one pass.

A factory describes its widget with an `arrange` builder bound to a `Handle`
and returns both as a `Made`, with the `LiveWidget` that reaches the widget
through the handle once the form is mounted.

## Design mode

`build_with(.., BuildOptions { design_mode: true, .. })` consults no binder and
expects the host to have put the form's container in design mode
(`Ui::set_design_mode(true)` on its handle). That is how a form designer renders
a live preview; it calls `LiveForm::set_design_size` as the user resizes the
form, so the nodes stay where they were drawn.
