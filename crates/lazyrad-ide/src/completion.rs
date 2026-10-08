#![forbid(unsafe_code)]

//! What the code editor offers to complete as you type a form script.
//!
//! [`complete`] looks at the word before the caret and what precedes it:
//!
//! * after `control.` it offers that control's properties (and methods) from
//!   the shared [`Catalog`], after `form.` and `app.` their members, and after
//!   any other value the common Rhai methods (`len`, `push`, `to_upper`…);
//! * after `module::` it offers the functions the project module defines, and
//!   after `global::` the script's top-level constants;
//! * anywhere else, the Rhai keywords, the standard library, the form's
//!   controls, `form` and `app`, the project's modules and the functions and
//!   variables the script itself declares.
//!
//! Nothing is offered inside a comment or a string. Candidates are filtered by
//! the typed prefix (case-insensitively; a prefix match ranks before a looser
//! subsequence match) so the editor can show the list as it comes.
//!
//! Everything here is pure text and data, unit-tested without a `Ui`; the IDE
//! keeps a [`ScriptContext`] up to date for each open code window.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use lazyrad_project::Catalog;
use lazyrad_runtime::{EngineHost, FormHost, StdlibContext};
use xui_code_editor::{CompletionItem, CompletionKind};
use xui_form::{SetError, Value, ValueType};

use crate::project::ProjectSession;

/// What kind of thing a candidate is; the editor shows it as a small marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    /// A control on the form, `form` or `app`.
    Object,
    /// A property of a control or object.
    Property,
    /// A method of a control, an object or a value.
    Method,
    /// A function: the standard library's or the script's own.
    Function,
    /// A variable or constant the script declares.
    Variable,
    /// A project module.
    Module,
    /// A Rhai keyword.
    Keyword,
}

/// One completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What the list shows and what is inserted.
    pub label: String,
    /// A short description: a signature or a control's kind.
    pub detail: Option<String>,
    /// What kind of thing it is.
    pub kind: ItemKind,
}

impl Item {
    fn new(label: impl Into<String>, kind: ItemKind, detail: Option<String>) -> Item {
        Item {
            label: label.into(),
            detail,
            kind,
        }
    }
}

/// The result of [`complete`]: the char offset where the word being completed
/// starts (the editor replaces `start..caret`) and the candidates, best first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// The char offset of the first character of the word being completed.
    pub start: usize,
    /// The candidates, best first.
    pub items: Vec<Item>,
}

/// A standard-library function: its name and a signature to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryFn {
    /// The function's name (`msg_box`).
    pub name: String,
    /// Its signature (`msg_box(text)`), shown as the candidate's detail.
    pub signature: String,
}

/// What a script in one code window can see besides its own text.
#[derive(Clone, Debug, Default)]
pub struct ScriptContext {
    /// The form's controls, as `(name, kind)`; empty for a module.
    pub controls: Vec<(String, String)>,
    /// The project's modules, each with the functions it defines.
    pub modules: Vec<(String, Vec<String>)>,
    /// The standard library's global functions.
    pub library: Vec<LibraryFn>,
    /// Whether the script is a form's (so `form` and the controls exist).
    pub is_form: bool,
}

/// The Rhai keywords offered as completions.
pub const KEYWORDS: &[&str] = &[
    "let",
    "const",
    "fn",
    "if",
    "else",
    "switch",
    "while",
    "loop",
    "for",
    "in",
    "do",
    "until",
    "break",
    "continue",
    "return",
    "throw",
    "try",
    "catch",
    "import",
    "export",
    "as",
    "private",
    "true",
    "false",
    "this",
    "global",
    "is_def_var",
    "is_def_fn",
    "type_of",
    "print",
    "debug",
];

/// The members of `form` in scripts.
const FORM_MEMBERS: &[(&str, ItemKind, &str)] = &[
    ("title", ItemKind::Property, "the window title"),
    ("state", ItemKind::Property, "a map kept between events"),
    ("show", ItemKind::Method, "show()"),
    ("hide", ItemKind::Method, "hide()"),
];

