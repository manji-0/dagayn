use std::cell::RefCell;
use std::collections::HashSet;

use serde_json::json;

use super::member_calls::MemberCallBindings;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text};
use super::{add_tested_by_edges, is_test_function, qualify, resolve_rust_call_targets};

pub(super) fn parse_rust_with_parser(
    file_path: &str,
    source: &[u8],
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
        language: "rust".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let mut defined_names = HashSet::new();
            collect_rust_defined_names(root, source, &mut defined_names);
            let mut type_names = HashSet::new();
            collect_rust_type_names(root, source, &mut type_names);
            let context = RustParseContext {
                source,
                file_path: file_path.clone(),
                defined_names: &defined_names,
                bindings: RefCell::new(MemberCallBindings::with_types(type_names)),
            };
            rust_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            let mut edges = resolve_rust_call_targets(&nodes, edges, &file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn rust_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_item" | "enum_item" | "trait_item" | "type_item" => {
                if let Some(name) = rust_type_name(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, enclosing_class);
                    let kind = if child.kind() == "type_item" {
                        crate::core::types::NodeKind::Type
                    } else {
                        crate::core::types::NodeKind::Class
                    };
                    nodes.push(ParsedNode {
                        kind,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: rust_type_modifiers(child, context.source),
                        is_test: false,
                        extra: rust_type_extra(child, context.source),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: context.file_path.to_string(),
                        target: qualified,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_emit_type_references(
                        child,
                        context.source,
                        &context.file_path,
                        &qualify(&context.file_path, &name, enclosing_class),
                        context.defined_names,
                        Some(&name),
                        edges,
                    );
                    rust_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "impl_item" => {
                if let Some(type_name) = rust_impl_type_name(child, context.source) {
                    if let Some(trait_name) = rust_impl_trait_name(child, context.source) {
                        edges.push(ParsedEdge {
                            kind: crate::core::types::EdgeKind::Implements,
                            source: qualify(&context.file_path, &type_name, None),
                            target: trait_name,
                            file_path: context.file_path.clone(),
                            line: child.start_position().row as i64 + 1,
                            extra: json!({
                                "relationship_role": "implements",
                                "syntax_source": "impl_item",
                            }),
                        });
                    }
                    rust_walk_children(child, context, Some(&type_name), None, nodes, edges);
                    continue;
                }
            }
            "function_item" | "function_signature_item" => {
                if let Some(name) = rust_identifier_child(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, enclosing_class);
                    let params = rust_child_text(child, context.source, "parameters");
                    let is_test =
                        is_test_function(&name, &context.file_path, child, context.source);
                    let extra = if child.kind() == "function_signature_item" {
                        json!({"is_abstract": true})
                    } else {
                        json!({})
                    };
                    nodes.push(ParsedNode {
                        kind: if is_test {
                            crate::core::types::NodeKind::Test
                        } else {
                            crate::core::types::NodeKind::Function
                        },
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params,
                        return_type: None,
                        modifiers: None,
                        is_test,
                        extra,
                    });
                    let container = enclosing_class
                        .map(|name| qualify(&context.file_path, name, None))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: container,
                        target: qualified,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_emit_type_references(
                        child,
                        context.source,
                        &context.file_path,
                        &qualify(&context.file_path, &name, enclosing_class),
                        context.defined_names,
                        Some(&name),
                        edges,
                    );
                    let snapshot = context.bindings.borrow().snapshot();
                    if let Some(class_name) = enclosing_class {
                        context
                            .bindings
                            .borrow_mut()
                            .bind_implicit_receivers(class_name);
                    }
                    rust_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    context.bindings.borrow_mut().restore(snapshot);
                    continue;
                }
            }
            "use_declaration" => {
                if let Some(target) = rust_use_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            "call_expression" | "macro_invocation" => {
                if let Some(call_name) = rust_bound_member_target(child, context)
                    .or_else(|| rust_call_name(child, context.source))
                {
                    let caller = enclosing_func
                        .map(|name| qualify(&context.file_path, name, enclosing_class))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Calls,
                        source: caller.clone(),
                        target: call_name.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    if let Some(edge) = rust_bridge_edge(
                        child,
                        context.source,
                        &context.file_path,
                        &caller,
                        &call_name,
                    ) {
                        edges.push(edge);
                    }
                }
            }

            "arguments" => {
                rust_emit_argument_references(
                    child,
                    context.source,
                    &context.file_path,
                    enclosing_class,
                    enclosing_func,
                    context.defined_names,
                    edges,
                );
            }
            _ => {}
        }
        rust_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        rust_bind_let(child, context);
    }
}

struct RustParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    defined_names: &'a HashSet<String>,
    bindings: RefCell<MemberCallBindings>,
}

