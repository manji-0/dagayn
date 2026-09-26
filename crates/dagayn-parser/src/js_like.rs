use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::js_modules::{
    JavaScriptCaches, JavaScriptParseContext, collect_javascript_defined_names,
    collect_javascript_import_map, collect_javascript_type_names, decode_javascript_string_literal,
    javascript_child_text, javascript_function_name, javascript_import_targets,
    javascript_named_child, resolve_javascript_call_target, resolve_javascript_module,
    resolve_javascript_namespace_member,
};
use super::member_calls::MemberCallBindings;
use super::parsers::*;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    ends_with_ascii_ignore_case, is_test_file, line_count, node_text, starts_with_ascii_ignore_case,
};
use super::{add_tested_by_edges, qualify, resolve_rust_call_targets};

pub(super) fn parse_javascript_like(
    file_path: &str,
    source: &[u8],
    language: &'static str,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = match language {
        "javascript" => new_javascript_parser(),
        "typescript" => new_typescript_parser(),
        "tsx" => new_tsx_parser(),
        _ => None,
    };
    parse_javascript_like_with_parser(
        file_path,
        source,
        language,
        parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

pub(super) fn parse_javascript_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &'static str,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_javascript_like_interned(
        &FilePath::new(file_path),
        source,
        language,
        parser,
        repo_root,
        caches,
    )
}

pub(super) fn parse_javascript_like_interned(
    file_path: &FilePath,
    source: &[u8],
    language: &'static str,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let test_file = is_javascript_test_file(file_path);
    let declaration_file = is_javascript_declaration_file(file_path);
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
        is_test: test_file,
        extra: if declaration_file {
            json!({"declaration_file": true})
        } else {
            json!({})
        },
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let root = tree.root_node();
        let mut defined_names = HashSet::new();
        collect_javascript_defined_names(root, source, &mut defined_names);
        let mut type_names = HashSet::new();
        collect_javascript_type_names(root, source, &mut type_names);
        let mut import_map = HashMap::new();
        collect_javascript_import_map(root, source, &mut import_map);
        let scopes = collect_javascript_member_paths(root, source);
        let context = JavaScriptParseContext {
            source,
            file_path: file_path.clone(),
            language,
            test_file,
            defined_names: &defined_names,
            import_map: &import_map,
            member_paths: &scopes.members,
            namespace_paths: &scopes.namespaces,
            declaration_file,
            ambient_depth: Cell::new(0),
            repo_root,
            caches,
            bindings: RefCell::new(MemberCallBindings::with_types(type_names)),
        };
        javascript_walk_children(root, &context, None, None, &mut nodes, &mut edges);
        let mut edges = resolve_rust_call_targets(&nodes, edges, file_path);
        if test_file {
            add_tested_by_edges(&nodes, &mut edges);
        }
        return (nodes, edges);
    }

    (nodes, edges)
}

fn javascript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        javascript_walk_node(child, context, owner_path, enclosing_func, nodes, edges);
    }
}

/// Extracts one syntax node and, unless an arm consumes it, its subtree.
fn javascript_walk_node(
    child: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    match child.kind() {
        "type_alias_declaration" => {
            if let Some(name) = child.child_by_field_name("name") {
                let name = node_text(name, context.source);
                javascript_emit_type_alias(child, &name, context, owner_path, nodes, edges);
                return;
            }
        }
        "class_declaration"
        | "abstract_class_declaration"
        | "class"
        | "interface_declaration"
        | "enum_declaration" => {
            if let Some(name) =
                javascript_named_child(child, context.source, &["identifier", "type_identifier"])
            {
                javascript_emit_class_node(
                    child,
                    &name,
                    json!({}),
                    context,
                    owner_path,
                    nodes,
                    edges,
                );
                return;
            }
            if child.kind() == "class" {
                javascript_walk_unbound_class(
                    child,
                    context,
                    owner_path,
                    enclosing_func,
                    nodes,
                    edges,
                );
                return;
            }
        }
        "internal_module" | "module" => {
            if let Some((segments, role)) = javascript_namespace_segments(child, context.source) {
                javascript_emit_namespace(
                    child, &segments, role, context, owner_path, nodes, edges,
                );
                return;
            }
        }
        "ambient_declaration" => {
            context.ambient_depth.set(context.ambient_depth.get() + 1);
            let mut cursor = child.walk();
            let global_body = child
                .children(&mut cursor)
                .any(|part| part.kind() == "global")
                .then(|| javascript_named_child_node(child, "statement_block"))
                .flatten();
            match global_body {
                // `declare global { ... }`
                Some(body) => javascript_emit_namespace_body(
                    child,
                    body,
                    &["global".to_string()],
                    "ambient_module",
                    context,
                    owner_path,
                    nodes,
                    edges,
                ),
                None => javascript_walk_children(
                    child,
                    context,
                    owner_path,
                    enclosing_func,
                    nodes,
                    edges,
                ),
            }
            context.ambient_depth.set(context.ambient_depth.get() - 1);
            return;
        }
        "method_definition"
            if child
                .parent()
                .is_some_and(|parent| parent.kind() == "object") =>
        {
            // A method of an object literal that is not a module-scope
            // container (function-local, argument, deeper nesting): nothing
            // can name it, so its calls stay with the enclosing node.
            if let Some(body) = child.child_by_field_name("body") {
                javascript_walk_children(body, context, owner_path, enclosing_func, nodes, edges);
            }
            return;
        }
        "function_declaration"
        | "generator_function_declaration"
        | "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "function_signature"
        | "arrow_function"
            if javascript_emit_function_node(child, context, owner_path, nodes, edges) =>
        {
            if let Some(name) = javascript_function_name(child, context.source) {
                let snapshot = context.bindings.borrow().snapshot();
                javascript_bind_this(context, owner_path);
                javascript_walk_children(child, context, owner_path, Some(&name), nodes, edges);
                context.bindings.borrow_mut().restore(snapshot);
            }
            return;
        }
        "lexical_declaration" | "variable_declaration"
            if javascript_emit_variable_functions(child, context, owner_path, nodes, edges) =>
        {
            return;
        }
        "public_field_definition"
            if javascript_emit_field_function(child, context, owner_path, nodes, edges) =>
        {
            return;
        }
        "import_statement" | "export_statement" => {
            for target in javascript_import_targets(child, context.source) {
                let resolved = resolve_javascript_module(
                    &target,
                    &context.file_path,
                    context.repo_root,
                    context.caches,
                )
                .unwrap_or(target);
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: context.file_path.to_string(),
                    target: resolved,
                    file_path: context.file_path.clone(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
            if child.kind() == "import_statement" {
                return;
            }
            if let Some(global) = javascript_umd_global_name(child, context.source) {
                if let Some(map) = nodes
                    .first_mut()
                    .and_then(|file| file.extra.as_object_mut())
                {
                    map.insert("umd_global".to_string(), json!(global));
                }
                return;
            }
            if javascript_emit_default_export(
                child,
                context,
                owner_path,
                enclosing_func,
                nodes,
                edges,
            ) {
                return;
            }
        }
        "call_expression" | "new_expression"
            if javascript_emit_call(child, context, owner_path, enclosing_func, nodes, edges) =>
        {
            return;
        }
        "class_heritage" | "extends_type_clause" => return,
        "jsx_opening_element" | "jsx_self_closing_element" => {
            javascript_emit_jsx_component_call(child, context, owner_path, enclosing_func, edges);
        }
        "pair"
        | "assignment_expression"
        | "array"
        | "arguments"
        | "shorthand_property_identifier" => {
            javascript_emit_value_references(child, context, owner_path, enclosing_func, edges);
        }
        _ => {}
    }
    javascript_walk_children(child, context, owner_path, enclosing_func, nodes, edges);
    javascript_bind_declarator(child, context);
    javascript_bind_assignment(child, context);
}

/// A function-valued or nested-container member of an object literal.
enum JavaScriptObjectMember<'tree> {
    /// `k() {}`, `k: () => {}`, `k: function () {}`; `value` is the
    /// function literal (the method itself for `method_definition`).
    Function {
        name: String,
        member: tree_sitter::Node<'tree>,
        value: tree_sitter::Node<'tree>,
    },
    /// `k: { ... }` whose object has function-valued members of its own.
    Container {
        name: String,
        member: tree_sitter::Node<'tree>,
        object: tree_sitter::Node<'tree>,
    },
}

/// Strips `( ... )`, `as T` / `as const`, and `satisfies T` around an object
/// literal.
fn javascript_unwrap_object(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut current = node;
    loop {
        match current.kind() {
            "object" => return Some(current),
            "parenthesized_expression" | "as_expression" | "satisfies_expression" => {
                current = current.named_child(0)?;
            }
            _ => return None,
        }
    }
}

fn javascript_object_key_name(key: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match key.kind() {
        "property_identifier" | "identifier" | "private_property_identifier" => {
            Some(node_text(key, source))
        }
        "string" => {
            Some(decode_javascript_string_literal(key, source)).filter(|name| !name.is_empty())
        }
        _ => None,
    }
}

/// Deepest object-literal nesting modeled as containers
/// (`api.a.b.c` is depth 3); deeper objects are walked without nodes.
const JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH: usize = 6;

/// Function-valued members of `object`, plus (when `allow_nested`) nested
/// objects that have function-valued members at some depth below them, up to
/// [`JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH`].
fn javascript_object_members<'tree>(
    object: tree_sitter::Node<'tree>,
    source: &[u8],
    allow_nested: bool,
) -> Vec<JavaScriptObjectMember<'tree>> {
    javascript_object_members_at(
        object,
        source,
        if allow_nested {
            JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH
        } else {
            1
        },
    )
}

