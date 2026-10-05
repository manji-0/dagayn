use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::member_calls::{CallOrigin, MemberCallBindings};

use super::stdlib::gdscript::{is_godot_class, is_godot_global_function};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child, direct_child_text, first_descendant_text, line_count, line_of, node_text,
};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_gdscript_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "gdscript")];
    let mut edges = Vec::new();
    let mut context = GdscriptParseContext {
        source,
        file_path: file_path.clone(),
        class_names: HashSet::new(),
        consts: HashSet::new(),
        variables: HashSet::new(),
        bindings: RefCell::default(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        gdscript_collect_scope_names(tree.root_node(), &mut context);
        context.bindings =
            RefCell::new(MemberCallBindings::with_types(context.class_names.clone()));
        gdscript_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        gdscript_mark_engine_edges(tree.root_node(), source, &nodes, &mut edges);
        let mut edges = resolve_gdscript_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct GdscriptParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    /// Classes the script declares (`class_name Repo`, `class Inner:`).
    class_names: HashSet<String>,
    /// Constants the script declares (`const Other = preload(...)`), which
    /// name a script or a value the script chose.
    consts: HashSet<String>,
    /// Variables, parameters, and loop variables the script declares.
    variables: HashSet<String>,
    /// The types of the variables in scope (`var s: Store`,
    /// `var s := Store.new()`), and the calls they hold the result of
    /// (`var c = make()`).
    bindings: RefCell<MemberCallBindings>,
}

fn gdscript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "extends_statement" if enclosing_func.is_none() => {
                if let Some(target) = gdscript_extends_target(child, context.source) {
                    edges.push(ParsedEdge::new(
                        crate::core::types::EdgeKind::ImportsFrom,
                        context.file_path.to_string(),
                        target,
                        context.file_path.clone(),
                        line_of(child),
                    ));
                }
                continue;
            }
            "class_name_statement" => {
                if let Some(name) = direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_class(child, context, &name, None, nodes, edges);
                }
                continue;
            }
            "class_definition" => {
                if let Some(name) = direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_class(child, context, &name, enclosing_class, nodes, edges);
                    let scope = match enclosing_class {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name,
                    };
                    if let Some(target) = direct_child(child, &["extends_statement"])
                        .and_then(|extends| gdscript_extends_target(extends, context.source))
                    {
                        edges.push(ParsedEdge::new(
                            crate::core::types::EdgeKind::Inherits,
                            qualify(&context.file_path, &scope, None),
                            target,
                            context.file_path.clone(),
                            line_of(child),
                        ));
                    }
                    if let Some(body) = direct_child(child, &["class_body"]) {
                        let saved = context.bindings.borrow().snapshot();
                        gdscript_walk_children(body, context, Some(&scope), None, nodes, edges);
                        context.bindings.borrow_mut().restore(saved);
                    }
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    if let Some(body) = direct_child(child, &["body"]) {
                        let saved = context.bindings.borrow().snapshot();
                        if let Some(parameters) = child.child_by_field_name("parameters") {
                            gdscript_bind_parameters(parameters, context);
                        }
                        gdscript_walk_children(
                            body,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                        context.bindings.borrow_mut().restore(saved);
                    }
                    continue;
                }
            }
            "call" | "attribute_call" => {
                gdscript_emit_call(child, context, enclosing_class, enclosing_func, edges);
                if let Some(path) = gdscript_preload_path(child, context.source) {
                    edges.push(ParsedEdge::new(
                        crate::core::types::EdgeKind::ImportsFrom,
                        context.file_path.to_string(),
                        path,
                        context.file_path.clone(),
                        line_of(child),
                    ));
                }
            }
            _ => {}
        }
        gdscript_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        // After its value is walked: `var s := s.next()` calls `next` on
        // the `s` before it.
        if child.kind() == "variable_statement" {
            gdscript_bind_variable(child, context);
        }
    }
}

