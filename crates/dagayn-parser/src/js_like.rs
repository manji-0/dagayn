use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::js_calls::{
    javascript_bind_assignment, javascript_bind_declarator, javascript_bind_parameters,
    javascript_emit_call, javascript_emit_jsx_component_call, javascript_emit_value_references,
};
use super::js_declarations::{
    javascript_emit_class_node, javascript_emit_default_export, javascript_emit_field_function,
    javascript_emit_function_node, javascript_emit_type_alias, javascript_emit_variable_functions,
    javascript_walk_unbound_class,
};
use super::js_decorators::{javascript_decorator_is_owned, javascript_emit_decorator};
use super::js_members::{collect_javascript_class_table, collect_javascript_type_paths};
use super::js_modules::{
    JavaScriptCaches, JavaScriptParseContext, collect_javascript_defined_names,
    collect_javascript_import_map, collect_javascript_type_names,
    javascript_dynamic_import_specifier, javascript_import_equals, javascript_import_targets,
    javascript_named_child, javascript_require_specifier,
};
use super::js_namespaces::{
    javascript_emit_namespace, javascript_emit_namespace_body, javascript_named_child_node,
    javascript_namespace_segments, javascript_umd_global_name,
};
use super::js_objects::{collect_javascript_local_exports, collect_javascript_member_paths};
use super::js_resolve::{collect_javascript_external_packages, resolve_javascript_module};
use super::js_tests::{is_javascript_test_file, is_test_runner_name};
use super::js_types::{
    javascript_emit_type_references, javascript_emit_type_roots, javascript_merge_type_references,
    javascript_type_reference_source, javascript_type_root_position,
};
use super::member_calls::MemberCallBindings;
use super::parsers::*;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    ends_with_ascii_ignore_case, line_count, node_text, starts_with_ascii_ignore_case,
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
        let mut import_map = HashMap::new();
        collect_javascript_import_map(root, source, &mut import_map);
        let mut defined_names = HashSet::new();
        collect_javascript_defined_names(root, source, &import_map, &mut defined_names);
        let mut type_names = HashSet::new();
        collect_javascript_type_names(root, source, &mut type_names);
        let external_packages =
            collect_javascript_external_packages(&import_map, file_path, repo_root, caches);
        let scopes = collect_javascript_member_paths(root, source);
        let exported_names = collect_javascript_local_exports(root, source);
        let class_table = collect_javascript_class_table(root, source);
        let type_paths = collect_javascript_type_paths(root, source);
        let context = JavaScriptParseContext {
            source,
            file_path: file_path.clone(),
            language,
            test_file,
            defined_names: &defined_names,
            import_map: &import_map,
            external_packages: &external_packages,
            member_paths: &scopes.members,
            namespace_paths: &scopes.namespaces,
            class_table: &class_table,
            type_paths: &type_paths,
            type_depth: Cell::new(0),
            exported_names: &exported_names,
            declaration_file,
            ambient_depth: Cell::new(0),
            local_scopes: RefCell::new(Vec::new()),
            repo_root,
            caches,
            bindings: RefCell::new(MemberCallBindings::with_types(type_names)),
        };
        javascript_walk_children(root, &context, None, None, &mut nodes, &mut edges);
        javascript_collapse_duplicate_nodes(&mut nodes, &mut edges);
        javascript_merge_type_references(&nodes, &mut edges, file_path);
        javascript_mark_external_edges(&mut edges, &context);
        let mut edges = resolve_rust_call_targets(&nodes, edges, file_path);
        if test_file {
            add_tested_by_edges(&nodes, &mut edges);
        }
        return (nodes, edges);
    }

    (nodes, edges)
}

/// Marks `CALLS` / `REFERENCES` into an external package (`pkg::symbol`,
/// [`javascript_external_symbol`]) with `external: true` and
/// `external_package` (the package name without a subpath), so same-file
/// resolution, `TESTED_BY`, and query-time bare-name fallbacks leave them
/// alone. Post-processing already skips `::` targets.
fn javascript_mark_external_edges(edges: &mut [ParsedEdge], context: &JavaScriptParseContext<'_>) {
    let packages = context.external_packages;
    if packages.is_empty() {
        return;
    }
    for edge in edges {
        if !matches!(
            edge.kind,
            crate::core::types::EdgeKind::Calls | crate::core::types::EdgeKind::References
        ) {
            continue;
        }
        let Some(package) = edge
            .target
            .split_once("::")
            .and_then(|(module, _)| packages.get(module))
        else {
            continue;
        };
        if let Some(map) = edge.extra.as_object_mut() {
            map.insert("external".to_string(), json!(true));
            map.insert("external_package".to_string(), json!(package));
        }
    }
}