/// The members of `app` in scripts.
const APP_MEMBERS: &[(&str, ItemKind, &str)] = &[
    ("title", ItemKind::Property, "the project name"),
    ("path", ItemKind::Property, "the project folder"),
    ("quit", ItemKind::Method, "quit()"),
];

/// Common methods of Rhai's built-in values, offered after `value.` when the
/// value is not a known object.
const VALUE_METHODS: &[&str] = &[
    "abs",
    "ceiling",
    "chars",
    "clear",
    "contains",
    "ends_with",
    "filter",
    "floor",
    "index_of",
    "insert",
    "is_empty",
    "keys",
    "len",
    "map",
    "pad",
    "pop",
    "push",
    "reduce",
    "remove",
    "replace",
    "reverse",
    "round",
    "shift",
    "sort",
    "split",
    "starts_with",
    "sub_string",
    "to_float",
    "to_int",
    "to_lower",
    "to_string",
    "to_upper",
    "trim",
    "values",
];

/// The most candidates returned at once.
const MAX_ITEMS: usize = 200;

/// The completion for `text` with the caret at char offset `caret`, or `None`
/// when there is nothing to offer (inside a comment or string, or no
/// candidate matches).
pub fn complete(
    catalog: &Catalog,
    context: &ScriptContext,
    text: &str,
    caret: usize,
) -> Option<Completion> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    if in_comment_or_string(&chars[..caret]) {
        return None;
    }
    let start = word_start(&chars, caret);
    let prefix: String = chars[start..caret].iter().collect();
    // A word cannot start with a digit: `1.5` is not a member access.
    if prefix.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let candidates = match accessor(&chars, start) {
        Accessor::Member(receiver) => member_candidates(catalog, context, &receiver),
        Accessor::Path(module) => path_candidates(context, &module, text),
        Accessor::None => global_candidates(context, text, start),
    };
    let items = rank(candidates, &prefix);
    if items.is_empty() {
        return None;
    }
    Some(Completion { start, items })
}

/// What precedes the word being completed.
#[derive(Debug, PartialEq, Eq)]
enum Accessor {
    /// `receiver.` — a property or method access.
    Member(String),
    /// `module::` — a namespaced function or `global::` constant.
    Path(String),
    /// Nothing special: a bare identifier.
    None,
}

/// Classifies what precedes the word starting at `start`.
fn accessor(chars: &[char], start: usize) -> Accessor {
    if start >= 2 && chars[start - 1] == ':' && chars[start - 2] == ':' {
        let end = start - 2;
        let begin = word_start(chars, end);
        return Accessor::Path(chars[begin..end].iter().collect());
    }
    if start >= 1 && chars[start - 1] == '.' {
        let end = start - 1;
        let begin = word_start(chars, end);
        // `a..b` is a range, not a member access.
        if end >= 1 && chars[end - 1] == '.' {
            return Accessor::None;
        }
        return Accessor::Member(chars[begin..end].iter().collect());
    }
    Accessor::None
}

