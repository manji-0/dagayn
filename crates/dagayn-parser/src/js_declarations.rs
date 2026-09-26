use serde_json::{Value, json};

use super::js_calls::{javascript_emit_call, javascript_emit_value_references};
use super::js_decorators::{
    javascript_class_decorators, javascript_decorator_names, javascript_emit_decorators,
    javascript_field_decorator_names, javascript_has_data_model_decorator_name,
    javascript_insert_decorator_names, javascript_member_decorators,
};
use super::js_heritage::emit_javascript_inheritance_edges;
use super::js_like::{
    is_javascript_function_value, is_javascript_test_function, javascript_container_qn,
    javascript_member_owner, javascript_push_node, javascript_walk_children,
    javascript_walk_function_body, javascript_walk_node,
};
use super::js_modules::{
    JavaScriptParseContext, JavaScriptWrappedFunction, decode_javascript_string_literal,
    javascript_child_text, javascript_function_name, javascript_wrapped_function,
};
use super::js_objects::{
    JAVASCRIPT_MAX_OBJECT_CONTAINER_DEPTH, javascript_declarator_owns,
    javascript_emit_object_container, javascript_module_object_containers,
};
use super::js_types::{
    javascript_emit_type_references, javascript_emit_type_roots, javascript_type_reference_source,
};
use super::qualify;
use super::types::{ParsedEdge, ParsedNode};
use super::util::node_text;