/// One QN, one node: collapses overload signatures into their
/// implementation (`overloads: n`), getter / setter pairs into one accessor
/// (`accessors: ["get", "set"]`), and same-file declaration merging
/// (`interface Repo` twice, `function f` + `namespace f`) into one node
/// (`merged_declarations: n`), then drops the duplicate `CONTAINS` edges.
/// The graph store keeps only the last write for a QN, so without this the
/// surviving node depended on declaration order.
fn javascript_collapse_duplicate_nodes(nodes: &mut Vec<ParsedNode>, edges: &mut Vec<ParsedEdge>) {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if node.kind == crate::core::types::NodeKind::File {
            continue;
        }
        groups
            .entry(qualify(
                &node.file_path,
                &node.name,
                node.parent_name.as_deref(),
            ))
            .or_default()
            .push(index);
    }
    let mut drop = HashSet::new();
    for indexes in groups.values().filter(|indexes| indexes.len() > 1) {
        let primary = *indexes
            .iter()
            .min_by_key(|index| (javascript_merge_rank(&nodes[**index]), **index))
            .expect("non-empty group");
        let members = indexes
            .iter()
            .map(|index| &nodes[*index])
            .collect::<Vec<_>>();
        let line_start = members
            .iter()
            .map(|node| node.line_start)
            .min()
            .unwrap_or(0);
        let line_end = members.iter().map(|node| node.line_end).max().unwrap_or(0);
        let all_functions = members
            .iter()
            .all(|node| node.kind == crate::core::types::NodeKind::Function);
        let accessors = members
            .iter()
            .filter_map(|node| node.extra.get("accessors").and_then(Value::as_array))
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let signatures = members
            .iter()
            .filter(|node| javascript_is_bodiless(node))
            .count();
        let has_body = members.iter().any(|node| !javascript_is_bodiless(node));
        let modifiers = javascript_merged_modifiers(&members);
        let node = &mut nodes[primary];
        let Some(extra) = node.extra.as_object_mut() else {
            continue;
        };
        if all_functions && !accessors.is_empty() {
            let mut kinds = accessors;
            kinds.sort();
            kinds.dedup();
            extra.insert("member_role".to_string(), json!("accessor"));
            extra.insert("accessors".to_string(), json!(kinds));
            node.line_start = line_start;
            node.line_end = line_end;
            node.modifiers = modifiers;
        } else if all_functions && signatures > 0 {
            extra.insert("overloads".to_string(), json!(signatures));
            if has_body {
                extra.remove("declaration_only");
                extra.remove("is_abstract");
            }
            node.line_start = line_start;
            node.line_end = line_end;
        } else {
            extra.insert("merged_declarations".to_string(), json!(indexes.len()));
        }
        drop.extend(indexes.iter().copied().filter(|index| *index != primary));
    }
    if drop.is_empty() {
        return;
    }
    let mut index = 0;
    nodes.retain(|_| {
        let keep = !drop.contains(&index);
        index += 1;
        keep
    });
    let mut seen = HashSet::new();
    edges.retain(|edge| {
        edge.kind != crate::core::types::EdgeKind::Contains
            || seen.insert((edge.source.clone(), edge.target.clone()))
    });
}

/// Which declaration of a merged QN survives: implementations and concrete
/// types before signatures, interfaces, and namespaces.
fn javascript_merge_rank(node: &ParsedNode) -> u8 {
    let role = node.extra.get("type_role").and_then(Value::as_str);
    match node.kind {
        crate::core::types::NodeKind::Class => match role {
            Some("namespace" | "ambient_module") => 4,
            Some("interface") => 3,
            _ => 0,
        },
        crate::core::types::NodeKind::Function if javascript_is_bodiless(node) => 2,
        crate::core::types::NodeKind::Type => 1,
        _ => 0,
    }
}

fn javascript_is_bodiless(node: &ParsedNode) -> bool {
    node.kind == crate::core::types::NodeKind::Function
        && (node.extra.get("declaration_only") == Some(&json!(true))
            || node.extra.get("is_abstract") == Some(&json!(true)))
}

