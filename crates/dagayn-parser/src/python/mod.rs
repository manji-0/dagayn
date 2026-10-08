//! The Python extractor, on the syntax tree of Ruff's parser
//! (`ruff_python_parser`): definitions, imports, calls, references, and the
//! receiver types of member calls, for `.py` files, notebooks, marimo apps,
//! and Databricks exports.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use ruff_python_ast::visitor::source_order::{SourceOrderVisitor, walk_expr, walk_stmt};
use ruff_python_ast::{self as ast, Decorator, Expr, ExprContext, Stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};
use serde_json::{Value, json};

use super::documentation_directives::{
    extract_line_comment_dagayn_directives, nearest_documentation_source,
    push_documentation_directive_edge,
};
use super::member_calls::{CallOrigin, MemberCallBindings};
use super::stdlib::python::{
    is_python_builtin, python_constructs_stdlib_value, python_stdlib_package,
};
use super::stdlib::{StdlibEvidence, mark_external_edge, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count};
use super::{qualify, resolve_rust_call_targets};

mod source;

use source::*;

mod notebook;

use notebook::*;
pub(super) use notebook::{looks_like_marimo_md, parse_marimo_md_with_parser, parse_notebook};

mod bridges;

use bridges::*;

mod receivers;

use receivers::*;

pub(super) fn parse_python(
    file_path: &str,
    source: &[u8],
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    if is_databricks_py_source(source) {
        return parse_databricks_py(&file_path, source, repo_root);
    }
    if looks_like_marimo_py(source) {
        return parse_marimo_py(&file_path, source, repo_root);
    }

    parse_python_module(&file_path, source, repo_root)
}

fn parse_python_module(
    file_path: &FilePath,
    source: &[u8],
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let src = PySource::new(source);
    let parsed = src.parse();
    parse_python_module_tree(
        file_path,
        source,
        &src,
        &parsed.syntax().body,
        parsed.has_syntax_errors(),
        repo_root,
    )
}

fn parse_python_module_tree(
    file_path: &FilePath,
    source: &[u8],
    src: &PySource<'_>,
    body: &[Stmt],
    recovered: bool,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(file_path, line_end, "python")];
    let mut edges = Vec::new();

    let (import_map, top_level_defined_names, protocol_names) =
        collect_python_file_scope(body, src);
    let (class_names, defined_names) = collect_python_defined_names(body);
    let import_aliases = collect_python_import_aliases(body);
    let stdlib_aliases = python_stdlib_aliases(&import_aliases, file_path, repo_root);
    let external_aliases = python_external_aliases(&import_aliases, file_path, repo_root);
    let attribute_types = collect_python_attribute_types(body, src);
    let context = PythonParseContext {
        src,
        recovered,
        file_path: file_path.clone(),
        repo_root,
        import_map: &import_map,
        top_level_defined_names: &top_level_defined_names,
        protocol_names: &protocol_names,
        import_aliases: &import_aliases,
        stdlib_aliases: &stdlib_aliases,
        external_aliases: &external_aliases,
        defined_names: &defined_names,
        class_names: &class_names,
        attribute_types: &attribute_types,
        pytest_file: is_test_file(file_path.as_str())
            || file_path.as_str().rsplit('/').next() == Some("conftest.py"),
        bindings: RefCell::new(MemberCallBindings::with_types(class_names.clone())),
    };
    let mut walker = PythonWalker {
        context: &context,
        enclosing_class: None,
        enclosing_qualified: None,
        class_bases: Vec::new(),
        scopes: Vec::new(),
        function_depth: 0,
        type_checking_depth: 0,
        deferred_depth: 0,
        nodes: &mut nodes,
        edges: &mut edges,
    };
    walker.visit_body(body);
    python_keep_checked_references(file_path, &nodes, &mut edges);
    python_emit_lazy_exports(body, &context, &mut edges);
    extract_python_documentation_directives(file_path, src.text(), &nodes, &mut edges);
    let edges = resolve_python_call_targets(&nodes, edges, file_path);
    let edges = add_python_tested_by_edges(&nodes, edges, file_path);
    (nodes, edges)
}

fn extract_python_documentation_directives(
    file_path: &FilePath,
    text: &str,
    nodes: &[ParsedNode],
    edges: &mut Vec<ParsedEdge>,
) {
    for directive in extract_line_comment_dagayn_directives(text, &["#"]) {
        let source = nearest_documentation_source(file_path, nodes, directive.line);
        push_documentation_directive_edge(
            edges,
            source,
            file_path,
            "python",
            &directive,
            "comment_directive",
        );
    }
}

/// `from <module> import <name> [as <bound>]`: bound name -> (module, name
/// in the module).
type ImportMap = HashMap<String, (String, String)>;

struct PythonParseContext<'a> {
    src: &'a PySource<'a>,
    /// The parse recovered from syntax errors.
    recovered: bool,
    file_path: FilePath,
    repo_root: Option<&'a Path>,
    /// Top-level `from` imports.
    import_map: &'a ImportMap,
    top_level_defined_names: &'a HashSet<String>,
    protocol_names: &'a HashSet<String>,
    /// Local names bound by an import anywhere in the file, including the
    /// function-level imports `import_map` leaves out. A call on one of them
    /// (`_core.parse(...)`) records the receiver so native-binding
    /// resolution can tell which module the attribute came from.
    import_aliases: &'a HashMap<String, String>,
    /// The import aliases that name the standard library (and no module of
    /// this repository): local name -> (package, what it names), `run` ->
    /// (`subprocess`, `subprocess.run`).
    stdlib_aliases: &'a HashMap<String, (&'static str, String)>,
    /// The import aliases that name a module neither of this repository nor
    /// of the standard library (`yaml`, `np` of `import numpy as np`): local
    /// name -> (package, what it names). Only known with a repository root.
    external_aliases: &'a HashMap<String, (String, String)>,
    /// Functions and classes declared anywhere in the file, which shadow the
    /// builtins of the same name.
    defined_names: &'a HashSet<String>,
    /// Classes declared anywhere in the file.
    class_names: &'a HashSet<String>,
    /// The types a class gives the attributes of `self`.
    attribute_types: &'a AttributeTypes,
    /// A test file or `conftest.py`, whose functions pytest calls with its
    /// fixtures.
    pytest_file: bool,
    /// Variables bound to a standard-library value (`p = Path(x)`) are
    /// `foreign` bindings to the name it resolves to (`pathlib.Path`).
    bindings: RefCell<MemberCallBindings>,
}

impl PythonParseContext<'_> {
    fn text<T: Ranged>(&self, node: &T) -> String {
        self.src.slice(node).to_string()
    }

    fn line<T: Ranged>(&self, node: &T) -> i64 {
        self.src.start_line(node)
    }
}

/// Walks a module in source order, emitting definitions, imports, calls,
/// and references, and tracking the receiver bindings of member calls.
struct PythonWalker<'w, 'a> {
    context: &'w PythonParseContext<'a>,
    /// Dotted path of the innermost enclosing class (`Outer.Inner`).
    enclosing_class: Option<String>,
    /// Qualified name of the innermost enclosing class or function.
    enclosing_qualified: Option<String>,
    /// The base names of each enclosing class, innermost last.
    class_bases: Vec<Vec<String>>,
    /// The definitions being walked, outermost first, by the column of
    /// their keyword. Only kept for a file with syntax errors (see
    /// [`PythonWalker::visit_stmt`]).
    scopes: Vec<WalkerScope>,
    /// How many function bodies enclose the statement being walked; an
    /// import inside one runs when the function is called, not on import.
    function_depth: u32,
    /// How many `if TYPE_CHECKING:` bodies enclose it; an import inside one
    /// never runs.
    type_checking_depth: u32,
    /// How many branches, loops, `except` handlers, and lambdas enclose
    /// it; a module-level call inside one may not run on import.
    deferred_depth: u32,
    nodes: &'w mut Vec<ParsedNode>,
    edges: &'w mut Vec<ParsedEdge>,
}

/// A definition being walked: its keyword's column, and the walker's
/// state outside it.
struct WalkerScope {
    column: u32,
    enclosing_class: Option<String>,
    enclosing_qualified: Option<String>,
    class_bases: Vec<Vec<String>>,
    function_depth: u32,
}

