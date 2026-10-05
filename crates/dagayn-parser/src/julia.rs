use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::member_calls::{CallOrigin, MemberCallBindings};
use super::stdlib::julia::{is_julia_base_export, is_julia_stdlib_module, julia_module_exports};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};

use super::util::{
    direct_child, direct_child_text, direct_child_texts, first_descendant, first_descendant_text,
    last_descendant_text, line_count, line_of, node_text, resolve_import_path,
    set_namespaces_from_type_names, strip_matching_quotes,
};
use super::{add_tested_by_edges, is_test_function, qualify, resolve_rust_call_targets};

pub(super) fn parse_julia_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "julia")];
    let mut edges = Vec::new();
    let mut context = JuliaParseContext {
        source,
        file_path: file_path.clone(),
        repo_root,
        imports: RefCell::default(),
        types: HashSet::new(),
        values: RefCell::default(),
        bindings: RefCell::default(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        julia_collect_types(tree.root_node(), source, &mut context.types);
        context.bindings = RefCell::new(MemberCallBindings::with_types(context.types.clone()));
        julia_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        set_namespaces_from_type_names(&mut nodes);
        julia_mark_stdlib_calls(&nodes, &mut edges, &context.imports.borrow());
        let mut edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct JuliaParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    repo_root: Option<&'a Path>,
    imports: RefCell<JuliaImports>,
    /// Types the file declares (`struct Store`), by name.
    types: HashSet<String>,
    /// Parameters and variables of the function being walked, which hold
    /// values rather than name modules.
    values: RefCell<HashSet<String>>,
    /// The declared types of the parameters in scope (`s::Store`), and the
    /// calls variables hold the result of (`c = connect()`).
    bindings: RefCell<MemberCallBindings>,
}

/// What the file's `using` / `import` statements bring into scope.
#[derive(Default)]
struct JuliaImports {
    /// Names imported one by one, with their module (`mean` → `Statistics`
    /// after `using Statistics: mean`).
    names: HashMap<String, String>,
    /// Modules whose exports a `using` brings in whole (`using
    /// LinearAlgebra`).
    modules: Vec<String>,
}

fn julia_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        julia_visit(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

/// Handles one node, then descends into it unless an arm consumed it.
fn julia_visit(
    child: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    match child.kind() {
        "module_definition" => {
            if let Some(name) = direct_child_text(child, context.source, &["identifier"]) {
                julia_emit_class(
                    child,
                    context,
                    JuliaClassSpec {
                        name: &name,
                        parent_name: None,
                        extra: json!({"type_role": "class"}),
                        contains_from_parent: true,
                    },
                    nodes,
                    edges,
                );
                if let Some(block) = direct_child(child, &["block"]) {
                    julia_walk_children(block, context, Some(&name), None, nodes, edges);
                }
                return;
            }
        }
        "using_statement" | "import_statement" => {
            for target in julia_import_targets(child, context.source) {
                let mut edge = ParsedEdge::new(
                    crate::core::types::EdgeKind::ImportsFrom,
                    context.file_path.to_string(),
                    target,
                    context.file_path.clone(),
                    line_of(child),
                );
                julia_record_import(child, context, &mut edge);
                edges.push(edge);
            }
            return;
        }
        "export_statement" | "public_statement" => {
            julia_emit_symbol_references(child, context, enclosing_class, edges);
            return;
        }
        "macrocall_expression"
            if julia_handle_macrocall(
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
        "abstract_definition" | "struct_definition" => {
            if let Some(name) = julia_type_name(child, context.source) {
                let extra = if child.kind() == "abstract_definition" {
                    json!({"type_role": "abstract_type", "is_abstract": true})
                } else {
                    json!({"type_role": "struct"})
                };
                julia_emit_class(
                    child,
                    context,
                    JuliaClassSpec {
                        name: &name,
                        parent_name: enclosing_class,
                        extra,
                        contains_from_parent: true,
                    },
                    nodes,
                    edges,
                );
                if child.kind() == "struct_definition" {
                    julia_emit_inheritance(child, context, &name, enclosing_class, edges);
                }
                return;
            }
        }
        "function_definition" | "macro_definition" => {
            if let Some(name) = julia_function_name(child, context.source) {
                let functor = direct_child(child, &["signature"])
                    .and_then(|signature| first_descendant(signature, &["call_expression"]))
                    .and_then(|call| julia_functor_type(call, context.source))
                    .map(|ty| match enclosing_class {
                        Some(module) => format!("{module}.{ty}"),
                        None => ty,
                    });
                let parent = functor
                    .clone()
                    .or_else(|| julia_function_parent(enclosing_class, enclosing_func));
                julia_emit_function(child, context, &name, parent.as_deref(), nodes, edges);
                julia_emit_owner_reference(child, context, &name, parent.as_deref(), edges);
                if let Some(block) = direct_child(child, &["block"]) {
                    let saved = julia_enter_function(child, context);
                    julia_walk_children(
                        block,
                        context,
                        functor.as_deref().or(enclosing_class),
                        Some(&name),
                        nodes,
                        edges,
                    );
                    julia_leave_function(saved, context);
                }
                return;
            }
        }
        "assignment"
            if julia_handle_short_function(
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
        "call_expression" => {
            if julia_is_signature_call(child) || julia_is_assignment_lhs_call(child) {
                return;
            }
            if let Some(call_name) = julia_call_name(child, context.source) {
                if call_name == "include"
                    && let Some(target) = julia_first_string_arg(child, context.source)
                {
                    // `include` is relative to the including file.
                    let target = resolve_import_path(
                        &target,
                        &context.file_path,
                        context.repo_root,
                        &[],
                        false,
                    )
                    .unwrap_or(target);
                    edges.push(ParsedEdge::new(
                        crate::core::types::EdgeKind::ImportsFrom,
                        context.file_path.to_string(),
                        target,
                        context.file_path.clone(),
                        line_of(child),
                    ));
                }
                julia_emit_call(
                    child,
                    context,
                    &call_name,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
        }
        _ => {}
    }
    julia_walk_children(
        child,
        context,
        enclosing_class,
        enclosing_func,
        nodes,
        edges,
    );
    // After its value is walked: `c = c.next()` reads the `c` before it.
    if child.kind() == "assignment" {
        julia_bind_assignment(child, context);
    }
}

fn julia_handle_short_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(lhs) = julia_assignment_lhs_call(node) else {
        return false;
    };
    let Some(name) = julia_call_name(lhs, context.source) else {
        return false;
    };
    let parent = julia_function_parent(enclosing_class, enclosing_func);
    julia_emit_function(node, context, &name, parent.as_deref(), nodes, edges);
    julia_emit_owner_reference(node, context, &name, parent.as_deref(), edges);
    let saved = julia_enter_function(node, context);
    let mut seen_operator = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !seen_operator {
            if child.kind() == "operator" {
                seen_operator = true;
            }
            continue;
        }
        // Under the same parent the node was emitted with, so the body's
        // calls come from `outer.f` when `f(x) = ...` is local to `outer`.
        julia_visit(child, context, parent.as_deref(), Some(&name), nodes, edges);
    }
    julia_leave_function(saved, context);
    true
}

fn julia_handle_macrocall(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(macro_name) = julia_macro_name(node, context.source) else {
        return false;
    };
    match macro_name.as_str() {
        "enum" => {
            julia_emit_enum(node, context, enclosing_class, nodes, edges);
            true
        }
        "testset" => {
            julia_emit_testset(node, context, enclosing_class, enclosing_func, nodes, edges);
            true
        }
        _ => {
            // `@inline f(x) = ...` annotates a definition; it is not a call site.
            if !julia_macro_wraps_definition(node) {
                julia_emit_call(
                    node,
                    context,
                    &format!("@{macro_name}"),
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "macro_argument_list" {
                    julia_walk_children(
                        child,
                        context,
                        enclosing_class,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            true
        }
    }
}

struct JuliaClassSpec<'a> {
    name: &'a str,
    parent_name: Option<&'a str>,
    extra: Value,
    contains_from_parent: bool,
}

fn julia_emit_class(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    spec: JuliaClassSpec<'_>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, spec.name, spec.parent_name);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: spec.name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: spec.parent_name.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: spec.extra,
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        if spec.contains_from_parent {
            spec.parent_name
                .map(|parent| qualify(&context.file_path, parent, None))
                .unwrap_or_else(|| context.file_path.to_string())
        } else {
            context.file_path.to_string()
        },
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn julia_emit_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    parent_name: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, parent_name);
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
        language: "julia".to_string(),
        parent_name: parent_name.map(str::to_string),
        params: None,
        return_type: julia_return_type(node, context.source),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        parent_name
            .map(|parent| qualify(&context.file_path, parent, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn julia_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    call_name: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    // `Base.max(...)` / `LinearAlgebra.norm(...)`: the path the call is
    // named by, which `call_name` drops. Read (and dropped) by
    // `julia_mark_stdlib_calls`.
    let mut extra = match julia_call_signature(node, context.source) {
        Some(path) if path.contains('.') => json!({"stdlib_path": path}),
        _ => json!({}),
    };
    julia_mark_receiver(node, context, &mut extra);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
    if let Some(edge) = julia_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

/// The bindings saved around a function body.
type JuliaSavedScope = (HashSet<String>, crate::core::member_calls::BindingsSnapshot);

/// Enters a function body: its parameters are values, typed ones bound to
/// their type (`s::Store`).
fn julia_enter_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
) -> JuliaSavedScope {
    let saved = (
        context.values.borrow().clone(),
        context.bindings.borrow().snapshot(),
    );
    let call = match node.kind() {
        "assignment" => julia_assignment_lhs_call(node),
        _ => direct_child(node, &["signature"])
            .and_then(|signature| first_descendant(signature, &["call_expression"])),
    };
    if let Some(arguments) = call.and_then(|call| direct_child(call, &["argument_list"])) {
        let mut cursor = arguments.walk();
        for argument in arguments.named_children(&mut cursor) {
            let (name, type_name) = match argument.kind() {
                "identifier" => (node_text(argument, context.source), None),
                "typed_expression" => {
                    let mut cursor = argument.walk();
                    let parts = argument.named_children(&mut cursor).collect::<Vec<_>>();
                    match parts.as_slice() {
                        [name, type_name]
                            if name.kind() == "identifier" && type_name.kind() == "identifier" =>
                        {
                            (
                                node_text(*name, context.source),
                                Some(node_text(*type_name, context.source)),
                            )
                        }
                        _ => continue,
                    }
                }
                _ => continue,
            };
            let mut bindings = context.bindings.borrow_mut();
            bindings.forget_foreign(&name);
            if let Some(type_name) = type_name {
                bindings.bind_any(name.clone(), type_name);
            }
            context.values.borrow_mut().insert(name);
        }
    }
    saved
}

fn julia_leave_function(saved: JuliaSavedScope, context: &JuliaParseContext<'_>) {
    *context.values.borrow_mut() = saved.0;
    context.bindings.borrow_mut().restore(saved.1);
}

/// Binds the variable an assignment sets (`c = connect()`): a value,
/// holding the result of the call when it is one.
fn julia_bind_assignment(node: tree_sitter::Node<'_>, context: &JuliaParseContext<'_>) {
    let Some(left) = julia_first_named_child(node).filter(|left| left.kind() == "identifier")
    else {
        return;
    };
    let var = node_text(left, context.source);
    let mut cursor = node.walk();
    let value = node.named_children(&mut cursor).last();
    let origin = value
        .filter(|value| value.kind() == "call_expression")
        .and_then(|value| {
            Some(CallOrigin {
                name: julia_call_name(value, context.source)?,
                line: value.start_position().row as i64 + 1,
                unwrap: false,
            })
        });
    let mut bindings = context.bindings.borrow_mut();
    match origin {
        Some(origin) => bindings.bind_returned(var.clone(), origin),
        None => bindings.forget_foreign(&var),
    }
    context.values.borrow_mut().insert(var);
}

/// Records what the value a function stored in a field is called on says
/// (`s.save(x)`): `receiver_type` for a parameter typed by a type of
/// another file (`s::Store`), or `receiver_unknown` for any other value
/// (`q.go()`, `c.close()` after `c = connect()`, with `receiver_from`), so
/// no same-named function of the file is taken for it. A module
/// (`Base.max`, `Mod.f`) is not a value.
fn julia_mark_receiver(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    extra: &mut Value,
) {
    let Some(field) =
        julia_first_named_child(node).filter(|first| first.kind() == "field_expression")
    else {
        return;
    };
    let Some(receiver) = field.child_by_field_name("value") else {
        return;
    };
    if receiver.kind() != "identifier" {
        return;
    }
    let name = node_text(receiver, context.source);
    let bindings = context.bindings.borrow();
    if bindings.is_bound(&name) {
        return;
    }
    if let Some(type_name) = bindings.foreign_type(&name) {
        extra["receiver_type"] = json!(type_name);
        return;
    }
    if !context.values.borrow().contains(&name) {
        return;
    }
    extra["receiver_unknown"] = json!(true);
    if let Some(origin) = bindings.returned_by(&name) {
        extra["receiver_from"] = origin.to_json();
    }
}

/// The declared return type of a function: `Store` for `function
/// make()::Store` or `make()::Store = ...` (also before a `where`).
fn julia_return_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut head = match node.kind() {
        "assignment" => julia_first_named_child(node)?,
        _ => julia_first_named_child(direct_child(node, &["signature"])?)?,
    };
    if head.kind() == "where_expression" {
        head = julia_first_named_child(head)?;
    }
    if head.kind() != "typed_expression" {
        return None;
    }
    let mut cursor = head.walk();
    let parts = head.named_children(&mut cursor).collect::<Vec<_>>();
    match parts.as_slice() {
        [call, return_type] if call.kind() == "call_expression" => {
            Some(node_text(*return_type, source))
        }
        _ => None,
    }
}

/// Names of the types declared anywhere in the file.
fn julia_collect_types(node: tree_sitter::Node<'_>, source: &[u8], types: &mut HashSet<String>) {
    if matches!(node.kind(), "struct_definition" | "abstract_definition")
        && let Some(name) = julia_type_name(node, source)
    {
        types.insert(name);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        julia_collect_types(child, source, types);
    }
}

/// Records what an import of a stdlib module brings into scope, and marks
/// its edge: `using LinearAlgebra` at `LinearAlgebra`, `using Statistics:
/// mean` at `Statistics` (`Statistics.mean` its symbol), both certain.
fn julia_record_import(
    statement: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    edge: &mut ParsedEdge,
) {
    let (module, name) = match edge.target.split_once('.') {
        Some((module, name)) => (module.to_string(), Some(name.to_string())),
        None => (edge.target.clone(), None),
    };
    if !is_julia_stdlib_module(&module) {
        return;
    }
    let mut imports = context.imports.borrow_mut();
    match name {
        Some(name) => {
            imports.names.insert(name, module.clone());
        }
        // `import LinearAlgebra` binds the module alone; `using` its exports.
        None if statement.kind() == "using_statement" => imports.modules.push(module.clone()),
        None => {}
    }
    mark_stdlib_edge(
        &mut edge.target,
        &mut edge.extra,
        &module,
        StdlibEvidence::Certain,
    );
}

/// Points the calls into Julia's standard library at its module: a path
/// through a stdlib module (`Base.max`, `LinearAlgebra.norm`) or a name
/// imported from one (`mean` after `using Statistics: mean`), certainly; a
/// name exported by a module the file is `using` (`norm` after `using
/// LinearAlgebra`), or by `Base` (`println`, `push!`, `@time`), likely. A
/// name or module this file defines is its own.
fn julia_mark_stdlib_calls(nodes: &[ParsedNode], edges: &mut [ParsedEdge], imports: &JuliaImports) {
    let defined = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test" | "Class"))
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::Calls {
            continue;
        }
        let path = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove("stdlib_path"))
            .and_then(|path| path.as_str().map(str::to_string));
        let (module, evidence, symbol) = match path {
            Some(path) => {
                let root = path.split('.').next().unwrap_or(&path).to_string();
                if !is_julia_stdlib_module(&root) || defined.contains(root.as_str()) {
                    continue;
                }
                (root, StdlibEvidence::Certain, path)
            }
            None if defined.contains(edge.target.as_str()) => continue,
            None => {
                let name = edge.target.as_str();
                if let Some(module) = imports.names.get(name) {
                    (
                        module.clone(),
                        StdlibEvidence::Certain,
                        format!("{module}.{name}"),
                    )
                } else if let Some(module) = imports
                    .modules
                    .iter()
                    .find(|module| julia_module_exports(module, name))
                {
                    (
                        module.clone(),
                        StdlibEvidence::Likely,
                        format!("{module}.{name}"),
                    )
                } else if is_julia_base_export(name) {
                    ("Base".to_string(), StdlibEvidence::Likely, name.to_string())
                } else {
                    continue;
                }
            }
        };
        edge.target = symbol;
        mark_stdlib_edge(&mut edge.target, &mut edge.extra, &module, evidence);
    }
}

fn julia_emit_enum(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(args) = direct_child(node, &["macro_argument_list"]) else {
        return;
    };
    let identifiers = direct_child_texts(args, context.source, &["identifier"]);
    let Some(type_name) = identifiers.first() else {
        return;
    };
    let qualified_type = qualify(&context.file_path, type_name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: type_name.clone(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"julia_kind": "enum"}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified_type.clone(),
        context.file_path.clone(),
        line_of(node),
    ));
    for variant in identifiers.iter().skip(1) {
        nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Function,
            name: variant.clone(),
            file_path: context.file_path.clone(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: "julia".to_string(),
            parent_name: Some(type_name.clone()),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"julia_kind": "enum_variant"}),
        });
        edges.push(ParsedEdge::new(
            crate::core::types::EdgeKind::Contains,
            qualified_type.clone(),
            qualify(&context.file_path, variant, Some(type_name)),
            context.file_path.clone(),
            line_of(node),
        ));
    }
}

fn julia_emit_testset(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let desc = direct_child(node, &["macro_argument_list"])
        .and_then(|args| first_descendant_text(args, context.source, &["content"]));
    let line = node.start_position().row as i64 + 1;
    let name = desc
        .map(|desc| format!("testset:{desc}@L{line}"))
        .unwrap_or_else(|| format!("testset@L{line}"));
    let qualified = qualify(&context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Test,
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: line,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_func
            .map(|func| qualify(&context.file_path, func, enclosing_class))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line,
    ));
    if let Some(args) = direct_child(node, &["macro_argument_list"]) {
        julia_walk_children(args, context, enclosing_class, Some(&name), nodes, edges);
    }
}