/// Members of `object` when containers may nest `depth` more levels (1: this
/// object only).
fn javascript_object_members_at<'tree>(
    object: tree_sitter::Node<'tree>,
    source: &[u8],
    depth: usize,
) -> Vec<JavaScriptObjectMember<'tree>> {
    let allow_nested = depth > 1;
    let mut members = Vec::new();
    let mut cursor = object.walk();
    for member in object.named_children(&mut cursor) {
        match member.kind() {
            "method_definition" => {
                if let Some(name) = member
                    .child_by_field_name("name")
                    .and_then(|key| javascript_object_key_name(key, source))
                {
                    members.push(JavaScriptObjectMember::Function {
                        name,
                        member,
                        value: member,
                    });
                }
            }
            "pair" => {
                let (Some(name), Some(value)) = (
                    member
                        .child_by_field_name("key")
                        .and_then(|key| javascript_object_key_name(key, source)),
                    member.child_by_field_name("value"),
                ) else {
                    continue;
                };
                if is_javascript_function_value(value.kind()) {
                    members.push(JavaScriptObjectMember::Function {
                        name,
                        member,
                        value,
                    });
                } else if allow_nested
                    && let Some(nested) = javascript_unwrap_object(value)
                    && !javascript_object_members_at(nested, source, depth - 1).is_empty()
                {
                    members.push(JavaScriptObjectMember::Container {
                        name,
                        member,
                        object: nested,
                    });
                }
            }
            _ => {}
        }
    }
    members
}

/// Declaration-scope statements only: a child of the `program` or of a
/// namespace / ambient-module body, or the declaration of an `export`
/// statement there.
fn javascript_is_module_scope(node: tree_sitter::Node<'_>) -> bool {
    match node.parent() {
        Some(parent) if parent.kind() == "export_statement" => {
            parent.parent().is_some_and(javascript_is_declaration_scope)
        }
        Some(parent) => javascript_is_declaration_scope(parent),
        None => false,
    }
}

/// `program`, or the body of `namespace N {}` / `module "x" {}` /
/// `declare global {}`.
fn javascript_is_declaration_scope(node: tree_sitter::Node<'_>) -> bool {
    match node.kind() {
        "program" => true,
        "statement_block" => node.parent().is_some_and(|parent| {
            matches!(parent.kind(), "internal_module" | "module")
                || (parent.kind() == "ambient_declaration"
                    && parent
                        .children(&mut parent.walk())
                        .any(|child| child.kind() == "global"))
        }),
        _ => false,
    }
}

/// Module-scope object containers: `(binding name, object literal)` for
/// `const X = { ... }` and `("default", object)` for `export default { ... }`.
fn javascript_module_object_containers<'tree>(
    statement: tree_sitter::Node<'tree>,
    source: &[u8],
) -> Vec<(String, tree_sitter::Node<'tree>)> {
    let mut containers = Vec::new();
    match statement.kind() {
        "lexical_declaration" | "variable_declaration" if javascript_is_module_scope(statement) => {
            let mut cursor = statement.walk();
            for declarator in statement.named_children(&mut cursor) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                let (Some(name), Some(object)) = (
                    declarator
                        .child_by_field_name("name")
                        .filter(|name| name.kind() == "identifier"),
                    declarator
                        .child_by_field_name("value")
                        .and_then(javascript_unwrap_object),
                ) else {
                    continue;
                };
                if !javascript_object_members(object, source, true).is_empty() {
                    containers.push((node_text(name, source), object));
                }
            }
        }
        "export_statement" if javascript_is_module_scope(statement) => {
            let mut cursor = statement.walk();
            let is_default = statement
                .children(&mut cursor)
                .any(|child| child.kind() == "default");
            if let Some(object) = statement
                .child_by_field_name("value")
                .and_then(javascript_unwrap_object)
                .filter(|_| is_default)
                .filter(|object| !javascript_object_members(*object, source, true).is_empty())
            {
                containers.push(("default".to_string(), object));
            }
        }
        _ => {}
    }
    containers
}

/// Member paths found before the walk.
pub(super) struct JavaScriptMemberPaths {
    /// Object-container members (`api.get`, `api.nested.deep`) and namespace
    /// members (`Outer.helper`, `Outer.Inner`, `A.B.C.abc`), used to bind
    /// `api.nested.deep()` / `Outer.Deep.deepFn()` before any node exists.
    pub(super) members: HashSet<String>,
    /// Namespace and ambient-module owner paths (`Outer`, `A.B`, `global`).
    pub(super) namespaces: HashSet<String>,
}

