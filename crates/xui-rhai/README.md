# xui-rhai

Script [`xui-form`](../xui-form) forms with [Rhai](https://rhai.rs).

This crate is the reusable half of the LazyRAD runtime. It knows nothing about
LazyRAD projects: any xui app can load a `.lfm` form plus a `.rhai` script and
get a scriptable, declarative UI. See `examples/scripted_form.rs` (with
`hello.lfm` and `hello.rhai`) for the whole path.

It depends only on `xui-core`, `xui-form` and `rhai`.

## What it provides

| Module | Contents |
|---|---|
| `value` | The one place `xui-form` `Value` and Rhai `Dynamic` are converted. |
| `control` | Control handles over a live form (`name_edit.text`), the `form` object (`title`, `state`, `show`/`hide`), and the `FormHost` seam. |
| `engine` | `EngineHost`: the `on_var` resolver, located `ScriptError`s, the stop flag and operation budget, globals, `call_with`, `register_module` and `prepare`. |
| `form` | `ScriptBinder`, `handler_name`, `form_load`/`form_close`/`form_reload` and `ScriptForm`. |
| `error` | Locating parse and eval errors (file, line, column). |
| `message` | The `Msg` vocabulary a script's UI calls leave for the host. |

## The `on_var` push-into-scope trick

Rhai functions cannot see the enclosing scope, so `name_edit.text` inside
`fn hello_button_click()` would not resolve by itself. `EngineHost` installs an
`Engine::on_var` resolver that looks an unknown name up among the active form's
controls, then `form`, then the registered globals.

The resolver **pushes** the value into the scope instead of returning it. Rhai
marks a value returned from `on_var` read-only, so a setter such as
`label.text = "x"` would fail with a "cannot modify property of constant" error;
a pushed variable is an ordinary mutable entry and the setter works.

## Handler naming

A handler is `fn <control>_<event>(args…)` in snake_case, with xui's event name
lowercased: `hello_button_click`, `name_edit_change`, `agree_check_toggle`. The
form's own events use the `form` prefix: `form_load`, `form_close`. A host that
hot-reloads a form (issue #91) builds it with `ScriptForm::build_deferred`, then
`load`s it and calls `reload(old_state)`, which runs `fn form_reload(old_state)`
when the script defines it.

`ScriptBinder` consults the compiled script and wires an event only when the
matching function exists, so a missing handler is a silent no-op. `ScriptForm`
runs a handler with the event's typed arguments, padding extra parameters with
`()` because Rhai matches a function by name *and* arity.
