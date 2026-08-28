use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, is_test_file, line_count, node_text, set_declared_namespaces,
    strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_csharp_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File.as_str().to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "csharp".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            csharp_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            set_declared_namespaces(
                &mut nodes,
                collect_namespace_paths(
                    tree.root_node(),
                    source,
                    &["namespace_declaration", "file_scoped_namespace_declaration"],
                    Some("name"),
                    &[],
                ),
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn csharp_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "using_directive" => {
                csharp_emit_import(child, source, file_path, edges);
            }
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "struct_declaration" => {
                if let Some(name) = csharp_type_name(child, source) {
                    csharp_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    csharp_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "method_declaration" | "constructor_declaration" => {
                if let Some(name) = csharp_function_name(child, source) {
                    csharp_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    csharp_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "invocation_expression" | "object_creation_expression" => {
                csharp_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        csharp_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn csharp_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let text = node_text(node, source);
    let target = text
        .trim()
        .trim_start_matches("using")
        .trim()
        .trim_end_matches(';')
        .trim();
    if target.is_empty() {
        return;
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom
            .as_str()
            .to_string(),
        source: file_path.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn csharp_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = csharp_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
        if csharp_is_value_container(type_role) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class.as_str().to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "csharp".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: file_path.to_string(),
        target: qualify(file_path, name, enclosing_class),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn csharp_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    csharp_declared_name(node, source)
}

/// Reads the `name` field of a declaration, never a positional identifier.
///
/// `method_declaration` exposes its return type as a `returns` field that is
/// itself an `identifier` for user-defined types, so the first direct
/// `identifier` child is the return type rather than the declared name.
fn csharp_declared_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let text = node_text(csharp_generic_base(name), source)
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// Unwraps `generic_name` to the bare identifier, leaving other nodes as-is.
fn csharp_generic_base(node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    if node.kind() != "generic_name" {
        return node;
    }
    let mut cursor = node.walk();
    let base = node
        .children(&mut cursor)
        .find(|child| child.kind() == "identifier")
        .unwrap_or(node);
    base
}

fn csharp_type_role(node: tree_sitter::Node<'_>, source: &[u8]) -> (&'static str, bool, bool) {
    match node.kind() {
        "interface_declaration" => ("interface", true, true),
        "enum_declaration" => ("enum", false, false),
        "struct_declaration" => ("struct", false, false),
        _ => {
            let is_abstract = csharp_has_modifier(node, source, "abstract");
            if is_abstract {
                ("abstract_class", true, false)
            } else {
                ("class", false, false)
            }
        }
    }
}

fn csharp_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "struct" | "enum")
}

fn csharp_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function.as_str().to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "csharp".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: csharp_field_text(node, source, "parameters"),
        return_type: csharp_field_text(node, source, "returns"),
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn csharp_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    csharp_declared_name(node, source)
}

fn csharp_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = csharp_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls.as_str().to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = csharp_call_signature(node, source) {
        if let Some(edge) = csharp_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn csharp_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = csharp_callee(node)?;
    // `Factory.Create(...)` must resolve to `Create`, not to the receiver.
    let invoked = match callee.kind() {
        "member_access_expression" => callee.child_by_field_name("name")?,
        _ => callee,
    };
    let invoked = csharp_generic_base(invoked);
    matches!(invoked.kind(), "identifier").then(|| node_text(invoked, source))
}

fn csharp_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = csharp_callee(node)?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn csharp_callee(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let field = match node.kind() {
        "object_creation_expression" => "type",
        _ => "function",
    };
    node.child_by_field_name(field)
}

fn csharp_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Process.Start" | "System.Diagnostics.Process.Start" => ("invokes_binary", "subprocess"),
        "File.ReadAllText" | "File.ReadAllBytes" | "File.ReadAllLines" | "File.OpenRead" => {
            ("reads_file", "file_io")
        }
        "File.WriteAllText" | "File.WriteAllBytes" | "File.OpenWrite" | "File.Create" => {
            ("writes_file", "file_io")
        }
        "Assembly.LoadFile" | "NativeLibrary.Load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match csharp_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact
            .as_str()
            .to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "csharp",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn csharp_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "argument" {
            csharp_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if arg.kind() == "string_literal" {
            return Some(csharp_string_text(arg, source));
        }
        return None;
    }
    None
}

fn csharp_first_non_punctuation_child(
    node: tree_sitter::Node<'_>,
) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"));
    child
}

fn csharp_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_literal_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn csharp_field_text(node: tree_sitter::Node<'_>, source: &[u8], field: &str) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    let text = node_text(child, source).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Scans every `modifier` child, since declarations carry several of them.
fn csharp_has_modifier(node: tree_sitter::Node<'_>, source: &[u8], modifier: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .filter(|child| child.kind() == "modifier")
        .any(|child| node_text(child, source).trim() == modifier);
    found
}