impl<'ast> SourceOrderVisitor<'ast> for PythonWalker<'_, '_> {
    /// After a syntax error, Ruff's parser may keep the statements that
    /// follow in the body of the definition the error is in, whatever their
    /// indentation (an unexpected indent nests the rest of the file one
    /// level deeper). A statement indented no deeper than the keyword of an
    /// enclosing definition is not in it: it is walked in the scope its
    /// indentation puts it in.
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if !self.scopes.is_empty() {
            let column = self.context.src.column(stmt.start());
            if let Some(depth) = self.scopes.iter().position(|scope| scope.column >= column) {
                let inner = self.scopes.split_off(depth);
                let outer = &inner[0];
                let enclosing_class =
                    std::mem::replace(&mut self.enclosing_class, outer.enclosing_class.clone());
                let enclosing_qualified = std::mem::replace(
                    &mut self.enclosing_qualified,
                    outer.enclosing_qualified.clone(),
                );
                let class_bases =
                    std::mem::replace(&mut self.class_bases, outer.class_bases.clone());
                let function_depth =
                    std::mem::replace(&mut self.function_depth, outer.function_depth);
                self.visit_statement(stmt);
                self.enclosing_class = enclosing_class;
                self.enclosing_qualified = enclosing_qualified;
                self.class_bases = class_bases;
                self.function_depth = function_depth;
                self.scopes.extend(inner);
                return;
            }
        }
        self.visit_statement(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match expr {
            Expr::Call(call) => {
                self.emit_call(call);
                walk_expr(self, expr);
            }
            Expr::Lambda(_) => self.deferred(|walker| walk_expr(walker, expr)),
            Expr::Dict(dict) => {
                // `{"key": handler}`: a value naming a function or class of
                // this file or an import is a reference to it.
                for item in &dict.items {
                    if item.key.is_some()
                        && let Expr::Name(name) = &item.value
                    {
                        self.emit_value_reference(name);
                    }
                    if let Some(key) = &item.key {
                        self.visit_expr(key);
                    }
                    self.visit_expr(&item.value);
                }
            }
            // `{key: handler for key in keys}`
            Expr::DictComp(comprehension) => {
                if let Expr::Name(name) = &*comprehension.value {
                    self.emit_value_reference(name);
                }
                walk_expr(self, expr);
            }
            Expr::List(list) if matches!(list.ctx, ExprContext::Load) => {
                for element in &list.elts {
                    if let Expr::Name(name) = element {
                        self.emit_value_reference(name);
                    }
                }
                walk_expr(self, expr);
            }
            // `[("dfs", _dfs), ...]`, `handlers = (on_a, on_b)`.
            Expr::Tuple(tuple) if matches!(tuple.ctx, ExprContext::Load) => {
                for element in &tuple.elts {
                    if let Expr::Name(name) = element {
                        self.emit_value_reference(name);
                    }
                }
                walk_expr(self, expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}

impl PythonWalker<'_, '_> {
    /// Enters a definition whose keyword is at `start`, when the file has
    /// syntax errors.
    fn push_scope(&mut self, start: TextSize) {
        if self.context.recovered {
            self.scopes.push(WalkerScope {
                column: self.context.src.column(start),
                enclosing_class: self.enclosing_class.clone(),
                enclosing_qualified: self.enclosing_qualified.clone(),
                class_bases: self.class_bases.clone(),
                function_depth: self.function_depth,
            });
        }
    }

    fn pop_scope(&mut self) {
        if self.context.recovered {
            self.scopes.pop();
        }
    }

    /// The end of a definition: of its body, without the statements a
    /// syntax error left in it that are indented no deeper than its keyword.
    fn definition_end(&self, start: TextSize, range: TextRange, body: &[Stmt]) -> TextSize {
        if !self.context.recovered {
            return range.end();
        }
        let src = self.context.src;
        let column = src.column(start);
        body.iter()
            .rev()
            .find(|stmt| src.column(stmt.start()) > column)
            .map_or(start, |stmt| match stmt {
                Stmt::FunctionDef(function) => {
                    let start = src.definition_start(function.range, &function.decorator_list);
                    self.definition_end(start, function.range, &function.body)
                }
                Stmt::ClassDef(class) => {
                    let start = src.definition_start(class.range, &class.decorator_list);
                    self.definition_end(start, class.range, &class.body)
                }
                stmt => stmt.end(),
            })
    }

    fn visit_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::FunctionDef(function) if !function.name.is_empty() => {
                self.visit_function(function);
            }
            Stmt::ClassDef(class) if !class.name.is_empty() => self.visit_class(class),
            Stmt::TypeAlias(alias) if matches!(&*alias.name, Expr::Name(_)) => {
                self.emit_type_alias(alias);
            }
            Stmt::Import(_) | Stmt::ImportFrom(_) => self.emit_imports(stmt),
            Stmt::If(branch) if python_is_type_checking_test(&branch.test) => {
                self.visit_expr(&branch.test);
                self.type_checking_depth += 1;
                self.visit_body(&branch.body);
                self.type_checking_depth -= 1;
                for clause in &branch.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.visit_expr(test);
                    }
                    self.visit_body(&clause.body);
                }
            }
            Stmt::If(_) | Stmt::For(_) | Stmt::While(_) | Stmt::Match(_) => {
                self.deferred(|walker| walk_stmt(walker, stmt));
            }
            // The `try` body and `finally` run; a handler and `else` may not.
            Stmt::Try(attempt) => {
                self.visit_body(&attempt.body);
                self.deferred(|walker| {
                    for handler in &attempt.handlers {
                        walker.visit_except_handler(handler);
                    }
                    walker.visit_body(&attempt.orelse);
                });
                self.visit_body(&attempt.finalbody);
            }
            Stmt::Assign(assign) => self.visit_assign(assign),
            Stmt::AnnAssign(assign) => self.visit_assignment(
                &assign.target,
                Some(&assign.annotation),
                assign.value.as_deref(),
                assign.range,
            ),
            // `return wrapped`: a nested function handed to the caller.
            Stmt::Return(ast::StmtReturn {
                value: Some(value), ..
            }) if matches!(&**value, Expr::Name(_)) => {
                if let Expr::Name(name) = &**value {
                    self.emit_value_reference(name);
                }
                walk_stmt(self, stmt);
            }
            Stmt::AugAssign(assign) => {
                walk_stmt(self, stmt);
                python_bind_assignment(&assign.target, None, Some(&assign.value), self.context);
            }
            _ => walk_stmt(self, stmt),
        }
    }
    fn deferred(&mut self, walk: impl FnOnce(&mut Self)) {
        self.deferred_depth += 1;
        walk(self);
        self.deferred_depth -= 1;
    }

    fn caller(&self) -> String {
        self.enclosing_qualified
            .clone()
            .unwrap_or_else(|| self.context.file_path.to_string())
    }

    fn scope(&self) -> Option<String> {
        python_scope_path(&self.context.file_path, self.enclosing_qualified.as_deref())
            .map(str::to_string)
    }

    fn visit_function(&mut self, function: &ast::StmtFunctionDef) {
        let context = self.context;
        // Decorators run in the enclosing scope.
        for decorator in &function.decorator_list {
            self.visit_decorator(decorator);
        }
        let name = function.name.to_string();
        let scope = self.scope();
        let qualified = qualify(&context.file_path, &name, scope.as_deref());
        let params = python_parameters_text(function, context.src);
        let return_type = function
            .returns
            .as_deref()
            .map(|returns| context.text(returns));
        let is_test = python_is_test_function(&name, &context.file_path, function, context.src);
        let decorators = python_decorator_names(&function.decorator_list, context.src);
        let mut extra = json!({});
        if !decorators.is_empty() {
            extra["decorators"] = json!(decorators);
        }
        if decorators
            .iter()
            .any(|decorator| decorator.rsplit('.').next() == Some("abstractmethod"))
        {
            extra["is_abstract"] = json!(true);
        }
        let start = context
            .src
            .definition_start(function.range, &function.decorator_list);
        let line_start = context.src.line(start);
        self.nodes.push(ParsedNode {
            kind: if is_test {
                crate::core::types::NodeKind::Test
            } else {
                crate::core::types::NodeKind::Function
            },
            name,
            file_path: context.file_path.clone(),
            line_start,
            line_end: context
                .src
                .line(self.definition_end(start, function.range, &function.body)),
            language: "python".to_string(),
            parent_name: scope,
            params,
            return_type,
            modifiers: None,
            is_test,
            extra,
        });
        self.edges.push(ParsedEdge::new(
            crate::core::types::EdgeKind::Contains,
            self.caller(),
            qualified.clone(),
            context.file_path.clone(),
            line_start,
        ));
        let snapshot = context.bindings.borrow().snapshot();
        if let Some(class_name) = &self.enclosing_class {
            context
                .bindings
                .borrow_mut()
                .bind_implicit_receivers(class_name);
        }
        for (name, type_name) in python_parameter_types(&function.parameters, context.src) {
            python_bind_type(&name, &type_name, context);
        }
        if context.pytest_file {
            python_bind_pytest_fixtures(&function.parameters, context);
        }
        self.push_scope(start);
        let outer = self.enclosing_qualified.replace(qualified);
        if let Some(type_params) = &function.type_params {
            self.visit_type_params(type_params);
        }
        self.visit_parameters(&function.parameters);
        if let Some(returns) = &function.returns {
            self.visit_annotation(returns);
        }
        self.function_depth += 1;
        self.visit_body(&function.body);
        self.function_depth -= 1;
        self.enclosing_qualified = outer;
        self.pop_scope();
        context.bindings.borrow_mut().restore(snapshot);
    }

