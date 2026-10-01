use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::member_calls::{CallOrigin, MemberCallBindings};

use super::stdlib::lua::{is_lua_base_function, lua_std_library};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, node_text_is, strip_matching_quotes};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_lua_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_lua_like_with_parser(file_path, source, "lua", parser)
}

fn parse_lua_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let mut context = LuaParseContext {
        source,
        file_path: file_path.clone(),
        language,
        bound_names: HashSet::new(),
        std_aliases: HashMap::new(),
        tables: HashSet::new(),
        values: RefCell::default(),
        bindings: RefCell::default(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        lua_collect_bound_names(tree.root_node(), &mut context);
        context.bindings = RefCell::new(MemberCallBindings::with_types(context.tables.clone()));
        lua_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        lua_mark_std_imports(&mut edges);
        let mut edges = resolve_lua_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct LuaParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    language: &'a str,
    /// Every name the file binds: locals, parameters, loop variables,
    /// functions, and globals it assigns. Any of them may shadow a standard
    /// library (`local string = "x"`) or base function (`function print()`).
    bound_names: HashSet<String>,
    /// Names bound to a standard library by requiring it
    /// (`local str = require("string")`): `str.format` is `string.format`.
    std_aliases: HashMap<String, &'static str>,
    /// Tables the file declares (`local M = {}`, `function Store.new()`),
    /// which `Store.new()` constructs values of.
    tables: HashSet<String>,
    /// Locals of the function being walked that hold a value rather than a
    /// module table: its parameters, loop variables, and locals assigned
    /// anything but a table, a function, or a `require`.
    values: RefCell<HashSet<String>>,
    /// The types of the locals holding a constructed value
    /// (`local s = Store.new()`) or a call's result (`local c = connect()`).
    bindings: RefCell<MemberCallBindings>,
}

fn lua_collect_bound_names(node: tree_sitter::Node<'_>, context: &mut LuaParseContext<'_>) {
    let source = context.source;
    let mut aliased = HashSet::new();
    if node.kind() == "assignment_statement"
        && let (Some(variables), Some(values)) = (
            lua_direct_child(node, &["variable_list"]),
            lua_direct_child(node, &["expression_list"]),
        )
    {
        let mut cursor = variables.walk();
        let names: Vec<_> = variables.named_children(&mut cursor).collect();
        let mut cursor = values.walk();
        for (name, value) in names.iter().zip(values.named_children(&mut cursor)) {
            if name.kind() == "identifier"
                && value.kind() == "function_call"
                && let Some(library) = lua_require_target(value, source)
                    .as_deref()
                    .and_then(lua_std_library)
            {
                context
                    .std_aliases
                    .insert(node_text(*name, source), library);
                aliased.insert(name.id());
            }
        }
    }
    lua_collect_table(node, context);
    if matches!(
        node.kind(),
        "variable_list" | "parameters" | "for_numeric_clause" | "function_declaration"
    ) {
        let mut cursor = node.walk();
        for name in node.children_by_field_name("name", &mut cursor) {
            if name.kind() == "identifier" {
                context.bound_names.insert(node_text(name, source));
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_list" && !aliased.is_empty() {
            let mut names = child.walk();
            for name in child.children_by_field_name("name", &mut names) {
                if name.kind() == "identifier" && !aliased.contains(&name.id()) {
                    context.bound_names.insert(node_text(name, source));
                }
            }
            continue;
        }
        lua_collect_bound_names(child, context);
    }
}

/// Marks `require("string")` as an import of the standard library.
fn lua_mark_std_imports(edges: &mut [ParsedEdge]) {
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::ImportsFrom {
            continue;
        }
        if let Some(library) = lua_std_library(&edge.target) {
            mark_stdlib_edge(
                &mut edge.target,
                &mut edge.extra,
                library,
                StdlibEvidence::Certain,
            );
        }
    }
}

/// The standard library a call reaches, with the symbol it names: through a
/// library table the file does not rebind (`string.format`,
/// `io.stdout:write`) or a local requiring one (`str.format` after
/// `local str = require("string")`) certainly, or a base function called
/// bare (`print`, `pairs`) likely, since a global of the same name may come
/// from anywhere. `self:print()` is a method, not `print`.
fn lua_std_call(
    callee: tree_sitter::Node<'_>,
    call_name: &str,
    context: &LuaParseContext<'_>,
) -> Option<(&'static str, String, StdlibEvidence)> {
    if callee.kind() == "identifier" {
        return (is_lua_base_function(call_name) && !context.bound_names.contains(call_name))
            .then(|| ("_G", call_name.to_string(), StdlibEvidence::Likely));
    }
    let (head, rest) = call_name.split_once('.')?;
    let library = match context.std_aliases.get(head) {
        Some(library) => *library,
        None => lua_std_library(head).filter(|library| !context.bound_names.contains(*library))?,
    };
    Some((
        library,
        format!("{library}.{rest}"),
        StdlibEvidence::Certain,
    ))
}

fn lua_walk_children(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        lua_visit(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        // After its values are walked: `s = s:next()` calls `next` on the
        // previous `s`.
        match child.kind() {
            "variable_declaration" => {
                if let Some(assign) = lua_direct_child(child, &["assignment_statement"]) {
                    lua_bind_assignment(assign, context);
                } else if let Some(names) = lua_direct_child(child, &["variable_list"]) {
                    // `local s`: a value, assigned later.
                    lua_bind_values(names, context);
                }
            }
            "assignment_statement" => lua_bind_assignment(child, context),
            "for_generic_clause" | "for_numeric_clause" => lua_bind_values(child, context),
            _ => {}
        }
    }
}

fn lua_visit(
    child: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    match child.kind() {
        "variable_declaration"
            if lua_handle_variable_declaration(
                child,
                context,
                enclosing_class,
                enclosing_func,
                nodes,
                edges,
            ) =>
        {
            return;
        }
        "assignment_statement"
            if lua_emit_assigned_functions(
                child,
                context,
                enclosing_class,
                enclosing_func,
                None,
                nodes,
                edges,
            ) =>
        {
            return;
        }
        "function_declaration" => {
            if let Some((parent, name)) = lua_table_function_name(child, context.source) {
                lua_emit_function(child, context, &name, Some(&parent), nodes, edges);
                lua_walk_function(child, context, Some(&parent), Some(&name), nodes, edges);
                return;
            }
            if let Some(name) = lua_direct_child_text(child, context.source, &["identifier"]) {
                // `local function f` in a function body is local to it.
                let is_local = lua_direct_child(child, &["local"]).is_some();
                let local_parent = is_local
                    .then(|| lua_local_parent(enclosing_class, enclosing_func))
                    .flatten();
                let parent = local_parent.as_deref().or(enclosing_class);
                lua_emit_function(child, context, &name, parent, nodes, edges);
                lua_walk_function(child, context, parent, Some(&name), nodes, edges);
                return;
            }
        }
        "function_definition" => {
            // A function passed as a value (`each(xs, function(x) ... end)`).
            lua_walk_function(
                child,
                context,
                enclosing_class,
                enclosing_func,
                nodes,
                edges,
            );
            return;
        }
        "function_call" => {
            if enclosing_func.is_none()
                && let Some(target) = lua_require_target(child, context.source)
            {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: context.file_path.to_string(),
                    target,
                    file_path: context.file_path.clone(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                return;
            }
            lua_emit_call(child, context, enclosing_class, enclosing_func, edges);
        }
        _ => {}
    }
    lua_walk_children(
        child,
        context,
        enclosing_class,
        enclosing_func,
        nodes,
        edges,
    );
}

fn lua_handle_variable_declaration(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(assign) = lua_direct_child(node, &["assignment_statement"]) else {
        return false;
    };
    let Some(var_name) = lua_assignment_variable_name(assign, context.source) else {
        return false;
    };
    let Some(expr_list) = lua_direct_child(assign, &["expression_list"]) else {
        return false;
    };

    let mut cursor = expr_list.walk();
    for expr in expr_list.children(&mut cursor) {
        if expr.kind() == "function_call"
            && let Some(target) = lua_require_target(expr, context.source)
        {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom,
                source: context.file_path.to_string(),
                target,
                file_path: context.file_path.clone(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            });
            return true;
        }
    }

    let _ = var_name;
    // `local f = function` in a function body is local to it.
    let local_parent = lua_local_parent(enclosing_class, enclosing_func);
    lua_emit_assigned_functions(
        assign,
        context,
        enclosing_class,
        enclosing_func,
        local_parent.as_deref(),
        nodes,
        edges,
    )
}

/// The parent of a declaration local to the function being walked.
fn lua_local_parent(enclosing_class: Option<&str>, enclosing_func: Option<&str>) -> Option<String> {
    let func = enclosing_func?;
    Some(match enclosing_class {
        Some(class) => format!("{class}.{func}"),
        None => func.to_string(),
    })
}

/// Emits functions bound by assignment: `f = function`, `M.a.h = function`,
/// and `t = { cb = function ... }`. Other values are walked normally.
/// Returns whether any function was bound.
fn lua_emit_assigned_functions(
    assign: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    local_parent: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let (Some(variables), Some(values)) = (
        lua_direct_child(assign, &["variable_list"]),
        lua_direct_child(assign, &["expression_list"]),
    ) else {
        return false;
    };
    let mut cursor = variables.walk();
    let targets: Vec<_> = variables.named_children(&mut cursor).collect();
    let mut cursor = values.walk();
    let exprs: Vec<_> = values.named_children(&mut cursor).collect();
    if !exprs
        .iter()
        .any(|expr| matches!(expr.kind(), "function_definition" | "table_constructor"))
    {
        return false;
    }
    let mut bound = false;
    for (index, expr) in exprs.iter().enumerate() {
        let target = targets.get(index).copied();
        let binding = target.and_then(|target| lua_binding_path(target, context.source));
        match (expr.kind(), binding) {
            ("function_definition", Some((parent, name))) => {
                let parent = parent.as_deref().or(local_parent).or(enclosing_class);
                lua_emit_function(*expr, context, &name, parent, nodes, edges);
                lua_walk_function(*expr, context, parent, Some(&name), nodes, edges);
                bound = true;
            }
            ("table_constructor", Some((parent, name))) => {
                let table = match parent {
                    Some(parent) => format!("{parent}.{name}"),
                    None => name,
                };
                let mut fields = expr.walk();
                for field in expr.named_children(&mut fields) {
                    let key = field
                        .child_by_field_name("name")
                        .filter(|key| key.kind() == "identifier");
                    let value = field
                        .child_by_field_name("value")
                        .filter(|value| value.kind() == "function_definition");
                    if let (Some(key), Some(value)) = (key, value) {
                        let key = node_text(key, context.source);
                        lua_emit_function(value, context, &key, Some(&table), nodes, edges);
                        lua_walk_function(value, context, Some(&table), Some(&key), nodes, edges);
                        bound = true;
                    } else {
                        lua_walk_children(
                            field,
                            context,
                            enclosing_class,
                            enclosing_func,
                            nodes,
                            edges,
                        );
                    }
                }
            }
            _ => lua_walk_children(
                *expr,
                context,
                enclosing_class,
                enclosing_func,
                nodes,
                edges,
            ),
        }
    }
    bound
}

/// `(parent, name)` for an assignment target: `f` -> (None, f),
/// `M.a.h` -> (Some("M.a"), h).
fn lua_binding_path(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Option<String>, String)> {
    match node.kind() {
        "identifier" => Some((None, node_text(node, source))),
        "dot_index_expression" | "method_index_expression" => {
            let table = node.child_by_field_name("table")?;
            let field = node
                .child_by_field_name("field")
                .or_else(|| node.child_by_field_name("method"))?;
            let parent = node_text(table, source).replace(':', ".");
            Some((Some(parent), node_text(field, source)))
        }
        _ => None,
    }
}

fn lua_emit_function(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: lua_first_descendant_text(node, context.source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn lua_emit_call(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(mut call_name) = lua_call_name(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    let mut extra = json!({});
    if let Some(callee) = lua_call_callee(node)
        && let Some((package, symbol, evidence)) = lua_std_call(callee, &call_name, context)
    {
        call_name = symbol;
        mark_stdlib_edge(&mut call_name, &mut extra, package, evidence);
    } else if let Some(callee) = lua_call_callee(node) {
        lua_mark_receiver(callee, context, &mut call_name, &mut extra);
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
    if let Some(signature) = lua_call_signature(node, context.source)
        && let Some(edge) = lua_bridge_edge(node, context, &caller, &signature)
    {
        edges.push(edge);
    }
}

/// Records a table the file declares: `local M = {}` / `Store = {}`, or
/// the table a function is declared in (`function Store.new()`).
fn lua_collect_table(node: tree_sitter::Node<'_>, context: &mut LuaParseContext<'_>) {
    let source = context.source;
    match node.kind() {
        "function_declaration" => {
            if let Some((parent, _)) = lua_table_function_name(node, source) {
                context.tables.insert(parent);
            }
        }
        "assignment_statement" => {
            let (Some(variables), Some(values)) = (
                lua_direct_child(node, &["variable_list"]),
                lua_direct_child(node, &["expression_list"]),
            ) else {
                return;
            };
            let mut cursor = variables.walk();
            let names: Vec<_> = variables.named_children(&mut cursor).collect();
            let mut cursor = values.walk();
            for (name, value) in names.iter().zip(values.named_children(&mut cursor)) {
                if value.kind() == "table_constructor"
                    && let Some((parent, name)) = lua_binding_path(*name, source)
                {
                    context.tables.insert(match parent {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name,
                    });
                }
            }
        }
        _ => {}
    }
}

/// Walks a function body with its parameters as values, and the locals
/// it binds dropped afterwards.
fn lua_walk_function(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let values = context.values.borrow().clone();
    let bindings = context.bindings.borrow().snapshot();
    if let Some(parameters) = node.child_by_field_name("parameters") {
        lua_bind_values(parameters, context);
    }
    lua_walk_children(node, context, enclosing_class, enclosing_func, nodes, edges);
    *context.values.borrow_mut() = values;
    context.bindings.borrow_mut().restore(bindings);
}

/// Marks the names a parameter list, `local` list, or loop clause binds
/// as values of unknown type (`function(conn)`, `for _, row in ...`).
fn lua_bind_values(node: tree_sitter::Node<'_>, context: &LuaParseContext<'_>) {
    let list = match node.kind() {
        "for_generic_clause" => lua_direct_child(node, &["variable_list"]),
        _ => Some(node),
    };
    let Some(list) = list else {
        return;
    };
    let mut cursor = list.walk();
    for name in list.children_by_field_name("name", &mut cursor) {
        if name.kind() == "identifier" {
            let name = node_text(name, context.source);
            context.bindings.borrow_mut().forget_foreign(&name);
            context.values.borrow_mut().insert(name);
        }
    }
}

/// The table a `T.new(...)` / `T:new(...)` call constructs a value of
/// (`Store`, `models.Store`), as written.
fn lua_constructed_table(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(call)?;
    if !matches!(
        callee.kind(),
        "dot_index_expression" | "method_index_expression"
    ) {
        return None;
    }
    let method = callee
        .child_by_field_name("field")
        .or_else(|| callee.child_by_field_name("method"))?;
    let table = callee.child_by_field_name("table")?;
    (node_text(method, source) == "new"
        && matches!(table.kind(), "identifier" | "dot_index_expression"))
    .then(|| node_text(table, source))
}

/// Binds the locals an assignment sets to what their values say: the table
/// constructing them (`local s = Store.new()`), the call they are the
/// result of (`local c = connect()`), or the local they copy. A table, a
/// function, or a `require` is a module rather than a value.
fn lua_bind_assignment(assign: tree_sitter::Node<'_>, context: &LuaParseContext<'_>) {
    let source = context.source;
    let (Some(variables), Some(values)) = (
        lua_direct_child(assign, &["variable_list"]),
        lua_direct_child(assign, &["expression_list"]),
    ) else {
        return;
    };
    let mut cursor = variables.walk();
    let names: Vec<_> = variables.named_children(&mut cursor).collect();
    let mut cursor = values.walk();
    for (name, value) in names.iter().zip(values.named_children(&mut cursor)) {
        if name.kind() != "identifier" {
            continue;
        }
        let var = node_text(*name, source);
        let mut bindings = context.bindings.borrow_mut();
        bindings.forget_foreign(&var);
        let is_value = match value.kind() {
            "table_constructor" | "function_definition" => false,
            "function_call" if lua_require_target(value, source).is_some() => false,
            "function_call" => {
                if let Some(table) = lua_constructed_table(value, source) {
                    if context.tables.contains(&table) {
                        bindings.bind(var.clone(), table);
                    } else {
                        let class = table.rsplit('.').next().unwrap_or(&table).to_string();
                        bindings.bind_any(var.clone(), class);
                    }
                } else if let Some(origin) = lua_call_origin(value, None, context, &bindings) {
                    bindings.bind_returned(var.clone(), origin);
                }
                true
            }
            "identifier" => {
                let other = node_text(value, source);
                if let Some(bound) = bindings.bound_type(&other).map(str::to_string) {
                    bindings.bind(var.clone(), bound);
                } else if let Some(foreign) = bindings.foreign_type(&other).map(str::to_string) {
                    bindings.bind_any(var.clone(), foreign);
                } else if let Some(origin) = bindings.returned_by(&other).cloned() {
                    bindings.bind_returned(var.clone(), origin);
                }
                context.values.borrow().contains(&other)
            }
            _ => true,
        };
        drop(bindings);
        if is_value {
            context.values.borrow_mut().insert(var);
        } else {
            context.values.borrow_mut().remove(&var);
        }
    }
}

/// The call an expression is the result of (`connect()`, `db:open()`), or
/// of which a local holds the result. In a chain repeating `method`
/// (`q:where(a):where(b)`) it is the call before the repeats, since the
/// repeats share one edge per line.
fn lua_call_origin(
    expression: tree_sitter::Node<'_>,
    method: Option<&str>,
    context: &LuaParseContext<'_>,
    bindings: &MemberCallBindings,
) -> Option<CallOrigin> {
    match expression.kind() {
        "function_call" => {
            let callee = lua_call_callee(expression)?;
            let name = match callee.kind() {
                "identifier" => callee,
                "dot_index_expression" | "method_index_expression" => callee
                    .child_by_field_name("field")
                    .or_else(|| callee.child_by_field_name("method"))?,
                _ => return None,
            };
            let name = node_text(name, context.source);
            if method == Some(name.as_str()) {
                let inner = callee.child_by_field_name("table")?;
                return lua_call_origin(inner, method, context, bindings);
            }
            Some(CallOrigin {
                name,
                line: expression.start_position().row as i64 + 1,
                unwrap: false,
            })
        }
        "identifier" => bindings
            .returned_by(&node_text(expression, context.source))
            .cloned(),
        _ => None,
    }
}

/// Rewrites a method call on a value by what its receiver says: a value of
/// a table of this file calls the table's function (`s:save()` after
/// `local s = Store.new()` is `Store.save`); of a table of another file,
/// the bare method with `receiver_type` (`Store` after `local Store =
/// require("store")`); of an unknown type (a parameter, a call's result,
/// `self.db`), the bare method with `receiver_unknown` (and
/// `receiver_from` for a call's result). A module table (`M.run()`,
/// `json.encode()`) and `self` are left alone.
fn lua_mark_receiver(
    callee: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    call_name: &mut String,
    extra: &mut Value,
) {
    if !matches!(
        callee.kind(),
        "dot_index_expression" | "method_index_expression"
    ) {
        return;
    }
    let (Some(table), Some(method)) = (
        callee.child_by_field_name("table"),
        callee
            .child_by_field_name("field")
            .or_else(|| callee.child_by_field_name("method")),
    ) else {
        return;
    };
    let source = context.source;
    let method = node_text(method, source);
    let bindings = context.bindings.borrow();
    let unknown = |origin: Option<CallOrigin>, call_name: &mut String, extra: &mut Value| {
        *call_name = method.clone();
        extra["receiver_unknown"] = json!(true);
        if let Some(origin) = origin {
            extra["receiver_from"] = origin.to_json();
        }
    };
    match table.kind() {
        "identifier" => {
            let name = node_text(table, source);
            if name == "self" {
                return;
            }
            if let Some(bound) = bindings.bound_type(&name) {
                *call_name = format!("{bound}.{method}");
            } else if let Some(foreign) = bindings.foreign_type(&name) {
                extra["receiver_type"] = json!(foreign);
                *call_name = method;
            } else if let Some(origin) = bindings.returned_by(&name) {
                unknown(Some(origin.clone()), call_name, extra);
            } else if context.values.borrow().contains(&name) {
                unknown(None, call_name, extra);
            }
        }
        "function_call" => {
            if lua_require_target(table, source).is_some() {
                return;
            }
            match lua_constructed_table(table, source) {
                Some(constructed) if context.tables.contains(&constructed) => {
                    *call_name = format!("{constructed}.{method}");
                }
                Some(constructed) => {
                    let class = constructed.rsplit('.').next().unwrap_or(&constructed);
                    extra["receiver_type"] = json!(class);
                    *call_name = method;
                }
                None => {
                    let origin = lua_call_origin(table, Some(&method), context, &bindings);
                    unknown(origin, call_name, extra);
                }
            }
        }
        "dot_index_expression" | "method_index_expression" | "bracket_index_expression" => {
            // A field of a value (`self.db:exec()`, `row.items:add()`); a
            // path from a module table (`M.sub.run()`) is the module's.
            let mut root = table;
            while matches!(
                root.kind(),
                "dot_index_expression" | "method_index_expression" | "bracket_index_expression"
            ) {
                let Some(inner) = root.child_by_field_name("table") else {
                    return;
                };
                root = inner;
            }
            let value = match root.kind() {
                "identifier" => {
                    let name = node_text(root, source);
                    name == "self"
                        || context.values.borrow().contains(&name)
                        || bindings.returned_by(&name).is_some()
                        || bindings.foreign_type(&name).is_some()
                        || bindings.is_bound(&name)
                }
                "function_call" => lua_require_target(root, source).is_none(),
                _ => false,
            };
            if value {
                unknown(None, call_name, extra);
            }
        }
        _ => {}
    }
}

fn lua_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "dot_index_expression" | "method_index_expression" => {
            let (parent, name) = lua_binding_path(callee, source)?;
            Some(match parent {
                Some(parent) if parent != "self" => format!("{parent}.{name}"),
                _ => name,
            })
        }
        _ => None,
    }
}

fn lua_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    let signature = match callee.kind() {
        "identifier" => node_text(callee, source),
        "dot_index_expression" | "method_index_expression" => node_text(callee, source)
            .replace(':', ".")
            .trim()
            .to_string(),
        _ => return None,
    };
    (!signature.is_empty()).then_some(signature)
}

fn lua_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "arguments")
}