fn julia_emit_symbol_references(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let marker = if node.kind() == "export_statement" {
        "julia_export"
    } else {
        "julia_public"
    };
    let source = enclosing_class
        .map(|class| qualify(&context.file_path, class, None))
        .unwrap_or_else(|| context.file_path.to_string());
    for target in direct_child_texts(node, context.source, &["identifier"]) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::References,
            source: source.clone(),
            target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({marker: true}),
        });
    }
}

fn julia_macro_wraps_definition(node: tree_sitter::Node<'_>) -> bool {
    let Some(arguments) = direct_child(node, &["macro_argument_list"]) else {
        return false;
    };
    let mut cursor = arguments.walk();
    let Some(first) = arguments.named_children(&mut cursor).next() else {
        return false;
    };
    match first.kind() {
        "function_definition"
        | "macro_definition"
        | "struct_definition"
        | "abstract_definition" => true,
        "assignment" => julia_assignment_lhs_call(first).is_some(),
        _ => false,
    }
}

fn julia_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(type_head) = direct_child(node, &["type_head"]) else {
        return;
    };
    let Some(binary) = direct_child(type_head, &["binary_expression"]) else {
        return;
    };
    // `Pt{T} <: AbstractVector{T}`: either side may be parameterized.
    let mut cursor = binary.walk();
    let operands: Vec<_> = binary
        .named_children(&mut cursor)
        .filter(|child| child.kind() != "operator")
        .collect();
    let Some(supertype) = operands.get(1).and_then(|operand| match operand.kind() {
        "identifier" => Some(node_text(*operand, context.source)),
        "parametrized_type_expression" => {
            direct_child_text(*operand, context.source, &["identifier"])
        }
        _ => None,
    }) else {
        return;
    };
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Inherits,
        source: qualify(&context.file_path, name, enclosing_class),
        target: supertype,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({"relationship_role": "extends", "syntax_source": "struct_definition"}),
    });
}