    fn visit_class(&mut self, class: &ast::StmtClassDef) {
        let context = self.context;
        for decorator in &class.decorator_list {
            self.visit_decorator(decorator);
        }
        let name = class.name.to_string();
        let scope = self.scope();
        let qualified = qualify(&context.file_path, &name, scope.as_deref());
        let class_path = scope
            .as_ref()
            .map(|scope| format!("{scope}.{name}"))
            .unwrap_or_else(|| name.clone());
        let bases = python_class_base_names(class, context.src);
        let decorators = python_decorator_names(&class.decorator_list, context.src);
        let start = context
            .src
            .definition_start(class.range, &class.decorator_list);
        let line_start = context.src.line(start);
        self.nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Class,
            name,
            file_path: context.file_path.clone(),
            line_start,
            line_end: context
                .src
                .line(self.definition_end(start, class.range, &class.body)),
            language: "python".to_string(),
            parent_name: scope,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: python_class_extra(&bases, &decorators),
        });
        self.edges.push(ParsedEdge::new(
            crate::core::types::EdgeKind::Contains,
            self.caller(),
            qualified.clone(),
            context.file_path.clone(),
            line_start,
        ));
        python_emit_bases(line_start, context, &qualified, &bases, self.edges);
        self.push_scope(start);
        self.class_bases.push(bases);
        let outer_class = self.enclosing_class.replace(class_path);
        let outer = self.enclosing_qualified.replace(qualified);
        if let Some(type_params) = &class.type_params {
            self.visit_type_params(type_params);
        }
        if let Some(arguments) = &class.arguments {
            self.visit_arguments(arguments);
        }
        self.visit_body(&class.body);
        self.enclosing_qualified = outer;
        self.enclosing_class = outer_class;
        self.class_bases.pop();
        self.pop_scope();
    }

    fn emit_type_alias(&mut self, alias: &ast::StmtTypeAlias) {
        let context = self.context;
        let name = context.text(&*alias.name);
        let scope = self.scope();
        let qualified = qualify(&context.file_path, &name, scope.as_deref());
        let line_start = context.line(alias);
        self.nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Type,
            name,
            file_path: context.file_path.clone(),
            line_start,
            line_end: context.src.end_line(alias),
            language: "python".to_string(),
            parent_name: scope,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"type_role": "alias"}),
        });
        self.edges.push(ParsedEdge::new(
            crate::core::types::EdgeKind::Contains,
            self.caller(),
            qualified,
            context.file_path.clone(),
            line_start,
        ));
    }

    fn emit_imports(&mut self, stmt: &Stmt) {
        let context = self.context;
        for (target, extra) in python_import_targets(stmt, &context.file_path, context.repo_root) {
            let (mut target, mut extra) = (target, extra);
            // Unresolved (`target` is the module as written) and of the
            // standard library: an import of its package.
            if extra["module"].as_str() == Some(target.as_str()) {
                if let Some(package) = python_stdlib_package(&target) {
                    mark_stdlib_edge(&mut target, &mut extra, package, StdlibEvidence::Certain);
                } else if let Some(package) =
                    python_external_package(&target, &context.file_path, context.repo_root)
                {
                    mark_external_edge(&mut target, &mut extra, &package, StdlibEvidence::Likely);
                }
            }
            // A module-level import runs on import and is left unmarked.
            if self.type_checking_depth > 0 {
                extra["import_scope"] = json!("type_checking");
            } else if self.function_depth > 0 {
                extra["import_scope"] = json!("function");
            }
            self.edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom,
                source: context.file_path.to_string(),
                target,
                file_path: context.file_path.clone(),
                line: context.line(stmt),
                extra,
            });
        }
    }

    fn emit_call(&mut self, call: &ast::ExprCall) {
        let context = self.context;
        self.emit_argument_references(call);
        let Some(call_name) = python_call_name(&call.func) else {
            return;
        };
        let caller = self.caller();
        let resolved = python_bound_member_target(call, context)
            .or_else(|| python_resolve_imported_call_target(&call_name, context));
        let mut extra = match python_import_receiver(call, context) {
            Some(receiver) => json!({"receiver": receiver}),
            None => json!({}),
        };
        let callee = &*call.func;
        let target = match resolved {
            Some(target) => target,
            None => match python_stdlib_name(callee, context, true) {
                Some((package, mut symbol, evidence)) => {
                    mark_stdlib_edge(&mut symbol, &mut extra, package, evidence);
                    symbol
                }
                None => match python_external_name(callee, context) {
                    Some((package, mut symbol)) => {
                        mark_external_edge(
                            &mut symbol,
                            &mut extra,
                            &package,
                            StdlibEvidence::Likely,
                        );
                        symbol
                    }
                    None => {
                        python_mark_receiver(callee, self.class_bases.last(), context, &mut extra);
                        call_name
                    }
                },
            },
        };
        // `module.f(..)` with `module` a module of this repository imported
        // anywhere in the file (`from pkg import module` in a test body):
        // resolution looks for `f` in that module's file.
        if !target.contains("::")
            && let Some(receiver) = extra.get("receiver").and_then(Value::as_str)
            && let Some(module) = context.import_aliases.get(receiver)
            && let Some(file) =
                python_resolve_module_to_file(module, &context.file_path, context.repo_root)
        {
            extra["module_file"] = json!(file);
        }
        // A call at the module's top level, outside any branch, loop, or
        // lambda, runs whenever the module is imported.
        if self.enclosing_qualified.is_none() && self.deferred_depth == 0 {
            extra["import_time"] = json!(true);
        }
        let line = context.line(call);
        self.edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target,
            file_path: context.file_path.clone(),
            line,
            extra,
        });
        if let Some(edge) = python_bridge_edge(
            call,
            context.src,
            &context.file_path,
            &caller,
            context.import_aliases,
        ) {
            self.edges.push(edge);
        }
    }

    /// `a = b = value`: each target but the last is bound to the next
    /// assignment, as tree-sitter nests a chained assignment, so only the
    /// last target takes the value's type.
    fn visit_assign(&mut self, assign: &ast::StmtAssign) {
        let Some((last, others)) = assign.targets.split_last() else {
            self.visit_expr(&assign.value);
            return;
        };
        for target in others {
            self.visit_expr(target);
        }
        let range = TextRange::new(last.start(), assign.end());
        self.visit_assignment(last, None, Some(&assign.value), range);
        for target in others.iter().rev() {
            if let Expr::Name(name) = target {
                self.context
                    .bindings
                    .borrow_mut()
                    .forget_foreign(name.id.as_str());
            }
        }
    }

    /// One `target [: annotation] [= value]` assignment.
    fn visit_assignment(
        &mut self,
        target: &Expr,
        annotation: Option<&Expr>,
        value: Option<&Expr>,
        range: TextRange,
    ) {
        let context = self.context;
        if let (Expr::Name(name), Some(Expr::Lambda(lambda))) = (target, value) {
            self.emit_lambda(name, lambda, range);
            python_bind_assignment(target, annotation, value, context);
            return;
        }
        // `self.handler = handler`, `registry[key] = handler`.
        if matches!(target, Expr::Attribute(_) | Expr::Subscript(_))
            && let Some(Expr::Name(name)) = value
        {
            self.emit_value_reference(name);
        }
        // `render = full if wide else short`: either branch may run.
        if let Some(Expr::If(choice)) = value {
            for branch in [&*choice.body, &*choice.orelse] {
                if let Expr::Name(name) = branch {
                    self.emit_value_reference(name);
                }
            }
        }
        self.visit_expr(target);
        if let Some(annotation) = annotation {
            self.visit_annotation(annotation);
        }
        if let Some(value) = value {
            self.visit_expr(value);
        }
        python_bind_assignment(target, annotation, value, context);
    }

    /// Emits `name = lambda ...` as a function so calls in the lambda body
    /// have a caller.
    fn emit_lambda(&mut self, name: &ast::ExprName, lambda: &ast::ExprLambda, range: TextRange) {
        let context = self.context;
        let name = name.id.to_string();
        let scope = self.scope();
        let qualified = qualify(&context.file_path, &name, scope.as_deref());
        let line_start = context.src.line(range.start());
        self.nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Function,
            name,
            file_path: context.file_path.clone(),
            line_start,
            line_end: context.src.line(range.end()),
            language: "python".to_string(),
            parent_name: scope,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"python_kind": "lambda"}),
        });
        self.edges.push(ParsedEdge::new(
            crate::core::types::EdgeKind::Contains,
            self.caller(),
            qualified.clone(),
            context.file_path.clone(),
            line_start,
        ));
        let outer = self.enclosing_qualified.replace(qualified);
        if let Some(parameters) = &lambda.parameters {
            self.visit_parameters(parameters);
        }
        self.visit_expr(&lambda.body);
        self.enclosing_qualified = outer;
    }

    /// Functions passed as arguments (`run_guarded(args, dispatch)`,
    /// `Thread(target=self._loop)`): references to them. A function nested
    /// in the enclosing one and a method of the enclosing class are kept
    /// only if the file defines them ([`python_keep_checked_references`]).
    fn emit_argument_references(&mut self, call: &ast::ExprCall) {
        let context = self.context;
        let values = call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|keyword| &keyword.value));
        for value in values {
            let target = match value {
                Expr::Name(name) => {
                    self.emit_value_reference(name);
                    continue;
                }
                Expr::Attribute(attribute)
                    if matches!(&*attribute.value, Expr::Name(owner)
                        if matches!(owner.id.as_str(), "self" | "cls")) =>
                {
                    let Some(class) = &self.enclosing_class else {
                        continue;
                    };
                    qualify(&context.file_path, attribute.attr.as_str(), Some(class))
                }
                _ => continue,
            };
            self.edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::References,
                source: self.caller(),
                target,
                file_path: context.file_path.clone(),
                line: context.line(value),
                extra: json!({"checked_local": true}),
            });
        }
    }

    /// A name used as a value: a function or class of the file or an
    /// import, or a function nested in the enclosing one (kept only if the
    /// file defines it).
    fn emit_value_reference(&mut self, name: &ast::ExprName) {
        let context = self.context;
        if python_resolve_reference_target(name.id.as_str(), context).is_some() {
            self.emit_reference_if_known(name);
            return;
        }
        let Some(scope) = &self.enclosing_qualified else {
            return;
        };
        if python_skip_value_reference_name(name.id.as_str()) {
            return;
        }
        self.edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::References,
            source: self.caller(),
            target: format!("{scope}.{}", name.id),
            file_path: context.file_path.clone(),
            line: context.line(name),
            extra: json!({"checked_local": true}),
        });
    }

    fn emit_reference_if_known(&mut self, name: &ast::ExprName) {
        let context = self.context;
        let Some(target) = python_resolve_reference_target(name.id.as_str(), context) else {
            return;
        };
        self.edges.push(ParsedEdge::new(
            crate::core::types::EdgeKind::References,
            self.caller(),
            target,
            context.file_path.clone(),
            context.line(name),
        ));
    }
}