fn gdscript_emit_class(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "gdscript".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn gdscript_emit_function(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
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
        language: "gdscript".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: direct_child_text(node, context.source, &["parameters"]),
        // `func make() -> Store:`: `Store`.
        return_type: node
            .child_by_field_name("return_type")
            .map(|return_type| node_text(return_type, context.source)),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn gdscript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = gdscript_call_name(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    let mut extra = match gdscript_call_path(node, &target, context.source) {
        Some(path) => json!({ GDSCRIPT_CALL_PATH_KEY: path }),
        None => json!({}),
    };
    if node.kind() == "attribute_call" {
        gdscript_mark_receiver(node, &target, context, &mut extra);
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
}

/// Scratch key on a `CALLS` edge: the callee as written from a plain name
/// (`print`, `Node2D.new`), consumed by [`gdscript_mark_engine_edges`].
const GDSCRIPT_CALL_PATH_KEY: &str = "gdscript_call_path";

/// The callee of a call written from a plain name: `print` for `print(x)`,
/// `Node2D.new` for `Node2D.new()`. A method of anything else
/// (`timer.start()` is `t` then `start`, but `a.b.c()` is not) has no
/// path.
fn gdscript_call_path(node: tree_sitter::Node<'_>, name: &str, source: &[u8]) -> Option<String> {
    if node.kind() == "call" {
        return Some(name.to_string());
    }
    let attribute = node
        .parent()
        .filter(|parent| parent.kind() == "attribute")?;
    let root = attribute
        .named_child(0)
        .filter(|root| root.kind() == "identifier")?;
    (attribute.named_child(1)? == node).then(|| format!("{}.{name}", node_text(root, source)))
}

/// Names the script declares: its functions and classes (`nodes`), and its
/// constants (`const Other = preload(...)`), variables, signals, enums,
/// parameters, and loop variables.
fn gdscript_collect_declared_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "const_statement" | "variable_statement" | "signal_statement" | "enum_definition" => {
            if let Some(name) = direct_child(node, &["name"]) {
                names.insert(node_text(name, source));
            }
        }
        "parameters" => {
            let mut cursor = node.walk();
            for parameter in node.named_children(&mut cursor) {
                let name = if parameter.kind() == "identifier" {
                    Some(parameter)
                } else {
                    direct_child(parameter, &["identifier"])
                };
                if let Some(name) = name {
                    names.insert(node_text(name, source));
                }
            }
        }
        "for_statement" => {
            if let Some(name) = direct_child(node, &["identifier"]) {
                names.insert(node_text(name, source));
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        gdscript_collect_declared_names(child, source, names);
    }
}

/// Points the calls and `extends` that reach the Godot engine at `godot`:
/// global scope functions (`print(x)`, `randi()`, `preload(path)`),
/// built-in types and engine classes (`Vector2(1, 2)`, `Node2D.new()`),
/// and `extends Node`. All likely: the engine is not imported, so only the
/// name says so, and a script declaring the name (a function, a
/// `const X = preload(...)`, a variable) keeps it. `res://` paths are
/// never the engine's.
fn gdscript_mark_engine_edges(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
) {
    let mut declared = nodes
        .iter()
        .filter(|node| node.kind != crate::core::types::NodeKind::File)
        .map(|node| node.name.clone())
        .collect::<HashSet<_>>();
    gdscript_collect_declared_names(root, source, &mut declared);
    for edge in edges.iter_mut() {
        let engine = match edge.kind {
            crate::core::types::EdgeKind::ImportsFrom => {
                is_godot_class(&edge.target) && !declared.contains(&edge.target)
            }
            crate::core::types::EdgeKind::Calls => {
                let Some(path) = edge
                    .extra
                    .as_object_mut()
                    .and_then(|extra| extra.remove(GDSCRIPT_CALL_PATH_KEY))
                    .and_then(|path| path.as_str().map(str::to_string))
                else {
                    continue;
                };
                let engine = match path.split_once('.') {
                    Some((root, _)) => is_godot_class(root) && !declared.contains(root),
                    None => {
                        (is_godot_global_function(&path) || is_godot_class(&path))
                            && !declared.contains(&path)
                    }
                };
                if engine {
                    edge.target = path;
                }
                engine
            }
            _ => false,
        };
        if engine {
            mark_stdlib_edge(
                &mut edge.target,
                &mut edge.extra,
                "godot",
                StdlibEvidence::Likely,
            );
        }
    }
}

/// Records the classes, constants, and variables the script declares.
fn gdscript_collect_scope_names(
    node: tree_sitter::Node<'_>,
    context: &mut GdscriptParseContext<'_>,
) {
    let source = context.source;
    match node.kind() {
        "class_name_statement" | "class_definition" => {
            if let Some(name) = direct_child_text(node, source, &["name"]) {
                context.class_names.insert(name);
            }
        }
        "const_statement" => {
            if let Some(name) = direct_child_text(node, source, &["name"]) {
                context.consts.insert(name);
            }
        }
        _ => {}
    }
    if node.kind() == "source" {
        let mut declared = HashSet::new();
        gdscript_collect_declared_names(node, source, &mut declared);
        context.variables = declared;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        gdscript_collect_scope_names(child, context);
    }
}

/// Binds `var` to a type written in the script (`Store`, `Array[int]`
/// is `Array`).
fn gdscript_bind_type(
    var: String,
    type_node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
) {
    let Some(type_name) = first_descendant_text(type_node, context.source, &["identifier"])
        .or_else(|| Some(node_text(type_node, context.source)))
        .filter(|name| !name.is_empty())
    else {
        return;
    };
    let mut bindings = context.bindings.borrow_mut();
    if context.class_names.contains(&type_name) {
        bindings.forget_foreign(&var);
        bindings.bind(var, type_name);
    } else {
        bindings.bind_any(var, type_name);
    }
}

/// Binds the typed parameters of a function (`p: Store`, `p: Store = null`).
fn gdscript_bind_parameters(parameters: tree_sitter::Node<'_>, context: &GdscriptParseContext<'_>) {
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        let Some(name) = direct_child_text(parameter, context.source, &["identifier"]) else {
            continue;
        };
        match parameter
            .child_by_field_name("type")
            .filter(|type_node| type_node.kind() == "type")
        {
            Some(type_node) => gdscript_bind_type(name, type_node, context),
            None => context.bindings.borrow_mut().forget_foreign(&name),
        }
    }
}

/// The class a `Class.new(...)` constructs.
fn gdscript_constructed_class(value: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if value.kind() != "attribute" || value.named_child_count() != 2 {
        return None;
    }
    let class = value
        .named_child(0)
        .filter(|class| class.kind() == "identifier")?;
    let call = value
        .named_child(1)
        .filter(|call| call.kind() == "attribute_call")?;
    (gdscript_call_name(call, source)? == "new").then(|| node_text(class, source))
}

/// Binds a declared variable to what it says: its type (`var s: Store`),
/// the class its value constructs (`var s := Store.new()`), or the call its
/// value is the result of (`var c = make()`, `var c = db.open()`).
fn gdscript_bind_variable(node: tree_sitter::Node<'_>, context: &GdscriptParseContext<'_>) {
    let Some(name) = direct_child_text(node, context.source, &["name"]) else {
        return;
    };
    if let Some(type_node) = node
        .child_by_field_name("type")
        .filter(|type_node| type_node.kind() == "type")
    {
        gdscript_bind_type(name, type_node, context);
        return;
    }
    let value = node.child_by_field_name("value");
    if let Some(class) = value.and_then(|value| gdscript_constructed_class(value, context.source)) {
        let mut bindings = context.bindings.borrow_mut();
        if context.class_names.contains(&class) {
            bindings.forget_foreign(&name);
            bindings.bind(name, class);
        } else {
            bindings.bind_any(name, class);
        }
        return;
    }
    let origin = value.and_then(|value| gdscript_value_origin(value, context));
    let mut bindings = context.bindings.borrow_mut();
    match origin {
        Some(origin) => bindings.bind_returned(name, origin),
        None => bindings.forget_foreign(&name),
    }
}

/// The call a value is the result of: `make()`, the last call of
/// `db.open()`, or a variable holding one.
fn gdscript_value_origin(
    value: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
) -> Option<CallOrigin> {
    match value.kind() {
        "call" => gdscript_call_origin(value, None, context),
        "attribute" => {
            let last = value.named_child(value.named_child_count().checked_sub(1)? as u32)?;
            (last.kind() == "attribute_call")
                .then(|| gdscript_call_origin(last, None, context))
                .flatten()
        }
        "identifier" => context
            .bindings
            .borrow()
            .returned_by(&node_text(value, context.source))
            .cloned(),
        _ => None,
    }
}

/// The origin of a call node (`make()`, `open()` in `db.open()`). In a
/// chain repeating `method` (`q.where(a).where(b)`) it is the call before
/// the repeats, since the repeats share one edge per line.
fn gdscript_call_origin(
    call: tree_sitter::Node<'_>,
    method: Option<&str>,
    context: &GdscriptParseContext<'_>,
) -> Option<CallOrigin> {
    let name = gdscript_call_name(call, context.source)?;
    if method == Some(name.as_str()) {
        let previous = call.prev_named_sibling()?;
        return match previous.kind() {
            "attribute_call" | "call" => gdscript_call_origin(previous, method, context),
            "identifier" if previous.prev_named_sibling().is_none() => context
                .bindings
                .borrow()
                .returned_by(&node_text(previous, context.source))
                .cloned(),
            _ => None,
        };
    }
    Some(CallOrigin {
        name,
        line: call.start_position().row as i64 + 1,
        unwrap: false,
    })
}

/// Records what a method call's receiver says: `receiver_type` for a
/// class of another script (`Store.create()`, or `store.save()` with
/// `store: Store`), the engine's method for an engine type (`timer.start()`
/// with `timer: Timer` is `Timer.start`), or `receiver_unknown` for a value
/// of unknown type (`q.go()` with `q` untyped, `$Label.set_text()`,
/// `make().close()`), with the call it came from (`receiver_from`). `self`,
/// a class of this script, and a constant (`const Other = preload(...)`)
/// keep the script's own binding.
fn gdscript_mark_receiver(
    call: tree_sitter::Node<'_>,
    method: &str,
    context: &GdscriptParseContext<'_>,
    extra: &mut Value,
) {
    let source = context.source;
    let Some(previous) = call.prev_named_sibling() else {
        return;
    };
    let unknown = |extra: &mut Value, origin: Option<CallOrigin>| {
        extra["receiver_unknown"] = json!(true);
        if let Some(origin) = origin {
            extra["receiver_from"] = origin.to_json();
        }
    };
    // `self.store.save()` is `store.save()`.
    let root = match previous.kind() {
        "identifier" => match previous.prev_named_sibling() {
            None => previous,
            Some(owner)
                if owner.kind() == "identifier"
                    && owner.prev_named_sibling().is_none()
                    && node_text(owner, source) == "self" =>
            {
                previous
            }
            Some(_) => return unknown(extra, None),
        },
        "attribute_call" | "call" => {
            let origin = gdscript_call_origin(previous, Some(method), context);
            return unknown(extra, origin);
        }
        _ => return unknown(extra, None),
    };
    let name = node_text(root, source);
    if name == "self" {
        return;
    }
    let bindings = context.bindings.borrow();
    if bindings.is_bound(&name) {
        return;
    }
    if let Some(type_name) = bindings.foreign_type(&name) {
        if is_godot_class(type_name) {
            extra[GDSCRIPT_CALL_PATH_KEY] = json!(format!("{type_name}.{method}"));
        } else {
            extra["receiver_type"] = json!(type_name);
        }
        return;
    }
    if let Some(origin) = bindings.returned_by(&name) {
        return unknown(extra, Some(origin.clone()));
    }
    if context.class_names.contains(&name)
        || context.consts.contains(&name)
        || is_godot_class(&name)
    {
        return;
    }
    let is_class = name.starts_with(|c: char| c.is_ascii_uppercase());
    if is_class && !context.variables.contains(&name) {
        extra["receiver_type"] = json!(name);
    } else {
        unknown(extra, None);
    }
}

fn gdscript_extends_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if let Some(path) = direct_child(node, &["string"]) {
        return gdscript_string_value(path, source);
    }
    let type_node = direct_child(node, &["type"])?;
    first_descendant_text(type_node, source, &["identifier"])
        .or_else(|| Some(node_text(type_node, source).trim().to_string()))
        .filter(|target| !target.is_empty())
}

fn gdscript_string_value(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let value = node_text(node, source)
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    (!value.is_empty()).then_some(value)
}

/// `preload("res://x.gd")` / `load(...)` pull in another script.
fn gdscript_preload_path(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let name = gdscript_call_name(node, source)?;
    if !matches!(name.as_str(), "preload" | "load") {
        return None;
    }
    let arguments = direct_child(node, &["arguments"])?;
    let path = direct_child(arguments, &["string"])?;
    gdscript_string_value(path, source)
}

fn gdscript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    direct_child_text(node, source, &["identifier"])
}

fn resolve_gdscript_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            // A method of a value of another type (`store.save()` with
            // `store: Store`) or of an unknown one is none of the script's.
            if edge.kind == "CALLS"
                && !edge.target.contains("::")
                && edge.extra.get("external").is_none()
                && edge.extra.get("receiver_type").is_none()
                && edge.extra["receiver_unknown"] != true
                && let Some(target) = symbols.get(&edge.target)
            {
                edge.target = target.clone();
            }
            edge
        })
        .collect()
}