/// Union of accessor modifiers (`get` and `set` sides) in first-seen order.
fn javascript_merged_modifiers(members: &[&ParsedNode]) -> Option<String> {
    let mut merged: Vec<&str> = Vec::new();
    for member in members {
        for modifier in member.modifiers.as_deref().unwrap_or("").split_whitespace() {
            if !merged.contains(&modifier) {
                merged.push(modifier);
            }
        }
    }
    (!merged.is_empty()).then(|| merged.join(" "))
}

/// Walks a function-like node's body with `name` as the caller: binds
/// `this` to the owner and scopes the function's local declarations so
/// calls to them stay internal.
pub(super) fn javascript_walk_function_body(
    function_node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let snapshot = context.bindings.borrow().snapshot();
    javascript_bind_this(context, owner_path);
    javascript_bind_parameters(function_node, context);
    let locals = collect_javascript_local_declarations(function_node, context.source);
    let scoped = !locals.is_empty();
    if scoped {
        context.local_scopes.borrow_mut().push(locals);
    }
    javascript_walk_children(function_node, context, owner_path, Some(name), nodes, edges);
    if scoped {
        context.local_scopes.borrow_mut().pop();
    }
    context.bindings.borrow_mut().restore(snapshot);
}

/// Walks class-level code (a field initializer, static block, or a member
/// without a static name) with the class as the caller.
fn javascript_walk_class_level(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(class_path) = owner_path else {
        return;
    };
    let (parent, class_name) = match class_path.rsplit_once('.') {
        Some((parent, class_name)) => (Some(parent), class_name),
        None => (None, class_path),
    };
    let snapshot = context.bindings.borrow().snapshot();
    javascript_bind_this(context, Some(class_path));
    let locals = collect_javascript_local_declarations(node, context.source);
    let scoped = !locals.is_empty();
    if scoped {
        context.local_scopes.borrow_mut().push(locals);
    }
    javascript_walk_children(node, context, parent, Some(class_name), nodes, edges);
    if scoped {
        context.local_scopes.borrow_mut().pop();
    }
    context.bindings.borrow_mut().restore(snapshot);
}

/// Names declared inside `node`'s body (nested functions, classes,
/// interfaces, type aliases, enums, and `const f = () => ...` /
/// `const C = class {}` bindings), at any depth.
fn collect_javascript_local_declarations(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> HashSet<String> {
    let mut names = HashSet::new();
    let body = node.child_by_field_name("body").unwrap_or(node);
    collect_javascript_local_declarations_into(body, source, &mut names);
    names
}

fn collect_javascript_local_declarations_into(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration" => {
                if let Some(name) = child.child_by_field_name("name") {
                    names.insert(node_text(name, source));
                }
            }
            "variable_declarator" => {
                if let (Some(name), Some(value)) = (
                    child
                        .child_by_field_name("name")
                        .filter(|name| name.kind() == "identifier"),
                    child.child_by_field_name("value"),
                ) && (is_javascript_function_value(value.kind()) || value.kind() == "class")
                {
                    names.insert(node_text(name, source));
                }
            }
            _ => {}
        }
        collect_javascript_local_declarations_into(child, source, names);
    }
}

/// Whether `name` is a declaration local to the function being walked.
pub(super) fn javascript_is_local_name(context: &JavaScriptParseContext<'_>, name: &str) -> bool {
    context
        .local_scopes
        .borrow()
        .iter()
        .any(|scope| scope.contains(name))
}