/// Drops the argument references whose nested function or method the file
/// does not define (`self.store` is an attribute, not a method).
fn python_keep_checked_references(
    file_path: &str,
    nodes: &[ParsedNode],
    edges: &mut Vec<ParsedEdge>,
) {
    let defined: HashSet<String> = nodes
        .iter()
        .filter(|node| node.kind == "Function")
        .map(|node| qualify(file_path, &node.name, node.parent_name.as_deref()))
        .collect();
    edges.retain_mut(|edge| {
        if edge.extra.get("checked_local").is_none() {
            return true;
        }
        if let Some(extra) = edge.extra.as_object_mut() {
            extra.remove("checked_local");
        }
        defined.contains(&edge.target)
    });
}

/// Parent path (relative to the file) of the innermost enclosing class or
/// function, so nested definitions qualify under it.
fn python_scope_path<'a>(file_path: &str, enclosing_qualified: Option<&'a str>) -> Option<&'a str> {
    enclosing_qualified?
        .strip_prefix(file_path)?
        .strip_prefix("::")
        .filter(|scope| !scope.is_empty())
}

/// The parameter list of a definition as written, parentheses included.
fn python_parameters_text(function: &ast::StmtFunctionDef, src: &PySource<'_>) -> Option<String> {
    let text = src.slice(&*function.parameters);
    (!text.is_empty()).then(|| text.to_string())
}

fn python_resolve_reference_target(name: &str, context: &PythonParseContext<'_>) -> Option<String> {
    if python_skip_value_reference_name(name) {
        return None;
    }
    if context.top_level_defined_names.contains(name) {
        return Some(qualify(&context.file_path, name, None));
    }
    let (module, original) = context.import_map.get(name)?;
    Some(
        python_resolve_module_to_file(module, &context.file_path, context.repo_root)
            .map(|resolved| qualify(&resolved, original, None))
            .unwrap_or_else(|| name.to_string()),
    )
}

fn python_skip_value_reference_name(name: &str) -> bool {
    name.is_empty()
        || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
        || matches!(
            name,
            "true"
                | "false"
                | "null"
                | "undefined"
                | "None"
                | "True"
                | "False"
                | "self"
                | "this"
                | "cls"
                | "super"
        )
}

fn collect_python_file_scope(
    body: &[Stmt],
    src: &PySource<'_>,
) -> (ImportMap, HashSet<String>, HashSet<String>) {
    let mut import_map = HashMap::new();
    let mut defined_names = HashSet::new();
    let mut protocol_names = HashSet::new();
    for stmt in body {
        match stmt {
            Stmt::ClassDef(class) if !class.name.is_empty() => {
                let name = class.name.to_string();
                if python_class_base_names(class, src)
                    .iter()
                    .any(|base| python_is_protocol_marker(base))
                {
                    protocol_names.insert(name.clone());
                }
                defined_names.insert(name);
            }
            Stmt::FunctionDef(function) if !function.name.is_empty() => {
                defined_names.insert(function.name.to_string());
            }
            Stmt::TypeAlias(alias) => {
                let name = src.slice(&*alias.name);
                if !name.is_empty() {
                    defined_names.insert(name.to_string());
                }
            }
            Stmt::ImportFrom(import) => {
                let module = python_import_from_module(import);
                if module.is_empty() {
                    continue;
                }
                for alias in python_import_from_names(import) {
                    let bound = alias.asname.as_ref().unwrap_or(&alias.name);
                    import_map.insert(bound.to_string(), (module.clone(), alias.name.to_string()));
                }
            }
            _ => {}
        }
    }
    (import_map, defined_names, protocol_names)
}

/// The module of `from <module> import ...` as written: `pkg.mod`,
/// `.mod`, `..`.
fn python_import_from_module(import: &ast::StmtImportFrom) -> String {
    let mut module = ".".repeat(import.level as usize);
    if let Some(name) = &import.module {
        module.push_str(name);
    }
    module
}

/// The names `from m import ...` binds; none for `from m import *`, and
/// none for `from __future__ import ...`, a compiler directive.
fn python_import_from_names(import: &ast::StmtImportFrom) -> impl Iterator<Item = &ast::Alias> {
    let future = import.level == 0
        && import
            .module
            .as_ref()
            .is_some_and(|module| module.as_str() == "__future__");
    import
        .names
        .iter()
        .filter(move |alias| !future && alias.name.as_str() != "*" && !alias.name.is_empty())
}

fn python_is_test_function(
    name: &str,
    file_path: &FilePath,
    function: &ast::StmtFunctionDef,
    src: &PySource<'_>,
) -> bool {
    python_name_matches_test_pattern(name)
        || (is_test_file(file_path) && python_is_test_runner_name(name))
        || python_has_test_annotation(&function.decorator_list, src)
}