fn julia_emit_owner_reference(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    parent_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(owner) = julia_qualified_function_owner(node, context.source) else {
        return;
    };
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::References,
        qualify(&context.file_path, name, parent_name),
        owner,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn julia_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = julia_call_signature(node, context.source).unwrap_or_else(|| call_name.into());
    let (relationship_role, bridge_kind) = match signature.as_str() {
        "run" | "readchomp" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "read" | "readlines" => ("reads_file", "file_io"),
        "write" => ("writes_file", "file_io"),
        "Libdl.dlopen" | "dlopen" | "ccall" | "@ccall" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (library, symbol) = match signature.as_str() {
        "ccall" | "@ccall" => julia_ccall_target(node, context.source, &signature)
            .map_or((None, None), |(library, symbol)| {
                (Some(library), Some(symbol))
            }),
        _ => (julia_first_string_arg(node, context.source), None),
    };
    let (target, confidence, confidence_tier) = match library {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    let mut extra = json!({
        "relationship_role": relationship_role,
        "bridge_kind": bridge_kind,
        "evidence_kind": "syntax",
        "evidence_source": signature,
        "source_language": "julia",
        "target_language": "unknown",
        "confidence": confidence,
        "confidence_tier": confidence_tier,
    });
    if let Some(symbol) = symbol {
        extra["symbol"] = json!(symbol);
    }
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line,
        extra,
    })
}