/// Whether `c` can be part of an identifier.
fn is_ident(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// The char offset where the identifier ending at `end` begins.
fn word_start(chars: &[char], end: usize) -> usize {
    let mut start = end;
    while start > 0 && is_ident(chars[start - 1]) {
        start -= 1;
    }
    start
}

/// Whether the end of `before` lies inside a comment or a string literal.
fn in_comment_or_string(before: &[char]) -> bool {
    !scan(before).1
}

/// Scans `chars` for code: which chars are code (outside every comment and
/// string, delimiters included), and whether the end of `chars` is code.
///
/// It follows Rhai's lexical rules closely enough for completion: `//` line
/// comments, nestable `/* */` block comments, `"…"` strings with `\`
/// escapes, `'…'` characters and `` `…` `` template strings, whose `${…}`
/// interpolations are code.
fn scan(chars: &[char]) -> (Vec<bool>, bool) {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        Line,
        Block(u32),
        Quote(char),
    }
    let mut state = State::Code;
    let mut mask = vec![false; chars.len()];
    // The brace depth inside each open `${…}` of a template string.
    let mut interpolations: Vec<u32> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        let mut width = 1;
        match state {
            State::Code => match (c, next) {
                ('/', Some('/')) => {
                    state = State::Line;
                    width = 2;
                }
                ('/', Some('*')) => {
                    state = State::Block(1);
                    width = 2;
                }
                ('"' | '\'' | '`', _) => state = State::Quote(c),
                ('{', _) => {
                    if let Some(depth) = interpolations.last_mut() {
                        *depth += 1;
                    }
                    mask[index] = true;
                }
                ('}', _) => match interpolations.last_mut() {
                    Some(0) => {
                        interpolations.pop();
                        state = State::Quote('`');
                    }
                    Some(depth) => {
                        *depth -= 1;
                        mask[index] = true;
                    }
                    None => mask[index] = true,
                },
                _ => mask[index] = true,
            },
            State::Line => {
                if c == '\n' {
                    state = State::Code;
                    mask[index] = true;
                }
            }
            State::Block(depth) => match (c, next) {
                ('*', Some('/')) => {
                    state = if depth == 1 {
                        State::Code
                    } else {
                        State::Block(depth - 1)
                    };
                    width = 2;
                }
                ('/', Some('*')) => {
                    state = State::Block(depth + 1);
                    width = 2;
                }
                _ => {}
            },
            State::Quote(quote) => match (c, next) {
                ('\\', Some(_)) if quote != '`' => width = 2,
                ('$', Some('{')) if quote == '`' => {
                    interpolations.push(0);
                    state = State::Code;
                    width = 2;
                }
                // A `"` or `'` string ends at the line's end even unclosed,
                // so one stray quote does not silence the rest of the file.
                ('\n', _) if quote != '`' => {
                    state = State::Code;
                    mask[index] = true;
                }
                _ if c == quote => state = State::Code,
                _ => {}
            },
        }
        index += width;
    }
    (mask, state == State::Code)
}

/// The candidates after `receiver.`.
fn member_candidates(catalog: &Catalog, context: &ScriptContext, receiver: &str) -> Vec<Item> {
    let members = |list: &[(&str, ItemKind, &str)]| {
        list.iter()
            .map(|(name, kind, detail)| Item::new(*name, *kind, Some((*detail).to_owned())))
            .collect()
    };
    if context.is_form && receiver == "form" {
        return members(FORM_MEMBERS);
    }
    if receiver == "app" {
        return members(APP_MEMBERS);
    }
    if context.is_form
        && let Some((_, kind)) = context.controls.iter().find(|(name, _)| name == receiver)
    {
        let items: Vec<Item> = catalog
            .property_names(kind)
            .into_iter()
            .map(|name| {
                let detail = catalog
                    .property(kind, &name)
                    .map(|spec| spec.description.clone())
                    .filter(|description| !description.is_empty());
                Item::new(name, ItemKind::Property, detail)
            })
            .collect();
        return items;
    }
    VALUE_METHODS
        .iter()
        .map(|name| Item::new(*name, ItemKind::Method, None))
        .collect()
}

/// The candidates after `module::`.
fn path_candidates(context: &ScriptContext, module: &str, text: &str) -> Vec<Item> {
    if module == "global" {
        return declarations(text)
            .into_iter()
            .filter(|(_, constant)| *constant)
            .map(|(name, _)| Item::new(name, ItemKind::Variable, Some("const".to_owned())))
            .collect();
    }
    context
        .modules
        .iter()
        .find(|(name, _)| name == module)
        .map(|(_, functions)| {
            functions
                .iter()
                .map(|name| Item::new(name.clone(), ItemKind::Function, None))
                .collect()
        })
        .unwrap_or_default()
}