fn collect_rust_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            if let Some(name) = rust_type_name(node, source) {
                names.insert(name);
            }
        }
        "impl_item" => {
            if let Some(name) = rust_impl_type_name(node, source) {
                names.insert(name);
            }
        }
        "function_item" | "function_signature_item" => {
            if let Some(name) = rust_identifier_child(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_defined_names(child, source, names);
    }
}

fn collect_rust_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            if let Some(name) = rust_type_name(node, source) {
                names.insert(name);
            }
        }
        "impl_item" => {
            if let Some(name) = rust_impl_type_name(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_type_names(child, source, names);
    }
}

fn rust_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    rust_identifier_child(node, source)
}

fn rust_impl_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("type")
        .and_then(|ty| rust_type_ident(ty, source))
}

fn rust_impl_trait_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("trait")
        .and_then(|ty| rust_type_ident(ty, source))
}

fn rust_type_ident(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node_text(node, source)),
        "generic_type" | "scoped_type_identifier" | "pointer_type" | "reference_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if let Some(name) = rust_type_ident(child, source) {
                    return Some(name);
                }
            }
            None
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    return Some(node_text(child, source));
                }
            }
            None
        }
    }
}

fn rust_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "field_identifier"
        ) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_type_role(kind: &str) -> &'static str {
    match kind {
        "enum_item" => "enum",
        "trait_item" => "trait",
        "type_item" => "alias",
        "struct_item" => "struct",
        _ => "class",
    }
}

fn rust_type_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> serde_json::Value {
    let type_role = rust_type_role(node.kind());
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "trait" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
        if rust_is_value_container(type_role, node, source) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    if let Some(derive_traits) = rust_derive_traits(node, source) {
        extra["derive_traits"] = json!(derive_traits);
    }
    extra
}

fn rust_node_with_leading_attributes(
    node: tree_sitter::Node<'_>,
) -> impl Iterator<Item = tree_sitter::Node<'_>> {
    let mut attrs = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        if matches!(sibling.kind(), "attribute_item" | "inner_attribute_item") {
            attrs.push(sibling);
            current = sibling.prev_sibling();
            continue;
        }
        break;
    }
    attrs.reverse();
    attrs.into_iter().chain(std::iter::once(node))
}

fn rust_is_value_container(type_role: &str, node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    matches!(type_role, "struct" | "enum") || rust_derives_value_semantics(node, source)
}

fn rust_derives_value_semantics(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    rust_derive_traits(node, source).is_some_and(|traits| {
        traits
            .iter()
            .any(|name| matches!(name.as_str(), "Serialize" | "Deserialize"))
    })
}

fn rust_type_modifiers(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut modifiers = Vec::new();
    if rust_has_pub_visibility(node, source) {
        modifiers.push("pub");
    }
    if modifiers.is_empty() {
        None
    } else {
        Some(modifiers.join(" "))
    }
}

fn rust_has_pub_visibility(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    for candidate in rust_node_with_leading_attributes(node) {
        let mut cursor = candidate.walk();
        for child in candidate.children(&mut cursor) {
            if child.kind() == "visibility_modifier" {
                let text = node_text(child, source);
                if text.starts_with("pub") {
                    return true;
                }
            }
        }
    }
    false
}

fn rust_derive_traits(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut traits = Vec::new();
    for candidate in rust_node_with_leading_attributes(node) {
        let texts = if matches!(candidate.kind(), "attribute_item" | "inner_attribute_item") {
            vec![node_text(candidate, source)]
        } else {
            let mut cursor = candidate.walk();
            candidate
                .children(&mut cursor)
                .filter(|child| matches!(child.kind(), "attribute_item" | "inner_attribute_item"))
                .map(|child| node_text(child, source))
                .collect::<Vec<_>>()
        };
        for text in texts {
            let Some(args) = text.strip_prefix("#[derive(") else {
                continue;
            };
            let Some(args) = args.strip_suffix(")]") else {
                continue;
            };
            for trait_name in args.split(',') {
                let trimmed = trait_name.trim();
                if !trimmed.is_empty() {
                    traits.push(trimmed.to_string());
                }
            }
        }
    }
    if traits.is_empty() {
        None
    } else {
        Some(traits)
    }
}