pub(super) fn collect_javascript_member_paths(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> JavaScriptMemberPaths {
    let mut paths = JavaScriptMemberPaths {
        members: HashSet::new(),
        namespaces: HashSet::new(),
    };
    collect_javascript_scope_member_paths(root, source, None, &mut paths);
    paths
}

/// Walks the statements of one declaration scope (`program` or a namespace
/// body) owned by `owner`.
fn collect_javascript_scope_member_paths(
    scope: tree_sitter::Node<'_>,
    source: &[u8],
    owner: Option<&str>,
    paths: &mut JavaScriptMemberPaths,
) {
    let mut cursor = scope.walk();
    for statement in scope.named_children(&mut cursor) {
        let statement = match statement.kind() {
            "export_statement" => statement
                .child_by_field_name("declaration")
                .unwrap_or(statement),
            _ => statement,
        };
        collect_javascript_statement_member_paths(statement, source, owner, paths);
    }
}

fn collect_javascript_statement_member_paths(
    statement: tree_sitter::Node<'_>,
    source: &[u8],
    owner: Option<&str>,
    paths: &mut JavaScriptMemberPaths,
) {
    let member = |name: &str| match owner {
        Some(owner) => format!("{owner}.{name}"),
        None => name.to_string(),
    };
    match statement.kind() {
        "expression_statement" | "ambient_declaration" => {
            let mut cursor = statement.walk();
            let is_global = statement
                .children(&mut cursor)
                .any(|child| child.kind() == "global");
            if is_global {
                if let Some(body) = javascript_named_child_node(statement, "statement_block") {
                    let path = member("global");
                    paths.namespaces.insert(path.clone());
                    paths.members.insert(path.clone());
                    collect_javascript_scope_member_paths(body, source, Some(&path), paths);
                }
                return;
            }
            let mut cursor = statement.walk();
            for inner in statement.named_children(&mut cursor) {
                collect_javascript_statement_member_paths(inner, source, owner, paths);
            }
        }
        "internal_module" | "module" => {
            let Some((segments, _)) = javascript_namespace_segments(statement, source) else {
                return;
            };
            let mut path = owner.map(str::to_string);
            for segment in &segments {
                let next = javascript_member_owner(path.as_deref(), segment);
                paths.namespaces.insert(next.clone());
                paths.members.insert(next.clone());
                path = Some(next);
            }
            if let (Some(body), Some(path)) = (statement.child_by_field_name("body"), path) {
                collect_javascript_scope_member_paths(body, source, Some(&path), paths);
            }
        }
        _ => {
            for (name, object) in javascript_module_object_containers(statement, source) {
                let path = member(&name);
                collect_javascript_object_member_paths(
                    object,
                    source,
                    &path,
                    JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH,
                    &mut paths.members,
                );
                if owner.is_some() {
                    paths.members.insert(path);
                }
            }
            if owner.is_some() {
                for name in javascript_declared_names(statement, source) {
                    paths.members.insert(member(&name));
                }
            }
        }
    }
}

/// Names a declaration statement introduces (functions, classes,
/// interfaces, enums, type aliases, bound variables).
fn javascript_declared_names(statement: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    match statement.kind() {
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "class_declaration"
        | "abstract_class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "type_alias_declaration" => statement
            .child_by_field_name("name")
            .map(|name| vec![node_text(name, source)])
            .unwrap_or_default(),
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = statement.walk();
            statement
                .named_children(&mut cursor)
                .filter(|declarator| declarator.kind() == "variable_declarator")
                .filter_map(|declarator| declarator.child_by_field_name("name"))
                .filter(|name| name.kind() == "identifier")
                .map(|name| node_text(name, source))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Segments and role of `namespace A.B.C` / `module Legacy` (`namespace`) or
/// `module "x"` (`ambient_module`).
fn javascript_namespace_segments(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Vec<String>, &'static str)> {
    let name = node.child_by_field_name("name")?;
    match name.kind() {
        "identifier" => Some((vec![node_text(name, source)], "namespace")),
        "nested_identifier" => {
            let mut segments = Vec::new();
            javascript_nested_identifier_segments(name, source, &mut segments);
            (!segments.is_empty()).then_some((segments, "namespace"))
        }
        "string" => {
            let module = decode_javascript_string_literal(name, source);
            (!module.is_empty()).then(|| (vec![module], "ambient_module"))
        }
        _ => None,
    }
}

fn javascript_nested_identifier_segments(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    segments: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "nested_identifier" | "member_expression" => {
                javascript_nested_identifier_segments(child, source, segments)
            }
            "identifier" | "property_identifier" => segments.push(node_text(child, source)),
            _ => {}
        }
    }
}

fn javascript_named_child_node<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
) -> Option<tree_sitter::Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Emits `namespace A.B.C { ... }` / `module "x" { ... }` as nested `Class`
/// nodes and walks the body with the innermost path as the owner.
fn javascript_emit_namespace(
    node: tree_sitter::Node<'_>,
    segments: &[String],
    role: &'static str,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    // `module "x" {}` outside `declare` is still an ambient module.
    let ambient = role == "ambient_module";
    if ambient {
        context.ambient_depth.set(context.ambient_depth.get() + 1);
    }
    javascript_emit_namespace_body(
        node, body, segments, role, context, owner_path, nodes, edges,
    );
    if ambient {
        context.ambient_depth.set(context.ambient_depth.get() - 1);
    }
}

#[allow(clippy::too_many_arguments)]
fn javascript_emit_namespace_body(
    node: tree_sitter::Node<'_>,
    body: tree_sitter::Node<'_>,
    segments: &[String],
    role: &'static str,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let line_start = node.start_position().row as i64 + 1;
    let line_end = node.end_position().row as i64 + 1;
    let mut owner = owner_path.map(str::to_string);
    for segment in segments {
        // Declaration merging: `namespace Outer {}` twice is one node.
        let exists = nodes.iter().any(|existing| {
            existing.kind == crate::core::types::NodeKind::Class
                && existing.name == *segment
                && existing.parent_name == owner
        });
        if !exists {
            javascript_push_node(
                context,
                nodes,
                ParsedNode {
                    kind: crate::core::types::NodeKind::Class,
                    name: segment.clone(),
                    file_path: context.file_path.clone(),
                    line_start,
                    line_end,
                    language: context.language.to_string(),
                    parent_name: owner.clone(),
                    params: None,
                    return_type: None,
                    modifiers: None,
                    is_test: false,
                    extra: json!({"type_role": role}),
                },
            );
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::Contains,
                source: javascript_container_qn(context, owner.as_deref()),
                target: qualify(&context.file_path, segment, owner.as_deref()),
                file_path: context.file_path.clone(),
                line: line_start,
                extra: json!({}),
            });
        }
        owner = Some(javascript_member_owner(owner.as_deref(), segment));
    }
    javascript_walk_children(body, context, owner.as_deref(), None, nodes, edges);
}

/// `export as namespace MyLib;` -> `MyLib`.
fn javascript_umd_global_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    let is_umd = children.iter().any(|child| child.kind() == "as")
        && children.iter().any(|child| child.kind() == "namespace");
    if !is_umd {
        return None;
    }
    children
        .iter()
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(*child, source))
}

/// Pushes a node, marking it `ambient` inside `declare` / ambient-module
/// bodies and in declaration files.
fn javascript_push_node(
    context: &JavaScriptParseContext<'_>,
    nodes: &mut Vec<ParsedNode>,
    mut node: ParsedNode,
) {
    if (context.declaration_file || context.ambient_depth.get() > 0)
        && let Some(map) = node.extra.as_object_mut()
    {
        map.insert("ambient".to_string(), json!(true));
    }
    nodes.push(node);
}

/// Binds `this` / `self` to the owner when members of that owner see one
/// (classes, object containers), not for namespaces.
fn javascript_bind_this(context: &JavaScriptParseContext<'_>, owner_path: Option<&str>) {
    if let Some(owner) = owner_path
        && !context.namespace_paths.contains(owner)
    {
        context.bindings.borrow_mut().bind_implicit_receivers(owner);
    }
}