/// Emits a `Class` node for a class-like declaration or class expression,
/// its CONTAINS and heritage edges, and walks its body with the class as the
/// owner. `additions` are merged into the role metadata.
pub(super) fn javascript_emit_class_node(
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
    let decorators = javascript_class_decorators(node);
    let member_decorators = javascript_field_decorator_names(node, context.source);
    if let Some(map) = extra.as_object_mut() {
        let names = javascript_decorator_names(&decorators, context.source);
        if javascript_has_data_model_decorator_name(&names)
            && map.get("container_role").is_none()
            && node.kind() != "interface_declaration"
        {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
        if !names.is_empty() {
            map.insert("decorators".to_string(), json!(names));
        }
        if !member_decorators.is_empty() {
            map.insert("member_decorators".to_string(), json!(member_decorators));
        }
    }
    javascript_push_node(
        context,
        nodes,
        node,
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
            modifiers: javascript_modifiers(&[node]),
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
    // Class decorator arguments run as class-level code.
    javascript_emit_decorators(
        &decorators,
        &qualified,
        context,
        owner_path,
        name,
        Some(&member_owner),
        nodes,
        edges,
    );
    javascript_walk_children(node, context, Some(&member_owner), None, nodes, edges);
}

/// Emits `Type <name>` (`type_role: "alias"`, `alias_form`) for
/// `type T = ...`. Object-shaped aliases are data containers. The right-hand
/// side has no calls and its members are not nodes: an alias is neither
/// nominal nor a container.
pub(super) fn javascript_emit_type_alias(
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
        node,
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
    let qualified = qualify(&context.file_path, name, owner_path);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: javascript_container_qn(context, owner_path),
        target: qualified.clone(),
        file_path: context.file_path.clone(),
        line,
        extra: json!({}),
    });
    for (field, position) in [
        ("type_parameters", "type_parameter"),
        ("value", "type_alias"),
    ] {
        if let Some(part) = node.child_by_field_name(field) {
            javascript_emit_type_references(part, position, &qualified, context, edges);
        }
    }
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

/// Walks an anonymous class expression that has no binding
/// (`return class extends Base { ... }`, `define(class { ... })`).
///
/// Nothing outside can name its members, so they are not nodes; calls in
/// member bodies and initializers stay attributed to the enclosing node
/// instead of being flattened into top-level functions.
pub(super) fn javascript_walk_unbound_class(
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
    // Its heritage, type parameters, and member signatures name types for
    // the enclosing node; the walk below covers member bodies and values.
    let source = javascript_type_reference_source(context, owner_path, enclosing_func, "heritage");
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        match part.kind() {
            "class_heritage" => {
                javascript_emit_type_references(part, "heritage", &source, context, edges);
            }
            "type_parameters" => {
                javascript_emit_type_references(part, "type_parameter", &source, context, edges);
            }
            _ => {}
        }
    }
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let walked: &[&str] = match member.kind() {
            "method_definition" => &["body"],
            "public_field_definition" | "field_definition" => {
                match member.child_by_field_name("value") {
                    Some(value) if is_javascript_function_value(value.kind()) => &["value"],
                    // The walk visits the whole member, annotation included.
                    Some(_) => &["name", "type", "value"],
                    None => &[],
                }
            }
            "class_static_block" => &["body"],
            _ => &[],
        };
        javascript_emit_type_roots(member, walked, context, owner_path, enclosing_func, edges);
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
pub(super) fn javascript_emit_default_export(
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
        // `export default memo(function Page() {})`: `default`, like any
        // default without a binding.
        "call_expression" => {
            let Some(wrapped) =
                javascript_wrapped_function(value, context.source, context.import_map)
            else {
                return false;
            };
            let extra = javascript_wrapped_extra(
                &wrapped,
                json!({"export_default": true, "anonymous": true}),
                context.source,
            );
            javascript_emit_wrapper_calls(&wrapped, context, owner_path, "default", nodes, edges);
            javascript_emit_bound_function(
                node,
                wrapped.function,
                "default",
                extra,
                context,
                owner_path,
                nodes,
                edges,
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
pub(super) fn javascript_emit_bound_function(
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
        declaration,
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
            modifiers: javascript_modifiers(&[declaration, function_node]),
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
    javascript_walk_function_body(function_node, context, owner_path, name, nodes, edges);
}

pub(super) fn javascript_emit_function_node(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> Option<String> {
    let name = javascript_member_name(node, context.source)?;
    let is_test = is_javascript_test_function(&name, &context.file_path);
    let qualified = qualify(&context.file_path, &name, owner_path);
    let mut extra = javascript_signature_extra(node);
    if let Some(accessor) = javascript_accessor_kind(node)
        && let Some(map) = extra.as_object_mut()
    {
        map.insert("member_role".to_string(), json!("accessor"));
        map.insert("accessors".to_string(), json!([accessor]));
    }
    let decorators = javascript_member_decorators(node);
    javascript_insert_decorator_names(&mut extra, &decorators, context.source);
    javascript_push_node(
        context,
        nodes,
        node,
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
            modifiers: javascript_modifiers(&[node]),
            is_test,
            extra,
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
    javascript_emit_decorators(
        &decorators,
        &qualify(&context.file_path, &name, owner_path),
        context,
        owner_path,
        &name,
        owner_path,
        nodes,
        edges,
    );
    Some(name)
}

/// Name of a function-like declaration or class / object member:
/// identifiers, `#private` names, string and number literal keys, and
/// computed keys holding a literal (`["computed"]() {}`). Other computed
/// keys (`[Symbol.iterator]`) have no static name.
pub(super) fn javascript_member_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "public_field_definition"
        | "field_definition" => node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("property"))
            .and_then(|key| javascript_property_name(key, source)),
        _ => javascript_function_name(node, source),
    }
}

/// Static name of a property key.
pub(super) fn javascript_property_name(
    key: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    match key.kind() {
        "property_identifier"
        | "identifier"
        | "private_property_identifier"
        | "type_identifier" => Some(node_text(key, source)),
        "string" => {
            Some(decode_javascript_string_literal(key, source)).filter(|name| !name.is_empty())
        }
        "number" => Some(node_text(key, source)),
        "computed_property_name" => {
            let inner = key.named_child(0)?;
            match inner.kind() {
                "string" | "number" => javascript_property_name(inner, source),
                "template_string"
                    if inner.named_child_count() == 0
                        || (inner.named_child_count() == 1
                            && inner
                                .named_child(0)
                                .is_some_and(|part| part.kind() == "string_fragment")) =>
                {
                    Some(decode_javascript_string_literal(inner, source))
                        .filter(|name| !name.is_empty())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// `get` / `set` for accessor members.
fn javascript_accessor_kind(node: tree_sitter::Node<'_>) -> Option<&'static str> {
    if !matches!(
        node.kind(),
        "method_definition" | "method_signature" | "abstract_method_signature"
    ) {
        return None;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find_map(|child| match child.kind() {
            "get" => Some("get"),
            "set" => Some("set"),
            _ => None,
        })
}

/// Space-separated declaration modifiers in source order (`static`,
/// `async`, `*`, `get`, `set`, `readonly`, `public` / `private` /
/// `protected`, `override`, `declare`, `abstract`, `accessor`), read from
/// the direct children of `parts` (a declaration and, for bound functions,
/// its function literal).
fn javascript_modifiers(parts: &[tree_sitter::Node<'_>]) -> Option<String> {
    let mut modifiers: Vec<&'static str> = Vec::new();
    for part in parts {
        let mut cursor = part.walk();
        for child in part.children(&mut cursor) {
            let modifier = match child.kind() {
                "static" => "static",
                "async" => "async",
                "*" => "*",
                "get" => "get",
                "set" => "set",
                "readonly" => "readonly",
                "override_modifier" => "override",
                "declare" => "declare",
                "abstract" => "abstract",
                "accessor" => "accessor",
                "accessibility_modifier" => match child.child(0).map(|token| token.kind()) {
                    Some("public") => "public",
                    Some("private") => "private",
                    Some("protected") => "protected",
                    _ => continue,
                },
                _ => continue,
            };
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
        }
    }
    (!modifiers.is_empty()).then(|| modifiers.join(" "))
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

pub(super) fn javascript_emit_variable_functions(
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
        // `const api: Api = { ... }`, `const h: Handler = () => ...`: the
        // binding's annotation belongs to the node the binding becomes.
        let annotate = |name: &str, edges: &mut Vec<ParsedEdge>| {
            if let Some(annotation) = declarator.child_by_field_name("type") {
                let qualified = qualify(&context.file_path, name, owner_path);
                javascript_emit_type_references(
                    annotation,
                    "variable_annotation",
                    &qualified,
                    context,
                    edges,
                );
            }
        };
        if let Some((name, object)) = containers.iter().find(|(_, object)| {
            object
                .parent()
                .is_some_and(|parent| javascript_declarator_owns(declarator, parent))
        }) {
            annotate(name, edges);
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
            annotate(name, edges);
            handled = true;
            continue;
        }
        // `const Comp = memo(function Inner() {})`: the binding is the
        // function importers and JSX name.
        let wrapped = function_node
            .is_none()
            .then(|| declarator.child_by_field_name("value"))
            .flatten()
            .and_then(|value| {
                javascript_wrapped_function(value, context.source, context.import_map)
            });
        let function_node = function_node.or(wrapped.as_ref().map(|wrapped| wrapped.function));
        let (Some(name), Some(function_node)) = (name, function_node) else {
            continue;
        };
        let is_test = is_javascript_test_function(&name, &context.file_path);
        let qualified = qualify(&context.file_path, &name, owner_path);
        let extra = wrapped
            .as_ref()
            .map(|wrapped| javascript_wrapped_extra(wrapped, json!({}), context.source))
            .unwrap_or_else(|| json!({}));
        javascript_push_node(
            context,
            nodes,
            node,
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
                modifiers: javascript_modifiers(&[function_node]),
                is_test,
                extra,
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
        annotate(&name, edges);
        if let Some(wrapped) = &wrapped {
            javascript_emit_wrapper_calls(wrapped, context, owner_path, &name, nodes, edges);
        }
        javascript_walk_function_body(function_node, context, owner_path, &name, nodes, edges);
        handled = true;
    }
    handled
}

/// `extra` plus `wrapped_by` (wrapper callees, outermost first) and the
/// wrapped function's own name as `expression_name`.
fn javascript_wrapped_extra(
    wrapped: &JavaScriptWrappedFunction<'_>,
    mut extra: Value,
    source: &[u8],
) -> Value {
    if let Some(map) = extra.as_object_mut() {
        map.insert(
            "wrapped_by".to_string(),
            json!(wrapped.wrapper_names(source)),
        );
        if let Some(inner) = wrapped.function.child_by_field_name("name") {
            map.insert(
                "expression_name".to_string(),
                json!(node_text(inner, source)),
            );
        }
    }
    extra
}

/// The wrapper calls of a wrapped function run where the binding is
/// declared: their `CALLS` and their other arguments (`areEqual` in
/// `memo(C, areEqual)`) belong to the container, not to the function. Type
/// arguments (`forwardRef<Ref, Props>`) describe the function, so their
/// references come from its node.
fn javascript_emit_wrapper_calls(
    wrapped: &JavaScriptWrappedFunction<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    for (index, call) in wrapped.calls.iter().enumerate() {
        javascript_emit_call(*call, context, owner_path, None, nodes, edges);
        if let Some(type_arguments) = call.child_by_field_name("type_arguments") {
            javascript_walk_node(
                type_arguments,
                context,
                owner_path,
                Some(name),
                nodes,
                edges,
            );
        }
        let Some(arguments) = call.child_by_field_name("arguments") else {
            continue;
        };
        javascript_emit_value_references(arguments, context, owner_path, None, edges);
        let inner = wrapped
            .calls
            .get(index + 1)
            .copied()
            .unwrap_or(wrapped.function);
        let mut cursor = arguments.walk();
        for argument in arguments.named_children(&mut cursor) {
            if argument.id() != inner.id() {
                javascript_walk_node(argument, context, owner_path, None, nodes, edges);
            }
        }
    }
}

pub(super) fn javascript_emit_field_function(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let name = javascript_member_name(node, context.source);
    let function_node = node
        .child_by_field_name("value")
        .filter(|value| is_javascript_function_value(value.kind()));
    let (Some(name), Some(function_node)) = (name, function_node) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, &context.file_path);
    let qualified = qualify(&context.file_path, &name, owner_path);
    let decorators = javascript_member_decorators(node);
    let mut extra = json!({});
    javascript_insert_decorator_names(&mut extra, &decorators, context.source);
    javascript_push_node(
        context,
        nodes,
        node,
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
            modifiers: javascript_modifiers(&[node, function_node]),
            is_test,
            extra,
        },
    );
    let container = owner_path
        .map(|class_name| qualify(&context.file_path, class_name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified.clone(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    javascript_emit_decorators(
        &decorators,
        &qualify(&context.file_path, &name, owner_path),
        context,
        owner_path,
        &name,
        owner_path,
        nodes,
        edges,
    );
    // `handler: Handler = () => ...`: the field's own annotation.
    if let Some(annotation) = node.child_by_field_name("type") {
        javascript_emit_type_references(annotation, "field", &qualified, context, edges);
    }
    javascript_walk_function_body(function_node, context, owner_path, &name, nodes, edges);
    true
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
            "public_field_definition" | "field_definition"
                if javascript_is_function_field(child) =>
            {
                return false;
            }
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

/// A class field holding a function literal (`handle = () => {}`): a method.
fn javascript_is_function_field(node: tree_sitter::Node<'_>) -> bool {
    node.child_by_field_name("value")
        .is_some_and(|value| is_javascript_function_value(value.kind()))
}

fn javascript_class_child_is_property_only(
    node: tree_sitter::Node<'_>,
    has_data_field: &mut bool,
) -> bool {
    match node.kind() {
        "method_definition" => return false,
        "public_field_definition" | "field_definition" if javascript_is_function_field(node) => {
            return false;
        }
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