fn rust_use_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    Some(
        text.replace("use ", "")
            .trim_end_matches(';')
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn rust_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "scoped_identifier" => return Some(node_text(child, source)),
            "field_expression" => return rust_rightmost_identifier(child, source),
            _ => {}
        }
    }
    None
}

fn rust_bound_member_target(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
) -> Option<String> {
    let mut cursor = node.walk();
    let field = node
        .children(&mut cursor)
        .find(|child| child.kind() == "field_expression")?;
    let method = rust_rightmost_identifier(field, context.source)?;
    let receiver = rust_leftmost_identifier(field, context.source)?;
    context.bindings.borrow().resolve_member(&receiver, &method)
}

fn rust_leftmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = rust_leftmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn rust_bind_let(node: tree_sitter::Node<'_>, context: &RustParseContext<'_>) {
    if node.kind() != "let_declaration" {
        return;
    }
    let mut ident = node.child_by_field_name("pattern").and_then(|pattern| {
        if pattern.kind() == "identifier" {
            Some(node_text(pattern, context.source))
        } else {
            rust_identifier_child(pattern, context.source)
        }
    });
    let mut annotated = node
        .child_by_field_name("type")
        .and_then(|ty| rust_type_ident(ty, context.source));
    let mut value = node.child_by_field_name("value");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "type_identifier" if annotated.is_none() => {
                annotated = Some(node_text(child, context.source));
            }
            "generic_type" | "scoped_type_identifier" | "reference_type" if annotated.is_none() => {
                if let Some(name) = rust_type_ident(child, context.source) {
                    annotated = Some(name);
                }
            }
            "call_expression" | "struct_expression" | "macro_invocation" if value.is_none() => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let Some(ident) = ident else {
        return;
    };
    if let Some(value) = value {
        if let Some(call_name) =
            rust_call_name(value, context.source).or_else(|| rust_type_ident(value, context.source))
        {
            let type_name = context
                .bindings
                .borrow()
                .constructor_type(&call_name)
                .map(str::to_string);
            if let Some(type_name) = type_name {
                context.bindings.borrow_mut().bind(ident, type_name);
                return;
            }
        }
    }
    if let Some(type_name) = annotated {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn rust_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = rust_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn rust_emit_argument_references(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|name| qualify(file_path, name, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "identifier" {
            continue;
        }
        let name = node_text(child, source);
        if rust_should_skip_value_reference(&name) || !defined_names.contains(&name) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::References,
            source: caller.clone(),
            target: qualify(file_path, &name, None),
            file_path: file_path.clone(),
            line: child.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn rust_emit_type_references(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    source_qualified: &str,
    defined_names: &HashSet<String>,
    skip_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut emitted = HashSet::new();
    let context = RustTypeReferenceContext {
        source,
        file_path: file_path.clone(),
        source_qualified,
        defined_names,
        skip_name,
    };
    rust_collect_type_references(node, edges, &mut emitted, &context);
}

struct RustTypeReferenceContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    source_qualified: &'a str,
    defined_names: &'a HashSet<String>,
    skip_name: Option<&'a str>,
}

fn rust_collect_type_references(
    node: tree_sitter::Node<'_>,
    edges: &mut Vec<ParsedEdge>,
    emitted: &mut HashSet<String>,
    context: &RustTypeReferenceContext<'_>,
) {
    if node.kind() == "type_identifier" {
        let name = node_text(node, context.source);
        if context.skip_name != Some(name.as_str())
            && context.defined_names.contains(&name)
            && emitted.insert(name.clone())
        {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::References,
                source: context.source_qualified.to_string(),
                target: qualify(&context.file_path, &name, None),
                file_path: context.file_path.clone(),
                line: node.start_position().row as i64 + 1,
                extra: json!({
                    "relationship_role": "type_reference",
                    "evidence_kind": "rust_type_identifier"
                }),
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        rust_collect_type_references(child, edges, emitted, context);
    }
}

fn rust_should_skip_value_reference(name: &str) -> bool {
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

fn rust_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = rust_call_signature(node, source).unwrap_or_else(|| call_name.to_string());
    let (relationship_role, bridge_kind) = rust_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match rust_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "rust",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn rust_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let signature = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty());
    signature
}

fn rust_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "std::process::Command::new" | "Command::new" => Some(("invokes_binary", "subprocess")),
        "std::fs::read"
        | "std::fs::read_to_string"
        | "std::fs::File::open"
        | "fs::read"
        | "fs::read_to_string"
        | "File::open" => Some(("reads_file", "file_io")),
        "std::fs::write" | "std::fs::File::create" | "fs::write" | "File::create" => {
            Some(("writes_file", "file_io"))
        }
        "libloading::Library::new" | "Library::new" => Some(("loads_shared_library", "ffi")),
        _ => None,
    }
}

fn rust_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string_literal" | "raw_string_literal") {
            return Some(decode_rust_string_literal(child, source));
        }
        return None;
    }
    None
}