fn collect_javascript_object_member_paths(
    object: tree_sitter::Node<'_>,
    source: &[u8],
    owner: &str,
    depth: usize,
    paths: &mut HashSet<String>,
) {
    for member in javascript_object_members_at(object, source, depth) {
        match member {
            JavaScriptObjectMember::Function { name, .. } => {
                paths.insert(format!("{owner}.{name}"));
            }
            JavaScriptObjectMember::Container { name, object, .. } => {
                let path = format!("{owner}.{name}");
                collect_javascript_object_member_paths(object, source, &path, depth - 1, paths);
                paths.insert(path);
            }
        }
    }
}

/// Emits `Class <name>` (`type_role: "object"`) for an object literal and
/// its function-valued members under the container's owner path. Other
/// members (data, shorthand references, deeper objects) are walked for
/// edges with the container's own caller, without creating nodes.
#[allow(clippy::too_many_arguments)]
fn javascript_emit_object_container(
    declaration: tree_sitter::Node<'_>,
    object: tree_sitter::Node<'_>,
    name: &str,
    additions: Value,
    depth: usize,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut extra = json!({"type_role": "object"});
    if let (Some(map), Value::Object(additions)) = (extra.as_object_mut(), additions) {
        map.extend(additions);
    }
    javascript_push_node(
        context,
        nodes,
        ParsedNode {
            kind: crate::core::types::NodeKind::Class,
            name: name.to_string(),
            file_path: context.file_path.clone(),
            line_start: declaration.start_position().row as i64 + 1,
            line_end: declaration.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        },
    );
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: javascript_container_qn(context, owner_path),
        target: qualify(&context.file_path, name, owner_path),
        file_path: context.file_path.clone(),
        line: declaration.start_position().row as i64 + 1,
        extra: json!({}),
    });
    let owner = javascript_member_owner(owner_path, name);
    let mut handled = HashSet::new();
    for member in javascript_object_members_at(object, context.source, depth) {
        match member {
            JavaScriptObjectMember::Function {
                name,
                member,
                value,
            } => {
                handled.insert(member.id());
                javascript_emit_bound_function(
                    member,
                    value,
                    &name,
                    json!({}),
                    context,
                    Some(&owner),
                    nodes,
                    edges,
                );
            }
            JavaScriptObjectMember::Container {
                name,
                member,
                object,
            } => {
                handled.insert(member.id());
                javascript_emit_object_container(
                    member,
                    object,
                    &name,
                    json!({}),
                    depth - 1,
                    context,
                    Some(&owner),
                    nodes,
                    edges,
                );
            }
        }
    }
    let mut cursor = object.walk();
    for member in object.named_children(&mut cursor) {
        if !handled.contains(&member.id()) {
            javascript_walk_node(member, context, None, None, nodes, edges);
        }
    }
}

/// Whether `node` is `declarator` or one of the wrappers between the
/// declarator and its object literal (`as`, `satisfies`, parentheses).
fn javascript_declarator_owns(
    declarator: tree_sitter::Node<'_>,
    mut node: tree_sitter::Node<'_>,
) -> bool {
    loop {
        if node.id() == declarator.id() {
            return true;
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => return false,
        }
    }
}

/// Emits a `Class` node for a class-like declaration or class expression,
/// its CONTAINS and heritage edges, and walks its body with the class as the
/// owner. `additions` are merged into the role metadata.
fn javascript_emit_class_node(
    node: tree_sitter::Node<'_>,
    name: &str,
    additions: Value,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, owner_path);
    let mut extra = javascript_class_extra(node, context.source, name);
    if let (Some(map), Value::Object(additions)) = (extra.as_object_mut(), additions) {
        map.extend(additions);
    }
    javascript_push_node(
        context,
        nodes,
        ParsedNode {
            kind: crate::core::types::NodeKind::Class,
            name: name.to_string(),
            file_path: context.file_path.clone(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        },
    );
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: javascript_container_qn(context, owner_path),
        target: qualified.clone(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    emit_javascript_inheritance_edges(node, context, &qualified, edges);
    let member_owner = javascript_member_owner(owner_path, name);
    javascript_walk_children(node, context, Some(&member_owner), None, nodes, edges);
}

/// Owner path of the members of container `name` declared under
/// `owner_path` (`Outer` + `Inner` -> `Outer.Inner`).
fn javascript_member_owner(owner_path: Option<&str>, name: &str) -> String {
    owner_path
        .map(|parent| format!("{parent}.{name}"))
        .unwrap_or_else(|| name.to_string())
}

/// Emits `Type <name>` (`type_role: "alias"`, `alias_form`) for
/// `type T = ...`. Object-shaped aliases are data containers. The right-hand
/// side has no calls and its members are not nodes: an alias is neither
/// nominal nor a container.
fn javascript_emit_type_alias(
    node: tree_sitter::Node<'_>,
    name: &str,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let form = node
        .child_by_field_name("value")
        .map(javascript_alias_form)
        .unwrap_or("other");
    let mut extra = json!({"type_role": "alias", "alias_form": form});
    if form == "object"
        && let Some(map) = extra.as_object_mut()
    {
        map.insert("container_role".to_string(), json!("data_container"));
        map.insert("value_semantics".to_string(), json!(true));
    }
    let line = node.start_position().row as i64 + 1;
    javascript_push_node(
        context,
        nodes,
        ParsedNode {
            kind: crate::core::types::NodeKind::Type,
            name: name.to_string(),
            file_path: context.file_path.clone(),
            line_start: line,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        },
    );
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: javascript_container_qn(context, owner_path),
        target: qualify(&context.file_path, name, owner_path),
        file_path: context.file_path.clone(),
        line,
        extra: json!({}),
    });
}