/// The library and C symbol of `ccall((:sym, "lib"), ...)` or
/// `@ccall lib.sym(...)::T`. The library is a string literal or the
/// constant naming it (`libfastsum`).
fn julia_ccall_target(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    signature: &str,
) -> Option<(String, String)> {
    let library_text = |library: tree_sitter::Node<'_>| match library.kind() {
        "string_literal" => julia_string_text(library, source),
        "identifier" => Some(node_text(library, source)),
        _ => None,
    };
    if signature == "ccall" {
        let args = direct_child(node, &["argument_list"])?;
        let mut cursor = args.walk();
        let tuple = args.named_children(&mut cursor).next()?;
        if tuple.kind() != "tuple_expression" {
            return None;
        }
        let mut inner = tuple.walk();
        let parts: Vec<_> = tuple.named_children(&mut inner).collect();
        let [symbol, library] = parts.as_slice() else {
            return None;
        };
        let symbol = match symbol.kind() {
            "quote_expression" => node_text(*symbol, source)
                .trim_start_matches(':')
                .to_string(),
            "string_literal" => julia_string_text(*symbol, source)?,
            _ => return None,
        };
        return Some((library_text(*library)?, symbol));
    }
    // `@ccall lib.sym(args...)::T`: the call inside the macro arguments.
    let args = direct_child(node, &["macro_argument_list"])?;
    let call = first_descendant(args, &["call_expression"])?;
    let mut cursor = call.walk();
    let callee = call
        .named_children(&mut cursor)
        .find(|child| child.kind() == "field_expression")?;
    let library = callee.child_by_field_name("value")?;
    let mut inner = callee.walk();
    let symbol = callee
        .named_children(&mut inner)
        .filter(|child| child.kind() == "identifier" && child.id() != library.id())
        .last()?;
    Some((library_text(library)?, node_text(symbol, source)))
}

