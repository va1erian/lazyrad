#![forbid(unsafe_code)]

//! Static lints for scripts that compile but fail at run time.
//!
//! Three mistakes compile cleanly and only surface when a handler runs, which
//! made building the MOD player sample slow (va1erian/lazyos#526):
//!
//! 1. a top-level `const` used inside a `fn` (Rhai functions cannot see the
//!    enclosing scope, unlike the controls, `form` and `app` the resolver
//!    supplies); `global::NAME` works;
//! 2. a script function whose name matches a method it calls on a value (Rhai
//!    resolves method calls to script functions too, so it recurses into
//!    itself);
//! 3. a `module::fn` call to a module neither the project nor a registered
//!    extension defines.
//!
//! The lints are warnings: they do not stop a project from running. They are
//! collected as located [`ScriptError`]s so the IDE and the packager can show
//! them with a line and column.
//!
//! The compiled AST is walked once. A function's body is recognised by its
//! source range (a script function exposes its body's start and end positions),
//! so a node's owner is the innermost function whose range contains it.
//! Comments and strings never become nodes, so no lint can fire on text inside
//! them.

use std::collections::{BTreeMap, BTreeSet};

use rhai::{AST, ASTFlags, ASTNode, Engine, EvalAltResult, Expr, Position, Stmt};

use crate::ScriptError;

/// The module a `module::fn` call may name even though the project does not
/// define it: Rhai's built-in `global` namespace.
const GLOBAL: &str = "global";

/// Every lint warning in `ast`, located against `file`.
///
/// `supplied` names the variables a form's script sees without declaring them
/// (its controls, `form` and `app`). `project_modules` names the project's own
/// modules; any other `module::fn` root is looked up in `engine` (which has the
/// host's extensions registered), so a `sys::…` call is accepted when the
/// player defines `sys`.
pub(crate) fn script(
    ast: &AST,
    file: &str,
    supplied: &BTreeSet<String>,
    project_modules: &BTreeSet<String>,
    engine: &Engine,
) -> Vec<ScriptError> {
    let consts = top_level_consts(ast);
    let functions = function_infos(ast);
    let locals = local_names(ast, &functions);
    let mut known_modules = BTreeMap::new();

    let mut lints = Vec::new();
    ast.walk(&mut |nodes| {
        let Some(node) = nodes.last() else {
            return true;
        };
        let position = node.position();
        let owner = owner_of(position, &functions);
        match node {
            ASTNode::Stmt(Stmt::FnCall(call, _)) | ASTNode::Expr(Expr::FnCall(call, _)) => {
                if call.is_qualified() {
                    let module = call.namespace.root();
                    if !module_known(module, project_modules, engine, &mut known_modules) {
                        lints.push(warn(
                            file,
                            position,
                            format!(
                                "`{module}::{}` names a module the project and its extensions \
                                 do not define",
                                call.name
                            ),
                        ));
                    }
                }
            }
            ASTNode::Expr(Expr::MethodCall(call, _)) => {
                if let Some(index) = owner
                    && call.name.as_str() == functions[index].name
                {
                    lints.push(warn(
                        file,
                        position,
                        format!(
                            "`fn {}` calls `.{}(…)` on a value, which Rhai resolves back to \
                             this function and recurses; rename the function or the method",
                            functions[index].name, call.name
                        ),
                    ));
                }
            }
            ASTNode::Expr(Expr::Variable(variable, _, _)) => {
                let name = variable.1.as_str();
                let visible = variable.2.is_empty();
                let Some(index) = owner else {
                    return true;
                };
                if visible
                    && consts.contains_key(name)
                    && !supplied.contains(name)
                    && !functions[index].params.contains(name)
                    && !locals[index].contains(name)
                {
                    lints.push(warn(
                        file,
                        position,
                        format!(
                            "`{name}` is a top-level constant, which a function cannot see; \
                             use `global::{name}` or keep the value in form.state"
                        ),
                    ));
                }
            }
            _ => {}
        }
        true
    });
    lints
}

/// The top-level `const` names and where each is declared.
fn top_level_consts(ast: &AST) -> BTreeMap<String, Position> {
    ast.statements()
        .iter()
        .filter_map(|statement| match statement {
            Stmt::Var(binding, options, _) if options.contains(ASTFlags::CONSTANT) => {
                Some((binding.0.name.to_string(), binding.0.pos))
            }
            _ => None,
        })
        .collect()
}

/// One script function's name, parameters and body source range.
struct FnInfo {
    name: String,
    params: BTreeSet<String>,
    start: Position,
    end: Position,
}

/// Every script function's signature and body range.
fn function_infos(ast: &AST) -> Vec<FnInfo> {
    ast.iter_fn_def()
        .map(|function| FnInfo {
            name: function.name.to_string(),
            params: function.params.iter().map(ToString::to_string).collect(),
            start: function.body.start_position(),
            end: function.body.end_position(),
        })
        .collect()
}

/// The `let`/`const` names declared in each function's body, in the same order
/// as [`function_infos`].
fn local_names(ast: &AST, functions: &[FnInfo]) -> Vec<BTreeSet<String>> {
    let mut locals: Vec<BTreeSet<String>> = vec![BTreeSet::new(); functions.len()];
    ast.walk(&mut |nodes| {
        if let Some(ASTNode::Stmt(Stmt::Var(binding, _, _))) = nodes.last()
            && let Some(index) = owner_of(binding.0.pos, functions)
        {
            locals[index].insert(binding.0.name.to_string());
        }
        true
    });
    locals
}

/// The index of the innermost function whose body contains `position`.
///
/// The innermost is the one with the greatest start (a closure nested in a
/// function starts later), which is the scope a name is actually resolved in.
fn owner_of(position: Position, functions: &[FnInfo]) -> Option<usize> {
    if position.is_none() {
        return None;
    }
    functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.start <= position && position <= function.end)
        .max_by_key(|(_, function)| function.start)
        .map(|(index, _)| index)
}

/// Whether `root` is a module a script may call into: the project's own, the
/// built-in `global`, or one the engine's registered extensions define.
///
/// The extensions are opaque, so a module is recognised by asking the engine to
/// resolve a call into it: a missing module is [`ErrorModuleNotFound`], while a
/// module that exists but has no such function is [`ErrorFunctionNotFound`].
/// The answers are cached, since one script usually calls a module repeatedly.
///
/// [`ErrorModuleNotFound`]: EvalAltResult::ErrorModuleNotFound
/// [`ErrorFunctionNotFound`]: EvalAltResult::ErrorFunctionNotFound
fn module_known(
    root: &str,
    project_modules: &BTreeSet<String>,
    engine: &Engine,
    cache: &mut BTreeMap<String, bool>,
) -> bool {
    if root == GLOBAL || project_modules.contains(root) {
        return true;
    }
    if let Some(known) = cache.get(root) {
        return *known;
    }
    let known = !matches!(
        engine.eval::<rhai::Dynamic>(&format!("{root}::__lazyrad_probe__()")),
        Err(error) if matches!(*error, EvalAltResult::ErrorModuleNotFound(..))
    );
    cache.insert(root.to_owned(), known);
    known
}

/// A located lint warning.
fn warn(file: &str, position: Position, message: impl Into<String>) -> ScriptError {
    ScriptError::new(file, position, format!("lint: {}", message.into()))
}