/// Shape of a type alias right-hand side.
fn javascript_alias_form(value: tree_sitter::Node<'_>) -> &'static str {
    match value.kind() {
        "object_type" => {
            let mut cursor = value.walk();
            let mapped = value.named_children(&mut cursor).any(|member| {
                member.kind() == "index_signature"
                    && member
                        .named_children(&mut member.walk())
                        .any(|part| part.kind() == "mapped_type_clause")
            });
            if mapped { "mapped" } else { "object" }
        }
        "union_type" => "union",
        "intersection_type" => "intersection",
        "function_type" | "constructor_type" => "function",
        "conditional_type" => "conditional",
        "tuple_type" | "array_type" | "readonly_type" => "tuple",
        "type_identifier" | "nested_type_identifier" | "generic_type" => "reference",
        "predefined_type" => "primitive",
        "literal_type" | "template_literal_type" => "literal",
        "index_type_query" | "lookup_type" | "type_query" => "operator",
        "parenthesized_type" => value
            .named_child(0)
            .map(javascript_alias_form)
            .unwrap_or("other"),
        _ => "other",
    }
}

fn javascript_container_qn(
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
) -> String {
    owner_path
        .map(|class_name| qualify(&context.file_path, class_name, None))
        .unwrap_or_else(|| context.file_path.to_string())
}

/// Walks an anonymous class expression that has no binding
/// (`return class extends Base { ... }`, `define(class { ... })`).
///
/// Nothing outside can name its members, so they are not nodes; calls in
/// member bodies and initializers stay attributed to the enclosing node
/// instead of being flattened into top-level functions.
fn javascript_walk_unbound_class(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method_definition" => {
                if let Some(member_body) = member.child_by_field_name("body") {
                    javascript_walk_children(
                        member_body,
                        context,
                        owner_path,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            "public_field_definition" | "field_definition" => {
                match member.child_by_field_name("value") {
                    Some(value) if is_javascript_function_value(value.kind()) => {
                        javascript_walk_children(
                            value,
                            context,
                            owner_path,
                            enclosing_func,
                            nodes,
                            edges,
                        );
                    }
                    Some(_) => javascript_walk_children(
                        member,
                        context,
                        owner_path,
                        enclosing_func,
                        nodes,
                        edges,
                    ),
                    None => {}
                }
            }
            "class_static_block" => {
                javascript_walk_children(member, context, owner_path, enclosing_func, nodes, edges)
            }
            _ => {}
        }
    }
}

/// Handles `export default ...`.
///
/// Anonymous classes and functions become nodes named `default`
/// (`export_default`, `anonymous`); a named default declaration is walked
/// normally and marked `export_default`. Other values (identifiers,
/// objects, calls) fall through to the generic walk.
fn javascript_emit_default_export(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut cursor = node.walk();
    if !node
        .children(&mut cursor)
        .any(|child| child.kind() == "default")
    {
        return false;
    }
    if let Some(declaration) = node.child_by_field_name("declaration") {
        let first_new = nodes.len();
        javascript_walk_children(node, context, owner_path, enclosing_func, nodes, edges);
        let line = declaration.start_position().row as i64 + 1;
        if let Some(declared) = nodes.get_mut(first_new)
            && declared.line_start == line
            && let Some(map) = declared.extra.as_object_mut()
        {
            map.insert("export_default".to_string(), json!(true));
        }
        return true;
    }
    let Some(value) = node.child_by_field_name("value") else {
        return false;
    };
    if let Some((name, object)) = javascript_module_object_containers(node, context.source)
        .into_iter()
        .next()
    {
        javascript_emit_object_container(
            node,
            object,
            &name,
            json!({"export_default": true, "anonymous": true}),
            JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH,
            context,
            owner_path,
            nodes,
            edges,
        );
        return true;
    }
    match value.kind() {
        "class" => {
            let (name, additions) = match value.child_by_field_name("name") {
                Some(name) => (
                    node_text(name, context.source),
                    json!({"export_default": true}),
                ),
                None => (
                    "default".to_string(),
                    json!({"export_default": true, "anonymous": true}),
                ),
            };
            javascript_emit_class_node(value, &name, additions, context, owner_path, nodes, edges);
            true
        }
        kind if is_javascript_function_value(kind) => {
            let (name, extra) = match value.child_by_field_name("name") {
                Some(name) if kind != "arrow_function" => (
                    node_text(name, context.source),
                    json!({"export_default": true}),
                ),
                _ => (
                    "default".to_string(),
                    json!({"export_default": true, "anonymous": true}),
                ),
            };
            javascript_emit_bound_function(
                node, value, &name, extra, context, owner_path, nodes, edges,
            );
            true
        }
        _ => false,
    }
}

/// Emits a `Function` node for a function literal bound to `name`
/// (`const f = () => {}`, `export default function () {}`) and walks its
/// body with the new node as the caller.
#[allow(clippy::too_many_arguments)]
fn javascript_emit_bound_function(
    declaration: tree_sitter::Node<'_>,
    function_node: tree_sitter::Node<'_>,
    name: &str,
    extra: Value,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_javascript_test_function(name, &context.file_path);
    let qualified = qualify(&context.file_path, name, owner_path);
    javascript_push_node(
        context,
        nodes,
        ParsedNode {
            kind: if is_test {
                crate::core::types::NodeKind::Test
            } else {
                crate::core::types::NodeKind::Function
            },
            name: name.to_string(),
            file_path: context.file_path.clone(),
            line_start: declaration.start_position().row as i64 + 1,
            line_end: declaration.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: javascript_child_text(function_node, context.source, "formal_parameters"),
            return_type: javascript_child_text(function_node, context.source, "type_annotation"),
            modifiers: None,
            is_test,
            extra,
        },
    );
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: javascript_container_qn(context, owner_path),
        target: qualified,
        file_path: context.file_path.clone(),
        line: declaration.start_position().row as i64 + 1,
        extra: json!({}),
    });
    let snapshot = context.bindings.borrow().snapshot();
    javascript_bind_this(context, owner_path);
    javascript_walk_children(function_node, context, owner_path, Some(name), nodes, edges);
    context.bindings.borrow_mut().restore(snapshot);
}