fn lua_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "os.execute" | "io.popen" => ("invokes_binary", "subprocess"),
        "io.open" => ("opens_file", "file_io"),
        "io.lines" | "io.read" => ("reads_file", "file_io"),
        "io.write" => ("writes_file", "file_io"),
        // LuaJIT: `local lib = ffi.load("fastsum")`.
        "package.loadlib" | "loadlib" | "ffi.load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match lua_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn lua_require_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = lua_call_callee(node)?;
    if first.kind() != "identifier" || !node_text_is(first, source, "require") {
        return None;
    }
    lua_first_string_arg(node, source)
}

fn lua_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = lua_direct_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(lua_string_text(child, source));
        }
        return None;
    }
    None
}

fn lua_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if let Some(content) = lua_first_descendant_text(node, source, &["string_content"]) {
        return content;
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn lua_table_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<(String, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some((Some(parent), name)) = lua_binding_path(child, source) {
            return Some((parent, name));
        }
    }
    None
}

fn lua_assignment_variable_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let variable_list = lua_direct_child(node, &["variable_list"])?;
    lua_first_descendant_text(variable_list, source, &["identifier"])
}

fn lua_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn lua_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    lua_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn lua_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = lua_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_lua_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    // `M.f` resolves through its table path; a bare name prefers a free
    // function, then a sibling in the caller's table.
    let mut by_path = HashMap::<String, String>::new();
    let mut by_name = HashMap::<&str, Vec<(Option<&str>, String)>>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        if let Some(parent) = node.parent_name.as_deref() {
            by_path
                .entry(format!("{parent}.{}", node.name))
                .or_insert_with(|| qualified.clone());
        }
        by_name
            .entry(node.name.as_str())
            .or_default()
            .push((node.parent_name.as_deref(), qualified));
    }
    let prefix = format!("{file_path}::");
    edges
        .into_iter()
        .map(|mut edge| {
            let external = edge.extra.get("external").and_then(|value| value.as_bool());
            if edge.kind != "CALLS" || edge.target.contains("::") || external == Some(true) {
                return edge;
            }
            // A method of a value (`conn:close()`) is none of the file's
            // functions, unless its type is a table of the file, which
            // `lua_mark_receiver` already wrote as `Store.save`.
            if edge.extra["receiver_unknown"] == true || edge.extra.get("receiver_type").is_some() {
                return edge;
            }
            if let Some(target) = by_path.get(&edge.target) {
                edge.target = target.clone();
                return edge;
            }
            // Unresolved `lib.fn` keeps its historical bare-name target.
            let name = match edge.target.rsplit_once('.') {
                Some((_, name)) => name.to_string(),
                None => edge.target.clone(),
            };
            let dotted = edge.target.contains('.');
            match by_name.get(name.as_str()) {
                Some(candidates) if !dotted => {
                    let caller = edge.source.strip_prefix(&prefix);
                    let caller_table =
                        caller.and_then(|rest| rest.rsplit_once('.').map(|(table, _)| table));
                    // A `local function` of the caller shadows every other.
                    let chosen = candidates
                        .iter()
                        .find(|(parent, _)| caller.is_some() && *parent == caller)
                        .or_else(|| candidates.iter().find(|(parent, _)| parent.is_none()))
                        .or_else(|| {
                            candidates
                                .iter()
                                .find(|(parent, _)| *parent == caller_table)
                        })
                        .unwrap_or(&candidates[0]);
                    edge.target = chosen.1.clone();
                }
                _ => edge.target = name,
            }
            edge
        })
        .collect()
}