fn decode_rust_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_content" | "string_fragment") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('`')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::new_rust_parser;
    use super::parse_rust_with_parser;

    #[test]
    fn test_parse_rust_with_parser_emits_tested_by_for_cfg_test() {
        let source = br#"
fn production() {}

#[test]
fn test_production() {
    production();
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (nodes, edges) = parse_rust_with_parser("src/lib.rs", source, Some(&mut parser));

        assert!(nodes
            .iter()
            .any(|node| node.kind == "Test" && node.name == "test_production" && node.is_test));
        assert!(edges.iter().any(|edge| {
            edge.kind == "TESTED_BY"
                && edge.source == "src/lib.rs::production"
                && edge.target == "src/lib.rs::test_production"
        }));
    }

    #[test]
    fn test_parse_rust_with_parser_marks_attribute_tests_without_name_prefix() {
        let source = br#"
#[test]
fn helpers_have_stable_contracts() {
    assert_eq!(1, 1);
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (nodes, _edges) = parse_rust_with_parser("src/tests.rs", source, Some(&mut parser));

        assert!(nodes.iter().any(|node| {
            node.kind == "Test" && node.name == "helpers_have_stable_contracts" && node.is_test
        }));
    }

    #[test]
    fn test_parse_rust_with_parser_emits_type_reference_edges() {
        let source = br#"
struct User {
    manager: Option<Box<User>>,
}

struct Repository {}

fn create_user(repo: Repository) -> User {
    User { manager: None }
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (_nodes, edges) = parse_rust_with_parser("src/lib.rs", source, Some(&mut parser));

        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "src/lib.rs::create_user"
                && edge.target == "src/lib.rs::Repository"
                && edge.extra["relationship_role"] == "type_reference"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "src/lib.rs::create_user"
                && edge.target == "src/lib.rs::User"
                && edge.extra["evidence_kind"] == "rust_type_identifier"
        }));
    }

    #[test]
    fn test_parse_rust_traits_and_impl_for_attach_methods() {
        let source = br#"
pub trait Repository {
    fn find(&self);
}

pub struct Repo;

impl Repo {
    pub fn new() -> Self { Repo }
}

impl Repository for Repo {
    fn find(&self) {}
}

type UserId = u64;

fn boot() {
    let repo = Repo::new();
    repo.find();
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (nodes, edges) = parse_rust_with_parser("src/lib.rs", source, Some(&mut parser));

        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Repository"
                && node.extra["type_role"] == "trait"
                && node.extra["is_contract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Repo" && node.extra["type_role"] == "struct"
        }));
        assert!(!nodes
            .iter()
            .any(|node| { node.kind == "Class" && node.extra["type_role"] == "implementation" }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "new"
                && node.parent_name.as_deref() == Some("Repo")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "find"
                && node.parent_name.as_deref() == Some("Repository")
                && node.extra["is_abstract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "find"
                && node.parent_name.as_deref() == Some("Repo")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Type" && node.name == "UserId" && node.extra["type_role"] == "alias"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPLEMENTS"
                && edge.source == "src/lib.rs::Repo"
                && edge.target == "Repository"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/lib.rs::boot"
                && edge.target == "src/lib.rs::Repo.new"
        }));
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CALLS"
                    && edge.source == "src/lib.rs::boot"
                    && edge.target == "src/lib.rs::Repo.find"
            }),
            "{edges:?}"
        );
        assert!(!edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/lib.rs::boot"
                && edge.target == "src/lib.rs::Repository.find"
        }));
        assert!(!edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "src/lib.rs::boot" && edge.target == "Repo::new"
        }));
    }
}