fn javascript_emit_function_node(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = javascript_function_name(node, context.source) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, &context.file_path);
    let qualified = qualify(&context.file_path, &name, owner_path);
    javascript_push_node(
        context,
        nodes,
        ParsedNode {
            kind: if is_test {
                crate::core::types::NodeKind::Test
            } else {
                crate::core::types::NodeKind::Function
            },
            name: name.clone(),
            file_path: context.file_path.clone(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: if node.kind() == "arrow_function" {
                None
            } else {
                javascript_child_text(node, context.source, "formal_parameters")
            },
            return_type: javascript_child_text(node, context.source, "type_annotation"),
            modifiers: None,
            is_test,
            extra: javascript_signature_extra(node),
        },
    );
    let container = owner_path
        .map(|name| qualify(&context.file_path, name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

/// `is_abstract` for contract members (interface method signatures,
/// `abstract` members); `declaration_only` for bodiless declarations that
/// are not contracts (`declare function`, overload signatures, methods of a
/// `declare class`).
fn javascript_signature_extra(node: tree_sitter::Node<'_>) -> Value {
    match node.kind() {
        "abstract_method_signature" => json!({"is_abstract": true}),
        "function_signature" => json!({"declaration_only": true}),
        "method_signature" => {
            let in_class = node
                .parent()
                .is_some_and(|parent| parent.kind() == "class_body");
            if in_class {
                json!({"declaration_only": true})
            } else {
                json!({"is_abstract": true})
            }
        }
        _ => json!({}),
    }
}

fn javascript_emit_variable_functions(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut handled = false;
    let containers = javascript_module_object_containers(node, context.source);
    let mut cursor = node.walk();
    for declarator in node.children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        if let Some((name, object)) = containers.iter().find(|(_, object)| {
            object
                .parent()
                .is_some_and(|parent| javascript_declarator_owns(declarator, parent))
        }) {
            javascript_emit_object_container(
                node,
                *object,
                name,
                json!({}),
                JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH,
                context,
                owner_path,
                nodes,
                edges,
            );
            handled = true;
            continue;
        }
        let mut name = None;
        let mut function_node = None;
        let mut class_node = None;
        let mut declarator_cursor = declarator.walk();
        for child in declarator.children(&mut declarator_cursor) {
            if child.kind() == "identifier" && name.is_none() {
                name = Some(node_text(child, context.source));
            } else if is_javascript_function_value(child.kind()) {
                function_node = Some(child);
            } else if child.kind() == "class" {
                class_node = Some(child);
            }
        }
        if let (Some(name), Some(class_node)) = (name.as_deref(), class_node) {
            // `const X = class [Inner] {}`: importers use the binding name.
            let mut additions = json!({"class_expression": true});
            if let (Some(inner), Some(map)) = (
                class_node.child_by_field_name("name"),
                additions.as_object_mut(),
            ) {
                map.insert(
                    "expression_name".to_string(),
                    json!(node_text(inner, context.source)),
                );
            }
            javascript_emit_class_node(
                class_node, name, additions, context, owner_path, nodes, edges,
            );
            handled = true;
            continue;
        }
        let (Some(name), Some(function_node)) = (name, function_node) else {
            continue;
        };
        let is_test = is_javascript_test_function(&name, &context.file_path);
        let qualified = qualify(&context.file_path, &name, owner_path);
        javascript_push_node(
            context,
            nodes,
            ParsedNode {
                kind: if is_test {
                    crate::core::types::NodeKind::Test
                } else {
                    crate::core::types::NodeKind::Function
                },
                name: name.clone(),
                file_path: context.file_path.clone(),
                line_start: node.start_position().row as i64 + 1,
                line_end: node.end_position().row as i64 + 1,
                language: context.language.to_string(),
                parent_name: owner_path.map(str::to_string),
                params: javascript_child_text(function_node, context.source, "formal_parameters"),
                return_type: javascript_child_text(
                    function_node,
                    context.source,
                    "type_annotation",
                ),
                modifiers: None,
                is_test,
                extra: json!({}),
            },
        );
        let container = owner_path
            .map(|class_name| qualify(&context.file_path, class_name, None))
            .unwrap_or_else(|| context.file_path.to_string());
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: container,
            target: qualified,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
        let snapshot = context.bindings.borrow().snapshot();
        javascript_bind_this(context, owner_path);
        javascript_walk_children(
            function_node,
            context,
            owner_path,
            Some(&name),
            nodes,
            edges,
        );
        context.bindings.borrow_mut().restore(snapshot);
        handled = true;
    }
    handled
}

fn javascript_emit_field_function(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut name = None;
    let mut function_node = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "property_identifier" && name.is_none() {
            name = Some(node_text(child, context.source));
        } else if is_javascript_function_value(child.kind()) {
            function_node = Some(child);
        }
    }
    let (Some(name), Some(function_node)) = (name, function_node) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, &context.file_path);
    let qualified = qualify(&context.file_path, &name, owner_path);
    javascript_push_node(
        context,
        nodes,
        ParsedNode {
            kind: if is_test {
                crate::core::types::NodeKind::Test
            } else {
                crate::core::types::NodeKind::Function
            },
            name: name.clone(),
            file_path: context.file_path.clone(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: javascript_child_text(function_node, context.source, "formal_parameters"),
            return_type: javascript_child_text(function_node, context.source, "type_annotation"),
            modifiers: None,
            is_test,
            extra: json!({}),
        },
    );
    let container = owner_path
        .map(|class_name| qualify(&context.file_path, class_name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    let snapshot = context.bindings.borrow().snapshot();
    javascript_bind_this(context, owner_path);
    javascript_walk_children(
        function_node,
        context,
        owner_path,
        Some(&name),
        nodes,
        edges,
    );
    context.bindings.borrow_mut().restore(snapshot);
    true
}

fn javascript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(call_name) = javascript_call_name(node, context.source) else {
        return false;
    };
    let effective_call_name = if context.test_file && !is_test_runner_name(&call_name) {
        javascript_base_test_runner_name(node, context.source).unwrap_or_else(|| call_name.clone())
    } else {
        call_name.clone()
    };
    if context.test_file && is_test_runner_name(&effective_call_name) {
        let line = node.start_position().row as i64 + 1;
        let synthetic_name = match javascript_first_string_arg(node, context.source) {
            Some(description) if !description.is_empty() => {
                format!("{effective_call_name}:{description}@L{line}")
            }
            _ => format!("{effective_call_name}@L{line}"),
        };
        let qualified = qualify(&context.file_path, &synthetic_name, owner_path);
        javascript_push_node(
            context,
            nodes,
            ParsedNode {
                kind: crate::core::types::NodeKind::Test,
                name: synthetic_name.clone(),
                file_path: context.file_path.clone(),
                line_start: line,
                line_end: node.end_position().row as i64 + 1,
                language: context.language.to_string(),
                parent_name: owner_path.map(str::to_string),
                params: None,
                return_type: None,
                modifiers: None,
                is_test: true,
                extra: json!({}),
            },
        );
        let container = enclosing_func
            .map(|func| qualify(&context.file_path, func, owner_path))
            .unwrap_or_else(|| context.file_path.to_string());
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: container,
            target: qualified,
            file_path: context.file_path.clone(),
            line,
            extra: json!({}),
        });
        javascript_walk_children(
            node,
            context,
            owner_path,
            Some(&synthetic_name),
            nodes,
            edges,
        );
        return true;
    }

    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    let target = javascript_bound_member_target(node, context, owner_path)
        .unwrap_or_else(|| resolve_javascript_call_target(&call_name, context));
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = javascript_bridge_edge(node, context, &caller) {
        edges.push(edge);
    }
    false
}

fn javascript_emit_value_references(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    match node.kind() {
        "pair" => {
            if let Some(value) = javascript_pair_value_identifier(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "shorthand_property_identifier" => {
            let value = node_text(node, context.source);
            javascript_emit_reference_if_known(node, context, &caller, &value, edges);
        }
        "assignment_expression" => {
            if let Some(value) = javascript_last_identifier_child(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "array" | "arguments" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    let value = node_text(child, context.source);
                    javascript_emit_reference_if_known(child, context, &caller, &value, edges);
                }
            }
        }
        _ => {}
    }
}

fn javascript_emit_jsx_component_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = javascript_jsx_component_target(node, context) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn javascript_jsx_component_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let (base_name, component_name) = javascript_jsx_component_reference(node, context.source)?;
    if let Some(base_name) = base_name {
        return resolve_javascript_namespace_member(&base_name, &component_name, context)
            .or(Some(component_name));
    }
    Some(resolve_javascript_call_target(&component_name, context))
}

fn javascript_jsx_component_reference(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Option<String>, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(child, source);
                return looks_like_jsx_component_name(&name).then_some((None, name));
            }
            "member_expression" => {
                let component_name = javascript_rightmost_identifier(child, source)?;
                if !looks_like_jsx_component_name(&component_name) {
                    return None;
                }
                let base_name = javascript_leftmost_identifier(child, source);
                return Some((base_name, component_name));
            }
            _ => {}
        }
    }
    None
}

fn looks_like_jsx_component_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_uppercase())
}

