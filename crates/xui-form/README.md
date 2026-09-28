# xui-form

Declarative forms for [xui](https://github.com/va1erian/xui).

`xui-form` describes a window's widgets, their properties, layout and event
bindings as data, validates that data against a typed schema, and builds live
`xui-core` widgets from it.

```text
            schema                 document                live widgets
Catalog ──────────────► FormDoc ──────────────► build() ──────────────► LiveForm
(what a kind     validate()   (plain TOML,     (Factories + Binder)
  supports)                     byte-stable)
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
| Building | `build`, `Factories`, `Binder`, `LiveWidget`, `LiveForm` | Live widgets and event wiring |

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
let text = form.get("cmdGo", "text");
form.relayout(xui_core::geometry::Size::new(800, 600));
# Ok(())
# }
```

`relayout` applies each node's `anchor` against the design size with
`xui_core::layout::anchored` and moves everything in one `Ui::apply_moves` call.
Call it from a resize handler.

## Design mode

`build_with(.., BuildOptions { design_mode: true })` consults no binder and
expects the host to have called `Ui::set_design_mode(true)`. That is how a form
designer renders a live preview.