pub(super) fn javascript_walk_children(
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
/// The outermost node of a type subtree also yields its type references
/// (§7.4); the subtree is still walked, but nested type nodes are not
/// collected twice.
pub(super) fn javascript_walk_node(
    child: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let position = (context.type_depth.get() == 0)
        .then(|| javascript_type_root_position(child))
        .flatten();
    let Some(position) = position else {
        javascript_walk_syntax_node(child, context, owner_path, enclosing_func, nodes, edges);
        return;
    };
    let source = javascript_type_reference_source(context, owner_path, enclosing_func, position);
    javascript_emit_type_references(child, position, &source, context, edges);
    context.type_depth.set(context.type_depth.get() + 1);
    javascript_walk_syntax_node(child, context, owner_path, enclosing_func, nodes, edges);
    context.type_depth.set(context.type_depth.get() - 1);
}

fn javascript_walk_syntax_node(
    child: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    // Inside a function body, declarations are locals: not nodes, and their
    // calls belong to the enclosing function.
    let in_function = enclosing_func.is_some();
    match child.kind() {
        "type_alias_declaration" | "interface_declaration" | "enum_declaration" if in_function => {
            // Not a node: the types it names belong to the enclosing function.
            let source = javascript_type_reference_source(
                context,
                owner_path,
                enclosing_func,
                "local_declaration",
            );
            javascript_emit_type_references(child, "local_declaration", &source, context, edges);
            return;
        }
        "class_declaration" | "abstract_class_declaration" | "class" if in_function => {
            javascript_walk_unbound_class(child, context, owner_path, enclosing_func, nodes, edges);
            return;
        }
        "function_declaration" | "generator_function_declaration" if in_function => {
            javascript_walk_children(child, context, owner_path, enclosing_func, nodes, edges);
            return;
        }
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
            // can name it, so its calls and types stay with the enclosing node.
            javascript_emit_type_roots(
                child,
                &["body"],
                context,
                owner_path,
                enclosing_func,
                edges,
            );
            if let Some(body) = child.child_by_field_name("body") {
                javascript_walk_children(body, context, owner_path, enclosing_func, nodes, edges);
            }
            return;
        }
        // A method signature of a type literal (`p: { m(): void }`,
        // `x: { a: { b(): T } }`) is part of a type, not a member: interface
        // members sit in `interface_body`, and a type's references were
        // collected from its outermost node.
        "method_signature"
            if context.type_depth.get() > 0
                || child
                    .parent()
                    .is_some_and(|parent| parent.kind() == "object_type") =>
        {
            javascript_walk_children(child, context, owner_path, enclosing_func, nodes, edges);
            return;
        }
        "function_declaration"
        | "generator_function_declaration"
        | "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "function_signature" => {
            if let Some(name) =
                javascript_emit_function_node(child, context, owner_path, nodes, edges)
            {
                javascript_walk_function_body(child, context, owner_path, &name, nodes, edges);
                return;
            }
            if child.kind() == "method_definition"
                && !in_function
                && child
                    .parent()
                    .is_some_and(|parent| parent.kind() == "class_body")
            {
                // `[Symbol.iterator]() {}`: no static name, class-level code.
                javascript_walk_class_level(child, context, owner_path, nodes, edges);
                return;
            }
        }
        "lexical_declaration" | "variable_declaration"
            if !in_function
                && javascript_emit_variable_functions(child, context, owner_path, nodes, edges) =>
        {
            return;
        }
        "public_field_definition" | "field_definition"
            if javascript_emit_field_function(child, context, owner_path, nodes, edges) =>
        {
            return;
        }
        // Field initializers, static blocks, and members without a static
        // name run as class-level code: their calls belong to the class.
        "public_field_definition" | "field_definition" | "class_static_block"
            if !in_function
                && owner_path.is_some()
                && child
                    .parent()
                    .is_some_and(|parent| parent.kind() == "class_body") =>
        {
            javascript_walk_class_level(child, context, owner_path, nodes, edges);
            return;
        }
        "import_statement" | "export_statement" => {
            for target in javascript_import_targets(child, context.source) {
                javascript_push_import(child, &target, None, context, edges);
            }
            if child.kind() == "import_statement" {
                if let Some((_, target)) = javascript_import_equals(child, context.source) {
                    javascript_push_import(child, &target, Some("import_equals"), context, edges);
                }
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
        // `require("./m")` / `import("./m")`: a dependency of the file, not a
        // call; a non-literal specifier emits nothing, but its arguments
        // still run.
        "call_expression" if javascript_is_module_call(child, context) => {
            if let Some(target) = javascript_require_specifier(child, context.source) {
                javascript_push_import(child, &target, Some("require"), context, edges);
            } else if let Some(target) = javascript_dynamic_import_specifier(child, context.source)
            {
                javascript_push_import(child, &target, Some("dynamic"), context, edges);
            }
        }
        "call_expression" | "new_expression"
            if javascript_emit_call(child, context, owner_path, enclosing_func, nodes, edges) =>
        {
            return;
        }
        "class_heritage" | "extends_type_clause" => return,
        "decorator" => {
            if !javascript_decorator_is_owned(child) {
                // Parameter and non-function field decorators: the current
                // caller (the method, or the class for field initializers).
                let caller = enclosing_func
                    .map(|func| qualify(&context.file_path, func, owner_path))
                    .unwrap_or_else(|| context.file_path.to_string());
                javascript_emit_decorator(
                    child,
                    &caller,
                    context,
                    owner_path,
                    enclosing_func,
                    nodes,
                    edges,
                );
            }
            return;
        }
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

/// An `IMPORTS_FROM` edge from the file to the module `specifier` names
/// (resolved to a repo file when possible, the raw specifier otherwise).
/// `import_kind` marks imports other than static `import` / `export from`.
fn javascript_push_import(
    node: tree_sitter::Node<'_>,
    specifier: &str,
    import_kind: Option<&str>,
    context: &JavaScriptParseContext<'_>,
    edges: &mut Vec<ParsedEdge>,
) {
    let target = resolve_javascript_module(
        specifier,
        &context.file_path,
        context.repo_root,
        context.caches,
    )
    .unwrap_or_else(|| specifier.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: context.file_path.to_string(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: import_kind.map_or_else(|| json!({}), |kind| json!({ "import_kind": kind })),
    });
}

/// A call of the CommonJS `require` (not shadowed by a declaration, an
/// import, or a local of this file) or a dynamic `import(...)`.
fn javascript_is_module_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> bool {
    let Some(function) = node.child_by_field_name("function") else {
        return false;
    };
    match function.kind() {
        "import" => true,
        "identifier" => {
            node_text(function, context.source) == "require"
                && !context.defined_names.contains("require")
                && !context.import_map.contains_key("require")
                && !javascript_is_local_name(context, "require")
        }
        _ => false,
    }
}

/// Pushes a node for the declaration `syntax`, marking it `ambient` inside
/// `declare` / ambient-module bodies and in declaration files, and
/// `exported` when the declaration is exported from its module or
/// namespace (`export ...` or a local `export { name }`).
pub(super) fn javascript_push_node(
    context: &JavaScriptParseContext<'_>,
    nodes: &mut Vec<ParsedNode>,
    syntax: tree_sitter::Node<'_>,
    mut node: ParsedNode,
) {
    let exported = javascript_declaration_is_exported(syntax)
        || (node.parent_name.is_none() && context.exported_names.contains(&node.name));
    if let Some(map) = node.extra.as_object_mut() {
        if context.declaration_file || context.ambient_depth.get() > 0 {
            map.insert("ambient".to_string(), json!(true));
        }
        if exported && node.kind != crate::core::types::NodeKind::Test {
            map.insert("exported".to_string(), json!(true));
        }
    }
    nodes.push(node);
}

/// Whether `syntax` is the declaration of an `export` statement, looking
/// through the wrappers between a declaration and its statement
/// (`export const x = ...`, `export declare function f()`).
fn javascript_declaration_is_exported(syntax: tree_sitter::Node<'_>) -> bool {
    if syntax.kind() == "export_statement" {
        return true;
    }
    let mut current = syntax;
    loop {
        let Some(parent) = current.parent() else {
            return false;
        };
        match parent.kind() {
            "export_statement" => return true,
            "variable_declarator"
            | "lexical_declaration"
            | "variable_declaration"
            | "ambient_declaration"
            | "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression" => current = parent,
            _ => return false,
        }
    }
}

/// Binds `this` / `self` to the owner when members of that owner see one
/// (classes, object containers), not for namespaces.
pub(super) fn javascript_bind_this(context: &JavaScriptParseContext<'_>, owner_path: Option<&str>) {
    if let Some(owner) = owner_path
        && !context.namespace_paths.contains(owner)
    {
        context.bindings.borrow_mut().bind_implicit_receivers(owner);
    }
}

/// Owner path of the members of container `name` declared under
/// `owner_path` (`Outer` + `Inner` -> `Outer.Inner`).
pub(super) fn javascript_member_owner(owner_path: Option<&str>, name: &str) -> String {
    owner_path
        .map(|parent| format!("{parent}.{name}"))
        .unwrap_or_else(|| name.to_string())
}

pub(super) fn javascript_container_qn(
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
) -> String {
    owner_path
        .map(|class_name| qualify(&context.file_path, class_name, None))
        .unwrap_or_else(|| context.file_path.to_string())
}

pub(super) fn is_javascript_function_value(kind: &str) -> bool {
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
pub(super) fn is_javascript_test_function(name: &str, file_path: &FilePath) -> bool {
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