fn javascript_emit_reference_if_known(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if javascript_should_skip_value_reference(name)
        || (!context.defined_names.contains(name) && !context.import_map.contains_key(name))
    {
        return;
    }
    let target = resolve_javascript_call_target(name, context);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::References,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn javascript_class_extra(node: tree_sitter::Node<'_>, source: &[u8], name: &str) -> Value {
    let type_role = match node.kind() {
        "abstract_class_declaration" => "abstract_class",
        "interface_declaration" => "interface",
        "enum_declaration" => "enum",
        _ => "class",
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "enum"
            && node
                .children(&mut node.walk())
                .any(|child| child.kind() == "const")
        {
            map.insert("const_enum".to_string(), json!(true));
        }
        if type_role == "interface" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
        if type_role == "abstract_class" {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if javascript_is_type_only_container(type_role)
            || javascript_is_data_model_class(node, source, name)
        {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    extra
}

fn javascript_is_type_only_container(type_role: &str) -> bool {
    type_role == "enum"
}

fn javascript_is_data_model_class(node: tree_sitter::Node<'_>, source: &[u8], name: &str) -> bool {
    if !matches!(node.kind(), "class_declaration" | "class") {
        return false;
    }
    if javascript_has_data_model_decorator(node, source) {
        return true;
    }
    if javascript_is_data_model_name(name) {
        return true;
    }
    javascript_is_property_only_class(node)
}

fn javascript_has_data_model_decorator(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let text = node_text(node, source);
    [
        "@Entity",
        "@ObjectType",
        "@InputType",
        "@ArgsType",
        "@Schema",
        "@model",
        "@Table",
    ]
    .iter()
    .any(|decorator| text.contains(decorator))
}

fn javascript_is_data_model_name(name: &str) -> bool {
    [
        "Dto", "DTO", "Data", "Payload", "Props", "State", "Model", "Entity", "Record", "Schema",
        "Input", "Output",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn javascript_is_property_only_class(node: tree_sitter::Node<'_>) -> bool {
    let mut has_data_field = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "method_definition" => return false,
            "public_field_definition" | "field_definition" | "property_signature" => {
                has_data_field = true;
            }
            _ => {
                if !javascript_class_child_is_property_only(child, &mut has_data_field) {
                    return false;
                }
            }
        }
    }
    has_data_field
}

fn javascript_class_child_is_property_only(
    node: tree_sitter::Node<'_>,
    has_data_field: &mut bool,
) -> bool {
    match node.kind() {
        "method_definition" => return false,
        "public_field_definition" | "field_definition" | "property_signature" => {
            *has_data_field = true;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !javascript_class_child_is_property_only(child, has_data_field) {
            return false;
        }
    }
    true
}

struct JavaScriptHeritageBase {
    target: String,
    role: &'static str,
    heritage_expression: Option<String>,
}

fn emit_javascript_inheritance_edges(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    qualified: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut bases = Vec::new();
    let mut mixin_calls = Vec::new();
    collect_javascript_bases(node, context, &mut bases, &mut mixin_calls);
    for base in bases {
        let mut extra = json!({"relationship_role": base.role, "syntax_source": node.kind()});
        if let (Some(expression), Some(map)) = (base.heritage_expression, extra.as_object_mut()) {
            map.insert("heritage_expression".to_string(), json!(expression));
        }
        edges.push(ParsedEdge {
            kind: if base.role == "implements" {
                crate::core::types::EdgeKind::Implements
            } else {
                crate::core::types::EdgeKind::Inherits
            },
            source: qualified.to_string(),
            target: base.target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
    }
    for (callee, line) in mixin_calls {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: qualified.to_string(),
            target: resolve_javascript_call_target(&callee, context),
            file_path: context.file_path.clone(),
            line,
            extra: json!({}),
        });
    }
}

/// Reads the heritage of one class / interface declaration (not of nested
/// declarations in its body).
///
/// TypeScript classes wrap `extends_clause` / `implements_clause` in
/// `class_heritage`; JavaScript `class_heritage` holds the base expression
/// directly; interfaces use `extends_type_clause`.
fn collect_javascript_bases(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    bases: &mut Vec<JavaScriptHeritageBase>,
    mixin_calls: &mut Vec<(String, i64)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_heritage" => {
                let mut heritage_cursor = child.walk();
                for part in child.named_children(&mut heritage_cursor) {
                    match part.kind() {
                        "extends_clause" => {
                            let mut clause_cursor = part.walk();
                            for value in part.children_by_field_name("value", &mut clause_cursor) {
                                javascript_expression_base(value, context, bases, mixin_calls);
                            }
                        }
                        "implements_clause" => {
                            javascript_type_bases(part, "implements", context, bases);
                        }
                        "comment" => {}
                        // JavaScript: `class_heritage` holds the expression itself.
                        _ => javascript_expression_base(part, context, bases, mixin_calls),
                    }
                }
            }
            "extends_type_clause" => javascript_type_bases(child, "extends", context, bases),
            _ => {}
        }
    }
}

fn javascript_expression_base(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    bases: &mut Vec<JavaScriptHeritageBase>,
    mixin_calls: &mut Vec<(String, i64)>,
) {
    let heritage_expression =
        (node.kind() != "identifier").then(|| node_text(node, context.source).trim().to_string());
    let mut current = node;
    loop {
        match current.kind() {
            "parenthesized_expression" => match current.named_child(0) {
                Some(inner) => current = inner,
                None => return,
            },
            "call_expression" => {
                if let Some(callee) = javascript_call_name(current, context.source) {
                    mixin_calls.push((callee, current.start_position().row as i64 + 1));
                }
                // The innermost identifiable argument is the real base:
                // `Mixin(Other(Base))` -> `Base`.
                let Some(argument) = current
                    .child_by_field_name("arguments")
                    .and_then(|arguments| arguments.named_child(0))
                else {
                    return;
                };
                current = argument;
            }
            "identifier" => {
                bases.push(JavaScriptHeritageBase {
                    target: node_text(current, context.source),
                    role: "extends",
                    heritage_expression,
                });
                return;
            }
            "member_expression" => {
                let Some(property) = current.child_by_field_name("property") else {
                    return;
                };
                let name = node_text(property, context.source);
                let target = current
                    .child_by_field_name("object")
                    .and_then(|object| javascript_namespace_member_target(object, &name, context))
                    .unwrap_or(name);
                bases.push(JavaScriptHeritageBase {
                    target,
                    role: "extends",
                    heritage_expression,
                });
                return;
            }
            _ => return,
        }
    }
}

fn javascript_type_bases(
    clause: tree_sitter::Node<'_>,
    role: &'static str,
    context: &JavaScriptParseContext<'_>,
    bases: &mut Vec<JavaScriptHeritageBase>,
) {
    let mut cursor = clause.walk();
    for child in clause.named_children(&mut cursor) {
        if let Some((target, heritage_expression)) = javascript_type_base(child, context) {
            bases.push(JavaScriptHeritageBase {
                target,
                role,
                heritage_expression,
            });
        }
    }
}

/// Base name of a heritage type: `Repo`, `Service<T>` -> `Service`,
/// `ns.Marker` -> the namespace-imported QN (or bare `Marker`).
fn javascript_type_base(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<(String, Option<String>)> {
    match node.kind() {
        "type_identifier" | "identifier" => Some((node_text(node, context.source), None)),
        "generic_type" => javascript_type_base(node.child_by_field_name("name")?, context),
        "nested_type_identifier" => {
            let name = node_text(node.child_by_field_name("name")?, context.source);
            let target = node
                .child_by_field_name("module")
                .and_then(|module| javascript_namespace_member_target(module, &name, context))
                .unwrap_or(name);
            Some((target, Some(node_text(node, context.source))))
        }
        _ => None,
    }
}

/// `ns.Name` where `ns` is a namespace (or default) import resolves to the
/// exporting module's QN; anything else stays unresolved.
fn javascript_namespace_member_target(
    object: tree_sitter::Node<'_>,
    name: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    if object.kind() != "identifier" {
        return None;
    }
    resolve_javascript_namespace_member(&node_text(object, context.source), name, context)
}

fn javascript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    match callee.kind() {
        "identifier" | "property_identifier" | "type_identifier" => Some(node_text(callee, source)),
        "member_expression" => javascript_rightmost_identifier(callee, source),
        _ => None,
    }
}

fn javascript_bound_member_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    if callee.kind() != "member_expression" {
        return None;
    }
    if let Some(target) = javascript_object_member_target(callee, context, owner_path) {
        return Some(target);
    }
    let method = javascript_rightmost_identifier(callee, context.source)?;
    let receiver = javascript_leftmost_identifier(callee, context.source)?;
    context.bindings.borrow().resolve_member(&receiver, &method)
}

/// `api.get()` / `api.nested.deep()` where `api` is a same-file object
/// container and the member exists, and `this.m()` inside a container
/// member when the container has `m`.
fn javascript_object_member_target(
    callee: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
) -> Option<String> {
    if context.member_paths.is_empty() {
        return None;
    }
    if let (Some(owner), Some(object), Some(property)) = (
        owner_path,
        callee.child_by_field_name("object"),
        callee.child_by_field_name("property"),
    ) && object.kind() == "this"
        && !context.namespace_paths.contains(owner)
    {
        let method = node_text(property, context.source);
        return context
            .member_paths
            .contains(&format!("{owner}.{method}"))
            .then(|| qualify(&context.file_path, &method, Some(owner)));
    }
    let path = javascript_member_path(callee, context.source)?;
    let (owner, method) = path.rsplit_once('.')?;
    let root = owner.split('.').next()?;
    if context.bindings.borrow().is_bound(root) || !context.member_paths.contains(&path) {
        return None;
    }
    Some(qualify(&context.file_path, method, Some(owner)))
}

/// Dotted text of a pure identifier member chain (`a.b.c`), or `None` when
/// any segment is computed, a call, `this`, and so on.
fn javascript_member_path(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(node, source)),
        "member_expression" => {
            let object = javascript_member_path(node.child_by_field_name("object")?, source)?;
            let property = node.child_by_field_name("property")?;
            (property.kind() == "property_identifier")
                .then(|| format!("{object}.{}", node_text(property, source)))
        }
        _ => None,
    }
}