fn python_name_matches_test_pattern(name: &str) -> bool {
    name.starts_with("test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_spec")
}

fn python_is_test_runner_name(name: &str) -> bool {
    matches!(
        name,
        "describe" | "it" | "test" | "beforeEach" | "afterEach" | "beforeAll" | "afterAll"
    )
}

fn python_has_test_annotation(decorators: &[Decorator], src: &PySource<'_>) -> bool {
    decorators.iter().any(|decorator| {
        matches!(
            src.slice(&decorator.expression).trim(),
            "Test"
                | "ParameterizedTest"
                | "RepeatedTest"
                | "TestFactory"
                | "org.junit.Test"
                | "org.junit.jupiter.api.Test"
        )
    })
}

fn python_emit_bases(
    line: i64,
    context: &PythonParseContext<'_>,
    qualified: &str,
    bases: &[String],
    edges: &mut Vec<ParsedEdge>,
) {
    for base in bases {
        let (kind, role) = if python_is_protocol_marker(base)
            || python_is_abc_marker(base)
            || python_is_typed_dict_marker(base)
        {
            (crate::core::types::EdgeKind::Inherits, "extends")
        } else if context.protocol_names.contains(base) {
            (crate::core::types::EdgeKind::Implements, "implements")
        } else {
            (crate::core::types::EdgeKind::Inherits, "extends")
        };
        edges.push(ParsedEdge {
            kind,
            source: qualified.to_string(),
            target: base.clone(),
            file_path: context.file_path.clone(),
            line,
            extra: json!({
                "relationship_role": role,
                "syntax_source": "class_definition",
            }),
        });
    }
}

/// The positional bases of a class, as written (`Base`, `pkg.Base`, and
/// `Generic` of `Generic[T]`). Keyword arguments (`metaclass=`) are not
/// bases.
fn python_class_base_names(class: &ast::StmtClassDef, src: &PySource<'_>) -> Vec<String> {
    class
        .bases()
        .iter()
        .filter_map(|base| python_base_name(base, src))
        .collect()
}

fn python_base_name(expr: &Expr, src: &PySource<'_>) -> Option<String> {
    match expr {
        Expr::Name(_) | Expr::Attribute(_) => Some(src.slice(expr).to_string()),
        Expr::Subscript(subscript) => {
            python_base_name(&subscript.value, src).or_else(|| match &*subscript.slice {
                Expr::Tuple(tuple) if !tuple.parenthesized => tuple
                    .elts
                    .iter()
                    .find_map(|element| python_base_name(element, src)),
                slice => python_base_name(slice, src),
            })
        }
        _ => None,
    }
}