fn julia_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            targets.push(node_text(child, source));
        } else if child.kind() == "selected_import" {
            let names = direct_child_texts(child, source, &["identifier"]);
            if let Some(module) = names.first() {
                targets.extend(names.iter().skip(1).map(|name| format!("{module}.{name}")));
            }
        }
    }
    targets
}

fn julia_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let type_head = direct_child(node, &["type_head"])?;
    first_descendant_text(type_head, source, &["identifier"])
}

fn julia_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let signature = direct_child(node, &["signature"])?;
    let call = first_descendant(signature, &["call_expression"])?;
    julia_call_name(call, source)
        .or_else(|| julia_functor_type(call, source).map(|_| "operator()".to_string()))
}

/// The struct made callable by `function (p::Pt)(y)`.
fn julia_functor_type(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(call)?;
    if first.kind() != "parenthesized_expression" {
        return None;
    }
    let typed = first_descendant(first, &["typed_expression"])?;
    let mut cursor = typed.walk();
    let ty = typed.named_children(&mut cursor).last()?;
    match ty.kind() {
        "identifier" => Some(node_text(ty, source)),
        "parametrized_type_expression" => direct_child_text(ty, source, &["identifier"]),
        _ => None,
    }
}

fn julia_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "field_expression" => last_descendant_text(first, source, &["identifier"]),
        _ => None,
    }
}