fn javascript_bind_declarator(node: tree_sitter::Node<'_>, context: &JavaScriptParseContext<'_>) {
    if node.kind() != "variable_declarator" {
        return;
    }
    let mut ident = None;
    let mut annotated = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "type_annotation" => {
                annotated = javascript_named_child(
                    child,
                    context.source,
                    &["identifier", "type_identifier"],
                );
            }
            "new_expression" | "call_expression" => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let Some(ident) = ident else {
        return;
    };
    if let Some(value) = value
        && let Some(type_name) = javascript_inferred_constructor(value, context)
    {
        javascript_bind_receiver(context, ident, type_name);
        return;
    }
    if let Some(type_name) = annotated {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn javascript_bind_assignment(node: tree_sitter::Node<'_>, context: &JavaScriptParseContext<'_>) {
    if node.kind() != "assignment_expression" {
        return;
    }
    let mut ident = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "new_expression" | "call_expression" => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let (Some(ident), Some(value)) = (ident, value) else {
        return;
    };
    if let Some(type_name) = javascript_inferred_constructor(value, context) {
        javascript_bind_receiver(context, ident, type_name);
    }
}

fn javascript_bind_receiver(
    context: &JavaScriptParseContext<'_>,
    ident: String,
    type_name: String,
) {
    if type_name.contains('.') {
        // Only produced for verified same-file member paths.
        context.bindings.borrow_mut().bind_path(ident, type_name);
    } else {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn javascript_inferred_constructor(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    // `new Outer.Inner()`: a same-file namespace member binds by its path.
    if node.kind() == "new_expression"
        && let Some(path) = javascript_callee_node(node)
            .filter(|callee| callee.kind() == "member_expression")
            .and_then(|callee| javascript_member_path(callee, context.source))
        && context.member_paths.contains(&path)
    {
        return Some(path);
    }
    let call_name = javascript_call_name(node, context.source)?;
    context
        .bindings
        .borrow()
        .constructor_type(&call_name)
        .map(str::to_string)
}

fn javascript_callee_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if node.kind() == "new_expression" {
        if let Some(constructor) = node.child_by_field_name("constructor") {
            return Some(constructor);
        }
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        return children
            .into_iter()
            .find(|child| !matches!(child.kind(), "new" | "arguments" | "type_arguments"));
    }
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .find(|child| child.kind() != "arguments")
}

fn javascript_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_leftmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_leftmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    javascript_callee_node(node)
        .map(|callee| node_text(callee, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn javascript_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
) -> Option<ParsedEdge> {
    let signature = javascript_call_signature(node, context.source)?;
    let (relationship_role, bridge_kind) = javascript_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) =
        match javascript_first_string_arg(node, context.source) {
            Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
            _ => (
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

fn javascript_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "child_process.exec"
        | "child_process.execFile"
        | "child_process.execSync"
        | "child_process.execFileSync"
        | "child_process.spawn"
        | "child_process.spawnSync"
        | "child_process.fork" => Some(("invokes_binary", "subprocess")),
        "fs.readFile" | "fs.readFileSync" | "fs.promises.readFile" => {
            Some(("reads_file", "file_io"))
        }
        "fs.writeFile" | "fs.writeFileSync" | "fs.promises.writeFile" => {
            Some(("writes_file", "file_io"))
        }
        _ => None,
    }
}

fn javascript_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string" | "template_string") {
            return Some(decode_javascript_string_literal(child, source));
        }
        return None;
    }
    None
}

fn javascript_pair_value_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut seen_colon = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == ":" {
            seen_colon = true;
            continue;
        }
        if seen_colon && child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn javascript_last_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .rev()
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(child, source))
}

fn javascript_base_test_runner_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    if callee.kind() != "member_expression" {
        return None;
    }
    let rightmost = javascript_rightmost_identifier(callee, source)?;
    if !matches!(
        rightmost.as_str(),
        "only" | "skip" | "each" | "todo" | "concurrent"
    ) {
        return None;
    }
    let mut cursor = callee.walk();
    for child in callee.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
        if child.kind() == "member_expression" {
            let mut inner = child.walk();
            for sub in child.children(&mut inner) {
                if sub.kind() == "identifier" {
                    return Some(node_text(sub, source));
                }
            }
        }
    }
    None
}

fn is_test_runner_name(name: &str) -> bool {
    matches!(name, "describe" | "it" | "test")
}

fn is_javascript_function_value(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
}

/// Name-based test detection for declared functions.
///
/// The `Test*` / `test_*` / `*_test` / `*_spec` heuristics apply only inside
/// test files: production code routinely has names such as
/// `TestimonialCard` or `TestModeBanner`.
fn is_javascript_test_function(name: &str, file_path: &FilePath) -> bool {
    is_javascript_test_file(file_path)
        && (starts_with_ascii_ignore_case(name, "test_")
            || name.starts_with("Test")
            || name.ends_with("_test")
            || name.ends_with("_spec")
            || is_test_runner_name(name))
}

fn is_javascript_declaration_file(file_path: &FilePath) -> bool {
    [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| ends_with_ascii_ignore_case(file_path, suffix))
}

fn is_javascript_test_file(file_path: &FilePath) -> bool {
    is_test_file(file_path)
        || ends_with_ascii_ignore_case(file_path, ".test.ts")
        || ends_with_ascii_ignore_case(file_path, ".spec.ts")
        || ends_with_ascii_ignore_case(file_path, ".test.js")
        || ends_with_ascii_ignore_case(file_path, ".spec.js")
}

fn javascript_should_skip_value_reference(name: &str) -> bool {
    matches!(
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
    ) || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
}