fn python_class_extra(bases: &[String], decorators: &[String]) -> Value {
    let is_protocol = bases.iter().any(|base| python_is_protocol_marker(base));
    let is_abc = bases.iter().any(|base| python_is_abc_marker(base));
    let is_typed_dict = bases.iter().any(|base| python_is_typed_dict_marker(base));
    let type_role = if is_protocol {
        "protocol"
    } else if is_abc {
        "abstract_class"
    } else if is_typed_dict {
        "typed_dict"
    } else {
        "class"
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_protocol || is_abc {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_protocol {
            map.insert("is_contract".to_string(), json!(true));
        }
        if !decorators.is_empty() {
            map.insert("decorators".to_string(), json!(decorators));
        }
    }
    extra
}

fn python_is_protocol_marker(name: &str) -> bool {
    name.rsplit('.').next().unwrap_or(name) == "Protocol"
}

fn python_is_abc_marker(name: &str) -> bool {
    matches!(name.rsplit('.').next().unwrap_or(name), "ABC" | "ABCMeta")
}

fn python_is_typed_dict_marker(name: &str) -> bool {
    name.rsplit('.').next().unwrap_or(name) == "TypedDict"
}

/// The decorators of a definition by the name they call or name:
/// `app.route` of `@app.route("/")`, `property` of `@property`.
fn python_decorator_names(decorators: &[Decorator], src: &PySource<'_>) -> Vec<String> {
    decorators
        .iter()
        .filter_map(|decorator| python_decorator_name(decorator, src))
        .collect()
}

fn python_decorator_name(decorator: &Decorator, src: &PySource<'_>) -> Option<String> {
    match &decorator.expression {
        expr @ (Expr::Name(_) | Expr::Attribute(_)) => Some(src.slice(expr).to_string()),
        Expr::Call(call) if matches!(&*call.func, Expr::Name(_) | Expr::Attribute(_)) => {
            Some(src.slice(&*call.func).to_string())
        }
        _ => None,
    }
}

/// `TYPE_CHECKING` or `typing.TYPE_CHECKING` (any module alias), the test
/// of an `if` whose body only type checkers run.
fn python_is_type_checking_test(test: &Expr) -> bool {
    match test {
        Expr::Name(name) => name.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(attribute) => attribute.attr.as_str() == "TYPE_CHECKING",
        _ => false,
    }
}

/// IMPORTS_FROM targets of one import statement, each with the raw module
/// and the names it binds in `extra`: the target may already be resolved to
/// a file, and native-binding resolution needs the module as written
/// (`from pkg import _core` names `pkg._core`, a module with no `.py`).
fn python_import_targets(
    stmt: &Stmt,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Vec<(String, serde_json::Value)> {
    match stmt {
        Stmt::Import(import) => import
            .names
            .iter()
            .filter(|alias| !alias.name.is_empty())
            .map(|alias| {
                let module = alias.name.to_string();
                let mut extra = json!({"module": module});
                if let Some(asname) = &alias.asname {
                    extra["alias"] = json!(asname.as_str());
                }
                (
                    python_resolve_module_to_file(&module, file_path, repo_root).unwrap_or(module),
                    extra,
                )
            })
            .collect(),
        Stmt::ImportFrom(import) => {
            let module = python_import_from_module(import);
            // `from __future__ import annotations` is a compiler directive,
            // not a dependency (tree-sitter parses it as its own statement).
            if module.is_empty() || module == "__future__" {
                return Vec::new();
            }
            // `from pkg import sub` imports the submodule `pkg/sub.py`, the
            // same way `import pkg.sub` does; only names that are not
            // submodules (or `*`) make it an import of `pkg` itself.
            let mut submodules = Vec::new();
            let mut imports_package = false;
            let mut bound_names = Vec::new();
            let mut names = python_import_from_names(import).peekable();
            if names.peek().is_none() {
                imports_package = true;
            }
            for alias in names {
                let name = alias.name.to_string();
                let bound = alias
                    .asname
                    .as_ref()
                    .map_or_else(|| name.clone(), |asname| asname.to_string());
                bound_names.push(json!([name, bound]));
                let submodule = if module.ends_with('.') {
                    format!("{module}{name}")
                } else {
                    format!("{module}.{name}")
                };
                match python_resolve_module_to_file(&submodule, file_path, repo_root) {
                    Some(path) if !submodules.contains(&path) => submodules.push(path),
                    Some(_) => {}
                    None => imports_package = true,
                }
            }
            let extra = json!({"module": module, "names": bound_names});
            let mut imports = Vec::new();
            if imports_package {
                imports.push((
                    python_resolve_module_to_file(&module, file_path, repo_root).unwrap_or(module),
                    extra.clone(),
                ));
            }
            imports.extend(submodules.into_iter().map(|path| (path, extra.clone())));
            imports
        }
        _ => Vec::new(),
    }
}

/// Every local name an import statement binds, at any depth of the file,
/// mapped to what it names: `import a.b` binds `a` -> `a`, `import a as b`
/// binds `b` -> `a`, `from m import n as k` binds `k` -> `m.n`.
fn collect_python_import_aliases(body: &[Stmt]) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for_each_statement(body, |stmt| match stmt {
        Stmt::Import(import) => {
            for alias in &import.names {
                match &alias.asname {
                    Some(asname) => {
                        aliases.insert(asname.to_string(), alias.name.to_string());
                    }
                    None => {
                        if let Some(head) = alias.name.split('.').next()
                            && !head.is_empty()
                        {
                            aliases.insert(head.to_string(), head.to_string());
                        }
                    }
                }
            }
        }
        Stmt::ImportFrom(import) => {
            let module = python_import_from_module(import);
            for alias in python_import_from_names(import) {
                let origin = if module.is_empty() || module.ends_with('.') {
                    format!("{module}{}", alias.name)
                } else {
                    format!("{module}.{}", alias.name)
                };
                let bound = alias.asname.as_ref().unwrap_or(&alias.name);
                aliases.insert(bound.to_string(), origin);
            }
        }
        _ => {}
    });
    aliases
}

/// The receiver of `alias.attr(...)` when `alias` was bound by an import.
fn python_import_receiver(
    call: &ast::ExprCall,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    let Expr::Attribute(attribute) = &*call.func else {
        return None;
    };
    let Expr::Name(receiver) = &*attribute.value else {
        return None;
    };
    let receiver = receiver.id.as_str();
    context
        .import_aliases
        .contains_key(receiver)
        .then(|| receiver.to_string())
}

/// The import aliases of `import_aliases` that name a standard-library
/// module, or a name imported from one, that no module of this repository
/// shadows (a local `json.py` is imported instead of the standard `json`).
fn python_stdlib_aliases(
    import_aliases: &HashMap<String, String>,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> HashMap<String, (&'static str, String)> {
    let mut shadowed = HashMap::new();
    import_aliases
        .iter()
        .filter_map(|(local, origin)| {
            let package = python_stdlib_package(origin)?;
            let in_repo = *shadowed.entry(package).or_insert_with(|| {
                python_resolve_module_to_file(package, file_path, repo_root).is_some()
            });
            (!in_repo).then(|| (local.clone(), (package, origin.clone())))
        })
        .collect()
}

/// Whether the repository has a module or package named `top`: a
/// `top.py` or `top/` beside the file or in a directory above it, or under
/// `src/` (the src layout).
fn python_top_module_in_repo(top: &str, file_path: &FilePath, repo_root: &Path) -> bool {
    let exists = |dir: &Path| {
        let base = repo_root.join(dir);
        base.join(format!("{top}.py")).is_file() || base.join(top).is_dir()
    };
    let mut dir = Path::new(file_path.as_str()).parent();
    while let Some(current) = dir {
        if exists(current) {
            return true;
        }
        dir = current.parent();
    }
    exists(Path::new("")) || exists(Path::new("src"))
}

/// The third-party package an absolute module belongs to (its top-level
/// module): neither the standard library nor a module of the repository.
/// Unknown without a repository root.
fn python_external_package(
    module: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    let repo_root = repo_root?;
    if module.starts_with('.') || python_stdlib_package(module).is_some() {
        return None;
    }
    let top = module.split('.').next().filter(|top| !top.is_empty())?;
    (!python_top_module_in_repo(top, file_path, repo_root)).then(|| top.to_string())
}

/// The import aliases that name a third-party module or a name imported
/// from one (see [`python_external_package`]).
fn python_external_aliases(
    import_aliases: &HashMap<String, String>,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> HashMap<String, (String, String)> {
    let mut packages: HashMap<String, Option<String>> = HashMap::new();
    import_aliases
        .iter()
        .filter_map(|(local, origin)| {
            let top = origin.split('.').next()?.to_string();
            let package = packages
                .entry(top.clone())
                .or_insert_with(|| python_external_package(&top, file_path, repo_root))
                .clone()?;
            Some((local.clone(), (package, origin.clone())))
        })
        .collect()
}

/// The third-party package and dotted name a callee is written through: an
/// import alias of a third-party module (`yaml.safe_load`, `np.array`) or a
/// name imported from one (`from pytest import raises`).
fn python_external_name(expr: &Expr, context: &PythonParseContext<'_>) -> Option<(String, String)> {
    match expr {
        Expr::Name(name) => {
            let name = name.id.as_str();
            if context.defined_names.contains(name) {
                return None;
            }
            context.external_aliases.get(name).cloned()
        }
        Expr::Attribute(attribute) => {
            let object = &*attribute.value;
            if !matches!(object, Expr::Name(_) | Expr::Attribute(_)) {
                return None;
            }
            let attr = attribute.attr.as_str();
            // A value of a third-party type (`monkeypatch.setattr` with
            // `monkeypatch: pytest.MonkeyPatch` or pytest's fixture).
            if let Some(PythonType::External(package, symbol)) =
                python_receiver_type(object, context)
            {
                return Some((package, format!("{symbol}.{attr}")));
            }
            let (package, base) = python_external_name(object, context)?;
            Some((package, format!("{base}.{attr}")))
        }
        _ => None,
    }
}

/// The standard-library package and dotted name an expression resolves to:
/// an import alias of the standard library (`subprocess`, `run` of `from
/// subprocess import run`), an attribute of one (`os.path.join`), a variable
/// bound to a standard-library value, or the value a call constructs
/// (`Path(p).open`, `open(p).read`, not `importlib.import_module(m).run`).
/// A builtin (`len`) counts when `bare_builtin` or when it is called
/// (`str(x).strip`); a bare variable named like one is too often a local
/// (`format`, `id`) to count.
///
/// The evidence is certain when the name is rooted at an import of the
/// standard library; a builtin (a local may shadow it) or a variable bound
/// to a standard-library value (it may be reassigned in a branch) is likely.
fn python_stdlib_name(
    expr: &Expr,
    context: &PythonParseContext<'_>,
    bare_builtin: bool,
) -> Option<(&'static str, String, StdlibEvidence)> {
    match expr {
        Expr::Name(name) => {
            let name = name.id.as_str();
            if let Some((package, origin)) = context.stdlib_aliases.get(name) {
                return Some((package, origin.clone(), StdlibEvidence::Certain));
            }
            if let Some(PythonType::Stdlib(package, symbol)) = python_receiver_type(expr, context) {
                return Some((package, symbol, StdlibEvidence::Likely));
            }
            let builtin = bare_builtin
                && is_python_builtin(name)
                && !context.defined_names.contains(name)
                && !context.import_aliases.contains_key(name);
            builtin.then(|| ("builtins", name.to_string(), StdlibEvidence::Likely))
        }
        Expr::Attribute(attribute) => {
            if let Some(PythonType::Stdlib(package, symbol)) = python_receiver_type(expr, context) {
                return Some((package, symbol, StdlibEvidence::Likely));
            }
            let (package, base, evidence) = python_stdlib_name(&attribute.value, context, false)?;
            Some((package, format!("{base}.{}", attribute.attr), evidence))
        }
        Expr::Call(call) => python_stdlib_name(&call.func, context, true)
            .filter(|(package, symbol, _)| python_constructs_stdlib_value(package, symbol)),
        _ => None,
    }
}

/// The name a call calls: `f` of `f()`, `m` of `obj.m()`.
fn python_call_name(func: &Expr) -> Option<String> {
    match func {
        Expr::Name(name) => Some(name.id.to_string()),
        Expr::Attribute(attribute) if !attribute.attr.is_empty() => {
            Some(attribute.attr.to_string())
        }
        _ => None,
    }
}

fn python_bound_member_target(
    call: &ast::ExprCall,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    let Expr::Attribute(attribute) = &*call.func else {
        return None;
    };
    if attribute.attr.is_empty() {
        return None;
    }
    let method = attribute.attr.as_str();
    match &*attribute.value {
        // `self.store.save()` with `self.store = Store()`, `Store` of this file.
        receiver @ Expr::Attribute(_) => match python_receiver_type(receiver, context)? {
            PythonType::Local(type_name) => Some(format!("{type_name}::{method}")),
            _ => None,
        },
        Expr::Name(receiver) => context
            .bindings
            .borrow()
            .resolve_member(receiver.id.as_str(), method),
        _ => None,
    }
}

/// Binds the variable an assignment assigns to what its value says: a
/// standard-library value (`p = Path(x)`), an instance of a class of this
/// file or an imported one (`store = GraphStore(path)`), the annotated type
/// (`store: GraphStore = ...`), or the result of a call.
fn python_bind_assignment(
    target: &Expr,
    annotation: Option<&Expr>,
    value: Option<&Expr>,
    context: &PythonParseContext<'_>,
) {
    let Expr::Name(target) = target else {
        return;
    };
    let var = target.id.as_str();
    // A value of the standard library: `p = Path(x)`, `f = open(x)`.
    context.bindings.borrow_mut().forget_foreign(var);
    if let Some(rhs @ Expr::Call(call)) = value {
        if let Some((_, symbol, _)) = python_stdlib_name(rhs, context, true) {
            context.bindings.borrow_mut().bind_any(var, symbol);
            return;
        }
        if let Some(call_name) = python_call_name(&call.func) {
            let type_name = context
                .bindings
                .borrow()
                .constructor_type(&call_name)
                .map(str::to_string);
            if let Some(type_name) = type_name {
                context.bindings.borrow_mut().bind(var, type_name);
                return;
            }
            // A class imported from another module of the repository:
            // `store = GraphStore(path)`.
            if matches!(&*call.func, Expr::Name(_) | Expr::Attribute(_)) {
                let callee = context.text(&*call.func);
                let root = callee.split('.').next().unwrap_or(&callee);
                let is_class = callee
                    .rsplit('.')
                    .next()
                    .is_some_and(|name| name.starts_with(|c: char| c.is_ascii_uppercase()));
                if is_class && context.import_aliases.contains_key(root) {
                    python_bind_type(var, &callee, context);
                    return;
                }
            }
        }
    }
    if let Some(annotation) =
        annotation.and_then(|annotation| python_annotation_type(annotation, context.src))
    {
        python_bind_type(var, &annotation, context);
        return;
    }
    // `conn = store_conn(store)`: the type is the call's return type.
    if let Some(origin) = value.and_then(|rhs| python_call_origin(rhs, context)) {
        context.bindings.borrow_mut().bind_returned(var, origin);
    }
}

/// The call an expression is the result of (`store_conn(store)`,
/// `self.store.connection()`), or of which a variable holds the result.
fn python_call_origin(expression: &Expr, context: &PythonParseContext<'_>) -> Option<CallOrigin> {
    match expression {
        Expr::Call(call) => {
            let name = match &*call.func {
                Expr::Name(name) => name.id.to_string(),
                Expr::Attribute(attribute) => attribute.attr.to_string(),
                _ => return None,
            };
            Some(CallOrigin {
                name,
                line: context.line(call),
                unwrap: false,
                element: false,
            })
        }
        Expr::Name(name) => context
            .bindings
            .borrow()
            .returned_by(name.id.as_str())
            .cloned(),
        _ => None,
    }
}

/// What a type written in the file is: a class of this file, a type of the
/// standard library (package, dotted name), a type of a third-party package
/// (package, dotted name), or a class of another module of the repository
/// (by its name, which resolution across files matches).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PythonType {
    Local(String),
    Stdlib(&'static str, String),
    External(String, String),
    Foreign(String),
}

/// A variable bound to a third-party type is remembered as
/// `package:dotted.name`; no Python name has a `:`.
const EXTERNAL_BINDING_SEPARATOR: char = ':';

/// The types pytest injects into a test or fixture for a parameter of this
/// name (`def test_x(tmp_path, monkeypatch)`), when it has no annotation.
fn python_pytest_fixture_type(name: &str) -> Option<PythonType> {
    let external = |symbol: &str| Some(PythonType::External("pytest".into(), symbol.into()));
    match name {
        "tmp_path" => Some(PythonType::Stdlib("pathlib", "pathlib.Path".into())),
        "tmp_path_factory" => external("pytest.TempPathFactory"),
        "monkeypatch" => external("pytest.MonkeyPatch"),
        "capsys" | "capfd" | "capsysbinary" | "capfdbinary" => external("pytest.CaptureFixture"),
        "caplog" => external("pytest.LogCaptureFixture"),
        "request" => external("pytest.FixtureRequest"),
        "recwarn" => external("pytest.WarningsRecorder"),
        "pytestconfig" => external("pytest.Config"),
        _ => None,
    }
}

/// Classifies `type_name` as written (`GraphStore`, `Path`, `pathlib.Path`,
/// `list`). Protocol types of `typing` / `collections.abc` (`Any`,
/// `Iterable`) say nothing about the methods and are none of these.
fn python_type_of(type_name: &str, context: &PythonParseContext<'_>) -> Option<PythonType> {
    let (root, rest) = match type_name.split_once('.') {
        Some((root, rest)) => (root, Some(rest)),
        None => (type_name, None),
    };
    let last = type_name.rsplit('.').next().unwrap_or(type_name);
    if let Some((package, origin)) = context.stdlib_aliases.get(root) {
        if matches!(*package, "typing" | "typing_extensions")
            || origin.starts_with("collections.abc")
        {
            return None;
        }
        let symbol = match rest {
            Some(rest) => format!("{origin}.{rest}"),
            None => origin.clone(),
        };
        return Some(PythonType::Stdlib(package, symbol));
    }
    if rest.is_none() && python_constructs_stdlib_value("builtins", type_name) {
        return Some(PythonType::Stdlib("builtins", type_name.to_string()));
    }
    if rest.is_none() && context.class_names.contains(type_name) {
        return Some(PythonType::Local(type_name.to_string()));
    }
    if let Some((package, origin)) = context.external_aliases.get(root) {
        let symbol = match rest {
            Some(rest) => format!("{origin}.{rest}"),
            None => origin.clone(),
        };
        return Some(PythonType::External(package.clone(), symbol));
    }
    let is_class = last.starts_with(|c: char| c.is_ascii_uppercase());
    let known = context.import_aliases.contains_key(root) || context.class_names.contains(last);
    (is_class && known).then(|| PythonType::Foreign(last.to_string()))
}

/// Binds `var` to the type `type_name` names (see [`python_type_of`]).
fn python_bind_type(var: &str, type_name: &str, context: &PythonParseContext<'_>) {
    if let Some(python_type) = python_type_of(type_name, context) {
        python_bind(var, python_type, context);
    }
}

fn python_bind(var: &str, python_type: PythonType, context: &PythonParseContext<'_>) {
    let mut bindings = context.bindings.borrow_mut();
    match python_type {
        PythonType::Local(type_name) => bindings.bind(var, type_name),
        PythonType::Stdlib(_, symbol) => bindings.bind_any(var, symbol),
        PythonType::External(package, symbol) => bindings.bind_any(
            var,
            format!("{package}{EXTERNAL_BINDING_SEPARATOR}{symbol}"),
        ),
        PythonType::Foreign(type_name) => bindings.bind_any(var, type_name),
    }
}

/// The type of a receiver: a variable bound to one, or an attribute of
/// `self` its class types.
fn python_receiver_type(expr: &Expr, context: &PythonParseContext<'_>) -> Option<PythonType> {
    let bindings = context.bindings.borrow();
    match expr {
        Expr::Name(name) => {
            let name = name.id.as_str();
            if let Some(bound) = bindings.bound_type(name).filter(|ty| !ty.contains("::")) {
                return Some(PythonType::Local(bound.to_string()));
            }
            let symbol = bindings.foreign_type(name)?;
            // Standard-library values are bound to their dotted name
            // (`pathlib.Path`, `str`), third-party ones to
            // `package:dotted.name`, classes of other modules to theirs.
            if let Some((package, symbol)) = symbol.split_once(EXTERNAL_BINDING_SEPARATOR) {
                return Some(PythonType::External(
                    package.to_string(),
                    symbol.to_string(),
                ));
            }
            if let Some(package) = python_stdlib_package(symbol) {
                return Some(PythonType::Stdlib(package, symbol.to_string()));
            }
            if python_constructs_stdlib_value("builtins", symbol) || is_python_builtin(symbol) {
                return Some(PythonType::Stdlib("builtins", symbol.to_string()));
            }
            Some(PythonType::Foreign(symbol.to_string()))
        }
        Expr::Attribute(attribute) => {
            let Expr::Name(object) = &*attribute.value else {
                return None;
            };
            if object.id.as_str() != "self" {
                return None;
            }
            let class_path = bindings.bound_type("self")?;
            let class_name = class_path.rsplit('.').next().unwrap_or(class_path);
            let type_name = context
                .attribute_types
                .get(class_name)?
                .get(attribute.attr.as_str())?
                .clone();
            drop(bindings);
            python_type_of(&type_name, context)
        }
        _ => None,
    }
}

/// Records what a member call's receiver says when nothing resolved the
/// call: `receiver_type` for a class of another module (`store:
/// GraphStore`), or `receiver_unknown` when the receiver's type is unknown
/// (`plugin.run()`, `make().save()`), so no same-named function of the file
/// is taken for it. A module (`helpers.run()`), `self` / `cls`, a class
/// (`Repo.create()`), and `super()` are known receivers.
fn python_mark_receiver(
    callee: &Expr,
    class_bases: Option<&Vec<String>>,
    context: &PythonParseContext<'_>,
    extra: &mut Value,
) {
    let Expr::Attribute(attribute) = callee else {
        return;
    };
    let receiver = &*attribute.value;
    match python_receiver_type(receiver, context) {
        Some(PythonType::Foreign(type_name)) => {
            extra["receiver_type"] = json!(type_name);
            return;
        }
        Some(_) => return,
        None => {}
    }
    if let Expr::Call(call) = receiver
        && context.src.slice(&*call.func) == "super"
    {
        python_mark_super_receiver(class_bases, extra);
        return;
    }
    let known = match receiver {
        Expr::Name(name) => {
            let name = name.id.as_str();
            matches!(name, "self" | "cls")
                || context.import_aliases.contains_key(name)
                || context.class_names.contains(name)
                || context.bindings.borrow().is_bound(name)
        }
        _ => false,
    };
    if !known {
        extra["receiver_unknown"] = json!(true);
        if let Some(origin) = python_call_origin(receiver, context) {
            extra["receiver_from"] = origin.to_json();
        }
    }
}

/// `super().m()` is a method of a base class, never the caller's own `m`:
/// the receiver is typed by the enclosing class's first base
/// (`class AuthService(BaseService)` gives `BaseService`), and is unknown
/// when the class names none (`object`'s).
fn python_mark_super_receiver(class_bases: Option<&Vec<String>>, extra: &mut Value) {
    let base = class_bases
        .and_then(|bases| bases.first())
        .map(|base| base.rsplit('.').next().unwrap_or(base).to_string())
        .filter(|base| base != "object");
    match base {
        Some(base) => extra["receiver_type"] = json!(base),
        None => extra["receiver_unknown"] = json!(true),
    }
}

/// The classes, and the functions and classes, declared anywhere in the
/// file.
fn collect_python_defined_names(body: &[Stmt]) -> (HashSet<String>, HashSet<String>) {
    let mut class_names = HashSet::new();
    let mut defined_names = HashSet::new();
    for_each_statement(body, |stmt| match stmt {
        Stmt::ClassDef(class) if !class.name.is_empty() => {
            class_names.insert(class.name.to_string());
            defined_names.insert(class.name.to_string());
        }
        Stmt::FunctionDef(function) if !function.name.is_empty() => {
            defined_names.insert(function.name.to_string());
        }
        _ => {}
    });
    (class_names, defined_names)
}

fn resolve_python_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    resolve_rust_call_targets(nodes, edges, file_path)
}

fn python_resolve_imported_call_target(
    call_name: &str,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    if context.top_level_defined_names.contains(call_name) {
        return None;
    }
    let (module, original) = context.import_map.get(call_name)?;
    let resolved = python_resolve_module_to_file(module, &context.file_path, context.repo_root)?;
    Some(qualify(&resolved, original, None))
}

fn add_python_tested_by_edges(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    if !is_test_file(file_path) {
        return edges;
    }
    let test_qnames = nodes
        .iter()
        .filter(|node| node.is_test)
        .map(|node| qualify(file_path, &node.name, node.parent_name.as_deref()))
        .collect::<HashSet<_>>();
    if test_qnames.is_empty() {
        return edges;
    }
    let mut out = edges;
    let tested_by_edges = out
        .iter()
        .filter(|edge| {
            edge.kind == "CALLS"
                && test_qnames.contains(&edge.source)
                && edge.extra["external"] != true
        })
        .map(|edge| {
            ParsedEdge::new(
                crate::core::types::EdgeKind::TestedBy,
                edge.target.clone(),
                edge.source.clone(),
                edge.file_path.clone(),
                edge.line,
            )
        })
        .collect::<Vec<_>>();
    out.extend(tested_by_edges);
    out
}

fn python_resolve_module_to_file(
    module: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let candidates_for = |base: PathBuf, rel: &str| {
        [
            base.join(format!("{rel}.py")),
            base.join(rel).join("__init__.py"),
        ]
    };

    if module.starts_with('.') {
        let leading_dots = module.bytes().take_while(|byte| *byte == b'.').count();
        let remainder = &module[leading_dots..];
        let mut base = caller_dir.to_path_buf();
        for _ in 0..leading_dots.saturating_sub(1) {
            base = base.parent().unwrap_or(Path::new("")).to_path_buf();
        }
        let candidates = if remainder.is_empty() {
            vec![base.join("__init__.py")]
        } else {
            let rel = remainder.replace('.', "/");
            candidates_for(base, &rel).into_iter().collect()
        };
        return candidates
            .into_iter()
            .find(|candidate| python_module_candidate_is_file(candidate, repo_root))
            .and_then(|candidate| python_module_candidate_path(candidate, repo_root));
    }

    let rel = module.replace('.', "/");
    let mut current = caller_dir.to_path_buf();
    loop {
        for candidate in candidates_for(current.clone(), &rel) {
            if python_module_candidate_is_file(&candidate, repo_root) {
                return python_module_candidate_path(candidate, repo_root);
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    None
}

fn python_module_candidate_is_file(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_file())
        .unwrap_or_else(|| candidate.is_file())
}

fn python_module_candidate_path(candidate: PathBuf, repo_root: Option<&Path>) -> Option<String> {
    if repo_root.is_some() {
        return Some(candidate.to_string_lossy().to_string());
    }
    candidate
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

/// Lazy re-exports of a package (`__init__.py` with a module `__getattr__`,
/// PEP 562) written as a table of `name: (module, attribute)` pairs:
///
/// ```python
/// _LAZY_EXPORTS = {"GraphNode": (".types", "GraphNode")}
///
/// def __getattr__(name):
///     module_name, attr_name = _LAZY_EXPORTS[name]
///     return getattr(import_module(module_name, __name__), attr_name)
/// ```
///
/// Each module of the table is an IMPORTS_FROM with the names it lends
/// (`lazy_export`), as a `from .types import GraphNode` would be, so
/// `from pkg import GraphNode` resolves to the declaration.
fn python_emit_lazy_exports(
    body: &[Stmt],
    context: &PythonParseContext<'_>,
    edges: &mut Vec<ParsedEdge>,
) {
    let has_getattr = body.iter().any(|stmt| {
        matches!(stmt, Stmt::FunctionDef(function)
            if function.decorator_list.is_empty() && function.name.as_str() == "__getattr__")
    });
    if !has_getattr {
        return;
    }
    // (module, [(attribute, exported name)], line), in order of appearance.
    type LazyModule = (String, Vec<(String, String)>, i64);
    let mut by_module: Vec<LazyModule> = Vec::new();
    for stmt in body {
        let value = match stmt {
            Stmt::Assign(assign) if assign.targets.len() == 1 => Some(&*assign.value),
            Stmt::AnnAssign(assign) => assign.value.as_deref(),
            _ => None,
        };
        let Some(Expr::Dict(dictionary)) = value else {
            continue;
        };
        for item in &dictionary.items {
            let (Some(Expr::StringLiteral(key)), Expr::Tuple(value)) = (&item.key, &item.value)
            else {
                continue;
            };
            let [Expr::StringLiteral(module), Expr::StringLiteral(attribute)] =
                value.elts.as_slice()
            else {
                continue;
            };
            let name = key.value.to_str().to_string();
            let module = module.value.to_str().to_string();
            let attribute = attribute.value.to_str().to_string();
            let line = context.line(key);
            match by_module.iter_mut().find(|(known, _, _)| *known == module) {
                Some((_, names, _)) => names.push((attribute, name)),
                None => by_module.push((module, vec![(attribute, name)], line)),
            }
        }
    }
    for (module, names, line) in by_module {
        let Some(target) =
            python_resolve_module_to_file(&module, &context.file_path, context.repo_root)
        else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: context.file_path.to_string(),
            target,
            file_path: context.file_path.clone(),
            line,
            extra: json!({"module": module, "names": names, "lazy_export": true}),
        });
    }
}

/// Binds the parameters of a test or fixture named after a pytest fixture
/// and left unannotated (`tmp_path`, `monkeypatch`, `capsys`) to the type
/// pytest injects, unless the file declares a fixture of that name itself.
fn python_bind_pytest_fixtures(parameters: &ast::Parameters, context: &PythonParseContext<'_>) {
    for parameter in parameters.iter_non_variadic_params() {
        if parameter.parameter.annotation.is_some() || parameter.default.is_some() {
            continue;
        }
        let name = parameter.parameter.name.as_str();
        if context.defined_names.contains(name) {
            continue;
        }
        if let Some(fixture_type) = python_pytest_fixture_type(name) {
            python_bind(name, fixture_type, context);
        }
    }
}