fn julia_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "field_expression" => Some(node_text(first, source).replace(' ', "")),
        _ => None,
    }
}

fn julia_macro_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let macro_identifier = direct_child(node, &["macro_identifier"])?;
    direct_child_text(macro_identifier, source, &["identifier"])
}

fn julia_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let args = direct_child(node, &["argument_list"])?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return julia_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn julia_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    first_descendant_text(node, source, &["content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn julia_qualified_function_owner(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let signature = if node.kind() == "assignment" {
        julia_assignment_lhs_call(node)
    } else {
        direct_child(node, &["signature"])
            .and_then(|signature| first_descendant(signature, &["call_expression"]))
    }?;
    let field = first_descendant(signature, &["field_expression"])?;
    let names = direct_child_texts(field, source, &["identifier"]);
    names.first().cloned()
}

fn julia_assignment_lhs_call<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let lhs = julia_first_named_child(node)?;
    if lhs.kind() == "call_expression" {
        Some(lhs)
    } else if lhs.kind() == "typed_expression" {
        first_descendant(lhs, &["call_expression"])
    } else {
        None
    }
}

fn julia_is_signature_call(node: tree_sitter::Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "signature")
}

fn julia_is_assignment_lhs_call(node: tree_sitter::Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "assignment" {
        return julia_first_named_child(parent) == Some(node);
    }
    if parent.kind() == "typed_expression" {
        return parent
            .parent()
            .is_some_and(|grandparent| grandparent.kind() == "assignment");
    }
    false
}

fn julia_function_parent(
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
) -> Option<String> {
    match (enclosing_class, enclosing_func) {
        (Some(class), Some(func)) => Some(format!("{class}.{func}")),
        (Some(class), None) => Some(class.to_string()),
        (None, Some(func)) => Some(func.to_string()),
        (None, None) => None,
    }
}

fn julia_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor).find(|child| child.is_named())
}
