// The form's code-behind. Handlers are `<control>_<event>` in snake_case, and
// the form's own events use the `form` prefix (PLAN.md §1.1).

fn form_load() {
    result_label.text = greeting(name_edit.text);
}

fn greet_button_click() {
    result_label.text = greeting(name_edit.text);
}

// A form-level helper the handlers can call.
fn greeting(name) {
    `Hello, ${name}!`
}
