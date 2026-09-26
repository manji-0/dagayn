use std::collections::HashSet;

use serde_json::{Value, json};

use super::js_declarations::{javascript_emit_bound_function, javascript_property_name};
use super::js_like::{
    is_javascript_function_value, javascript_container_qn, javascript_member_owner,
    javascript_push_node, javascript_walk_node,
};
use super::js_modules::JavaScriptParseContext;
use super::js_namespaces::{javascript_named_child_node, javascript_namespace_segments};
use super::qualify;
use super::types::{ParsedEdge, ParsedNode};
use super::util::node_text;

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
    javascript_property_name(key, source)
}

/// Deepest object-literal nesting modeled as containers
/// (`api.a.b.c` is depth 3); deeper objects are walked without nodes.
pub(super) const JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH: usize = 6;

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
pub(super) fn javascript_module_object_containers<'tree>(
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

/// Local names exported by `export { a, b as c }` (without `from`).
pub(super) fn collect_javascript_local_exports(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        let mut probe = statement.walk();
        let reexport = statement
            .children(&mut probe)
            .any(|child| child.kind() == "string");
        if statement.kind() != "export_statement" || reexport {
            continue;
        }
        let mut statement_cursor = statement.walk();
        for clause in statement.named_children(&mut statement_cursor) {
            if clause.kind() != "export_clause" {
                continue;
            }
            let mut clause_cursor = clause.walk();
            for spec in clause.named_children(&mut clause_cursor) {
                if spec.kind() == "export_specifier"
                    && let Some(name) = spec.child_by_field_name("name")
                {
                    names.insert(node_text(name, source));
                }
            }
        }
    }
    names
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
pub(super) fn javascript_emit_object_container(
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
        declaration,
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
pub(super) fn javascript_declarator_owns(
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