/// The candidates for a bare identifier. `start` is where the word being typed
/// begins, so it is not offered as its own completion.
fn global_candidates(context: &ScriptContext, text: &str, start: usize) -> Vec<Item> {
    let mut items = Vec::new();
    if context.is_form {
        items.push(Item::new(
            "form",
            ItemKind::Object,
            Some("this form".to_owned()),
        ));
        for (name, kind) in &context.controls {
            items.push(Item::new(
                name.clone(),
                ItemKind::Object,
                Some(kind.clone()),
            ));
        }
    }
    items.push(Item::new(
        "app",
        ItemKind::Object,
        Some("the application".to_owned()),
    ));
    for (name, _) in &context.modules {
        items.push(Item::new(
            name.clone(),
            ItemKind::Module,
            Some("module".to_owned()),
        ));
    }
    for function in &context.library {
        items.push(Item::new(
            function.name.clone(),
            ItemKind::Function,
            Some(function.signature.clone()),
        ));
    }
    for (name, signature) in script_functions(text) {
        items.push(Item::new(name, ItemKind::Function, Some(signature)));
    }
    // The script's own variables, except the word being typed.
    let typed_end = text
        .chars()
        .skip(start)
        .take_while(|c| is_ident(*c))
        .count();
    let typed: String = text.chars().skip(start).take(typed_end).collect();
    for (name, constant) in declarations(text) {
        if name != typed {
            let detail = if constant { "const" } else { "let" };
            items.push(Item::new(name, ItemKind::Variable, Some(detail.to_owned())));
        }
    }
    for keyword in KEYWORDS {
        items.push(Item::new(*keyword, ItemKind::Keyword, None));
    }
    items
}

/// The identifiers of the script's code, each with the identifier before it,
/// skipping comments and strings.
fn code_words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new();
    let mut index = 0;
    let mask = scan(&chars).0;
    let code: Vec<char> = chars
        .iter()
        .zip(&mask)
        .map(|(c, code)| if *code { *c } else { ' ' })
        .collect();
    while index < code.len() {
        if is_ident(code[index]) {
            let begin = index;
            while index < code.len() && is_ident(code[index]) {
                index += 1;
            }
            words.push(code[begin..index].iter().collect());
        } else {
            if !code[index].is_whitespace() {
                words.push(code[index].to_string());
            }
            index += 1;
        }
    }
    words
}

/// The functions the script defines, with a signature: `(name, "name(a, b)")`.
fn script_functions(text: &str) -> Vec<(String, String)> {
    let words = code_words(text);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, word) in words.iter().enumerate() {
        if word != "fn" {
            continue;
        }
        let Some(name) = words.get(index + 1).filter(|name| is_identifier(name)) else {
            continue;
        };
        if words.get(index + 2).map(String::as_str) != Some("(") {
            continue;
        }
        let params: Vec<&str> = words[index + 3..]
            .iter()
            .take_while(|word| *word != ")")
            .filter(|word| is_identifier(word))
            .map(String::as_str)
            .collect();
        if seen.insert(name.clone()) {
            out.push((name.clone(), format!("{name}({})", params.join(", "))));
        }
    }
    out
}

/// The variables and constants the script declares, with whether each is a
/// constant, plus every function's parameters (as variables).
fn declarations(text: &str) -> Vec<(String, bool)> {
    let words = code_words(text);
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |name: &str, constant: bool, out: &mut Vec<(String, bool)>| {
        if is_identifier(name) && !KEYWORDS.contains(&name) && seen.insert(name.to_owned()) {
            out.push((name.to_owned(), constant));
        }
    };
    for (index, word) in words.iter().enumerate() {
        match word.as_str() {
            "let" | "const" => {
                if let Some(name) = words.get(index + 1) {
                    push(name, word == "const", &mut out);
                }
            }
            "for" => {
                // `for x in …` and `for (x, i) in …`.
                for name in words[index + 1..]
                    .iter()
                    .take_while(|word| *word != "in")
                    .filter(|word| is_identifier(word))
                {
                    push(name, false, &mut out);
                }
            }
            "fn" if words.get(index + 2).map(String::as_str) == Some("(") => {
                for name in words[index + 3..]
                    .iter()
                    .take_while(|word| *word != ")")
                    .filter(|word| is_identifier(word))
                {
                    push(name, false, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether `word` is an identifier (not a number or punctuation).
fn is_identifier(word: &str) -> bool {
    word.chars()
        .next()
        .is_some_and(|c| c == '_' || c.is_alphabetic())
        && word.chars().all(is_ident)
}

/// Filters `items` by `prefix` and orders them: case-sensitive prefix
/// matches, then case-insensitive ones, then subsequence matches; ties by
/// kind and name. Duplicate labels keep their first (most specific) entry,
/// and an exact match of the whole typed word is dropped when it is the only
/// candidate, since completing it would change nothing.
fn rank(items: Vec<Item>, prefix: &str) -> Vec<Item> {
    let lower = prefix.to_lowercase();
    let mut seen = BTreeSet::new();
    let mut scored: Vec<(u8, Item)> = items
        .into_iter()
        .filter(|item| seen.insert(item.label.clone()))
        .filter_map(|item| {
            let label = item.label.to_lowercase();
            let score = if item.label.starts_with(prefix) {
                0
            } else if label.starts_with(&lower) {
                1
            } else if is_subsequence(&lower, &label) {
                2
            } else {
                return None;
            };
            Some((score, item))
        })
        .collect();
    scored.sort_by(|(a_score, a), (b_score, b)| {
        a_score
            .cmp(b_score)
            .then(a.kind.cmp(&b.kind))
            .then(a.label.cmp(&b.label))
    });
    let mut items: Vec<Item> = scored.into_iter().map(|(_, item)| item).collect();
    if items.len() == 1 && items[0].label == prefix {
        items.clear();
    }
    items.truncate(MAX_ITEMS);
    items
}

/// Whether every char of `needle` appears in `haystack` in order.
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack.chars();
    needle.chars().all(|c| haystack.any(|h| h == c))
}

/// The standard library's global functions, read from `engine`'s metadata:
/// the script-callable functions the LazyRAD stdlib registers, each once,
/// with a signature built from its parameter names.
pub fn library_functions(metadata_json: &str) -> Vec<LibraryFn> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for function in json["functions"].as_array().into_iter().flatten() {
        let Some(name) = function["name"].as_str() else {
            continue;
        };
        // Operators, property accessors and internal helpers are not
        // something a script types by name.
        if !is_identifier(name) || name.starts_with("get$") || name.starts_with("set$") {
            continue;
        }
        if function["namespace"].as_str() != Some("global") {
            continue;
        }
        if !seen.insert(name.to_owned()) {
            continue;
        }
        let params: Vec<String> = function["params"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|param| param["name"].as_str())
            .map(|param| param.split(':').next().unwrap_or(param).trim().to_owned())
            .filter(|param| !param.is_empty() && param != "_")
            .collect();
        out.push(LibraryFn {
            name: name.to_owned(),
            signature: format!("{name}({})", params.join(", ")),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The code editor's completer for one code window: [`complete`] over the
/// shared catalog and the window's [`ScriptContext`], which the IDE keeps
/// current (see [`context_for`]).
pub struct ScriptCompleter {
    catalog: Rc<Catalog>,
    context: Rc<RefCell<ScriptContext>>,
}

impl ScriptCompleter {
    /// A completer reading `context`, which the caller keeps up to date.
    pub fn new(catalog: Rc<Catalog>, context: Rc<RefCell<ScriptContext>>) -> ScriptCompleter {
        ScriptCompleter { catalog, context }
    }
}

impl xui_code_editor::Completer for ScriptCompleter {
    fn complete(&self, text: &str, caret: usize) -> Option<xui_code_editor::Completion> {
        // The IDE only replaces the context between events, never while the
        // editor is asking, so this borrow cannot collide.
        let context = self.context.borrow();
        let found = complete(&self.catalog, &context, text, caret)?;
        Some(xui_code_editor::Completion {
            start: found.start,
            items: found.items.into_iter().map(editor_item).collect(),
        })
    }
}

/// An [`Item`] as the editor's popup shows it.
fn editor_item(item: Item) -> CompletionItem {
    let kind = match item.kind {
        ItemKind::Object | ItemKind::Variable => CompletionKind::Variable,
        ItemKind::Property => CompletionKind::Property,
        ItemKind::Method => CompletionKind::Method,
        ItemKind::Function => CompletionKind::Function,
        ItemKind::Module => CompletionKind::Module,
        ItemKind::Keyword => CompletionKind::Keyword,
    };
    let completion = CompletionItem::new(item.label, kind);
    match item.detail {
        Some(detail) => completion.with_detail(detail),
        None => completion,
    }
}

/// What the script of the project item `name` can see: the form's controls
/// (for a form), the project's modules with their functions, and `library`.
pub fn context_for(session: &ProjectSession, name: &str, library: &[LibraryFn]) -> ScriptContext {
    let form = session.form(name);
    let controls = form
        .map(|form| {
            form.nodes
                .iter()
                .map(|node| (node.name.clone(), node.kind.clone()))
                .collect()
        })
        .unwrap_or_default();
    let modules = session
        .module_names()
        .into_iter()
        .filter(|module| *module != name)
        .map(|module| {
            let code = session.code(module).unwrap_or_default();
            let functions = exported_functions(code);
            (module.to_owned(), functions)
        })
        .collect();
    ScriptContext {
        controls,
        modules,
        library: library.to_vec(),
        is_form: form.is_some(),
    }
}

/// The functions a module exports: every `fn` not marked `private`.
fn exported_functions(code: &str) -> Vec<String> {
    let words = code_words(code);
    script_functions(code)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| {
            !words
                .windows(3)
                .any(|w| w[0] == "private" && w[1] == "fn" && w[2] == *name)
        })
        .collect()
}

/// The LazyRAD standard library's functions, read once from a headless
/// engine's metadata (only what LazyRAD registers, not Rhai's built-ins).
pub fn stdlib_functions(catalog: &Catalog) -> Vec<LibraryFn> {
    let host = EngineHost::new(
        Rc::new(NoForm),
        catalog,
        "completion.rhai",
        StdlibContext::headless("main_form"),
    );
    host.engine()
        .gen_fn_metadata_to_json(false)
        .map(|json| library_functions(&json))
        .unwrap_or_default()
}

/// A form with no controls: enough to build an engine and read its stdlib.
struct NoForm;

impl FormHost for NoForm {
    fn get(&self, _control: &str, _property: &str) -> Option<Value> {
        None
    }
    fn set(&self, _control: &str, _property: &str, _value: &Value) -> Result<(), SetError> {
        Err(SetError::UnknownWidget)
    }
    fn property_type(&self, _control: &str, _property: &str) -> Option<ValueType> {
        None
    }
    fn names(&self) -> Vec<String> {
        Vec::new()
    }
    fn kind(&self, _control: &str) -> Option<String> {
        None
    }
    fn property_names(&self, _control: &str) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stdlib_is_offered_from_the_real_engine() {
        let library = stdlib_functions(&catalog());
        let names: Vec<&str> = library.iter().map(|f| f.name.as_str()).collect();
        for name in ["msg_box", "now", "random"] {
            assert!(names.contains(&name), "{name} in {names:?}");
        }
        assert!(!names.contains(&"len"), "Rhai built-ins are left out");
    }

    #[test]
    fn private_module_functions_are_not_exported() {
        let code = "fn greet() {}\nprivate fn helper() {}\n// fn hidden() {}";
        assert_eq!(exported_functions(code), vec!["greet".to_owned()]);
    }

    #[test]
    fn the_editor_adapter_maps_kinds_and_details() {
        use xui_code_editor::Completer;
        let completer =
            ScriptCompleter::new(Rc::new(catalog()), Rc::new(RefCell::new(form_context())));
        let found = completer.complete("ms", 2).expect("a completion");
        assert_eq!(found.start, 0);
        let first = &found.items[0];
        assert_eq!(first.label, "msg_box");
        assert_eq!(first.insert, "msg_box");
        assert_eq!(first.kind, CompletionKind::Function);
        assert_eq!(first.detail.as_deref(), Some("msg_box(text)"));
    }

    fn catalog() -> Catalog {
        lazyrad_project::lazyrad_catalog()
    }

    fn form_context() -> ScriptContext {
        ScriptContext {
            controls: vec![
                ("hello_button".to_owned(), "Button".to_owned()),
                ("name_edit".to_owned(), "Edit".to_owned()),
            ],
            modules: vec![(
                "util".to_owned(),
                vec!["greet".to_owned(), "shout".to_owned()],
            )],
            library: vec![LibraryFn {
                name: "msg_box".to_owned(),
                signature: "msg_box(text)".to_owned(),
            }],
            is_form: true,
        }
    }

    /// Completes `text`, whose caret is marked with `|`.
    fn at(text: &str) -> Option<Completion> {
        let caret = text.chars().position(|c| c == '|').expect("a caret");
        let text = text.replacen('|', "", 1);
        complete(&catalog(), &form_context(), &text, caret)
    }

    fn labels(text: &str) -> Vec<String> {
        at(text)
            .map(|completion| {
                completion
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_bare_prefix_offers_globals_controls_and_keywords() {
        let found = labels("fn f() {\n    he|\n}");
        assert!(found.contains(&"hello_button".to_owned()));
        let found = labels("ms|");
        assert_eq!(found.first().map(String::as_str), Some("msg_box"));
        let found = labels("wh|");
        assert!(found.contains(&"while".to_owned()));
    }

    #[test]
    fn the_word_start_is_where_the_typed_word_begins() {
        let completion = at("let x = ms|").expect("a completion");
        assert_eq!(completion.start, 8);
    }

    #[test]
    fn a_control_member_offers_its_properties() {
        let found = labels("name_edit.|");
        assert!(found.contains(&"text".to_owned()), "{found:?}");
        assert!(found.contains(&"enabled".to_owned()));
        let found = labels("name_edit.te|");
        assert_eq!(found.first().map(String::as_str), Some("text"));
        assert!(
            !found.contains(&"while".to_owned()),
            "no keywords after a dot"
        );
    }

    #[test]
    fn form_and_app_offer_their_members() {
        assert!(labels("form.|").contains(&"state".to_owned()));
        assert!(labels("app.|").contains(&"quit".to_owned()));
    }

    #[test]
    fn an_unknown_value_offers_the_common_methods() {
        let found = labels("let s = \"a\";\ns.to_u|");
        assert_eq!(found, vec!["to_upper".to_owned()]);
    }

    #[test]
    fn a_module_path_offers_its_functions_and_global_its_constants() {
        assert_eq!(labels("util::sh|"), vec!["shout".to_owned()]);
        let found = labels("const LIMIT = 3;\nfn f() { global::|");
        assert_eq!(found, vec!["LIMIT".to_owned()]);
        assert!(labels("nothing::|").is_empty());
    }

    #[test]
    fn the_scripts_own_functions_and_variables_are_offered() {
        let text = "fn add_item(label, count) {}\nlet total = 0;\nad|";
        let completion = at(text).expect("a completion");
        let item = completion
            .items
            .iter()
            .find(|item| item.label == "add_item")
            .expect("the function");
        assert_eq!(item.detail.as_deref(), Some("add_item(label, count)"));
        assert!(labels("let total = 0;\nto|").contains(&"total".to_owned()));
        assert!(labels("fn f(count) {\n co|").contains(&"count".to_owned()));
        assert!(labels("for item in list {\n it|").contains(&"item".to_owned()));
    }

    #[test]
    fn nothing_is_offered_in_comments_or_strings() {
        assert!(at("// ms|").is_none());
        assert!(at("/* ms|").is_none());
        assert!(at("/* /* */ ms|").is_none(), "block comments nest");
        assert!(at("let s = \"ms|").is_none());
        assert!(at("let s = \"a\\\"ms|").is_none(), "an escaped quote");
        assert!(at("let s = `ms|").is_none());
        assert!(at("let s = `${ms|").is_some(), "interpolations are code");
        assert!(at("let s = \"a\"; ms|").is_some());
        assert!(at("/* x */ ms|").is_some());
        assert!(
            at("let s = `${#{a: 1}.a} ms|").is_none(),
            "back in the string"
        );
    }

    #[test]
    fn declarations_inside_comments_and_strings_are_ignored() {
        let found = labels("// let hidden = 1;\nlet s = \"let quoted = 2\";\nhi|");
        assert!(!found.contains(&"hidden".to_owned()));
        let found = labels("let s = \"let quoted = 2\";\nqu|");
        assert!(!found.contains(&"quoted".to_owned()));
    }

    #[test]
    fn numbers_and_ranges_are_not_member_accesses() {
        assert!(at("let x = 1.|").is_some_and(|c| c.items.iter().all(|i| i.label != "text")));
        assert!(at("let x = 15|").is_none(), "a number is not a word");
        let found = labels("for i in 0..|");
        assert!(!found.contains(&"to_upper".to_owned()), "{found:?}");
    }

    #[test]
    fn matching_is_case_insensitive_then_by_subsequence() {
        let found = labels("MSG|");
        assert_eq!(found.first().map(String::as_str), Some("msg_box"));
        let found = labels("hbtn|");
        assert_eq!(found, vec!["hello_button".to_owned()]);
    }

    #[test]
    fn an_exact_single_match_is_not_offered() {
        assert!(at("util::shout|").is_none());
    }

    #[test]
    fn multibyte_text_before_the_caret_uses_char_offsets() {
        let text = "let é = \"ü\"; ms|";
        let completion = at(text).expect("a completion");
        assert_eq!(
            completion.start,
            text.chars().position(|c| c == 'm').unwrap()
        );
    }

    #[test]
    fn empty_text_and_out_of_range_carets_are_safe() {
        assert!(complete(&catalog(), &form_context(), "", 0).is_some());
        assert!(complete(&catalog(), &form_context(), "ms", 99).is_some());
    }

    #[test]
    fn a_module_script_has_no_form_or_controls() {
        let context = ScriptContext {
            is_form: false,
            ..form_context()
        };
        let found: Vec<String> = complete(&catalog(), &context, "fo", 2)
            .map(|c| c.items.into_iter().map(|i| i.label).collect())
            .unwrap_or_default();
        assert!(!found.contains(&"form".to_owned()));
        assert!(found.contains(&"for".to_owned()));
        assert!(
            complete(&catalog(), &context, "form.", 5)
                .is_some_and(|c| c.items.iter().all(|i| i.label != "state"))
        );
    }

    #[test]
    fn the_library_is_read_from_engine_metadata() {
        let json = r#"{"functions":[
            {"name":"msg_box","namespace":"global","params":[{"name":"text: &str"}]},
            {"name":"msg_box","namespace":"global","params":[{"name":"text"},{"name":"title"}]},
            {"name":"+","namespace":"global","params":[]},
            {"name":"get$text","namespace":"global","params":[]},
            {"name":"inner","namespace":"internal","params":[]}
        ]}"#;
        let library = library_functions(json);
        assert_eq!(
            library,
            vec![LibraryFn {
                name: "msg_box".to_owned(),
                signature: "msg_box(text)".to_owned(),
            }]
        );
        assert!(library_functions("not json").is_empty());
    }
}
