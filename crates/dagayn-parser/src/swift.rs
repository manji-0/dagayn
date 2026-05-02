use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{is_test_function, qualify, resolve_rust_call_targets};

pub(super) fn parse_swift_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "swift".to_string(),
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
            let context = SwiftParseContext { source, file_path };
            swift_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }
    (nodes, edges)
}

struct SwiftParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn swift_walk_children(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" if enclosing_class.is_none() && enclosing_func.is_none() => {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: context.file_path.to_string(),
                    target: node_text(child, context.source).trim().to_string(),
                    file_path: context.file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                continue;
            }
            "class_declaration" | "protocol_declaration" => {
                if let Some(name) = swift_type_name(child, context.source) {
                    swift_emit_class(child, context, &name, nodes, edges);
                    swift_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_declaration" => {
                if let Some(name) = swift_function_name(child, context.source) {
                    swift_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    swift_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                swift_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        swift_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn swift_emit_class(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let swift_kind = swift_type_kind(node, context.source);
    let (type_role, extra_flags) = match swift_kind.as_str() {
        "protocol" => (
            "protocol",
            json!({"is_abstract": true, "is_contract": true}),
        ),
        "struct" => ("struct", json!({})),
        "enum" => ("enum", json!({})),
        _ => ("class", json!({})),
    };
    let mut extra = json!({"type_role": type_role, "swift_kind": swift_kind});
    if let (Some(extra_obj), Some(flags)) = (extra.as_object_mut(), extra_flags.as_object()) {
        for (key, value) in flags {
            extra_obj.insert(key.clone(), value.clone());
        }
    }
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "swift".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualify(context.file_path, name, None),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for base in swift_inheritance_targets(node, context.source) {
        edges.push(ParsedEdge {
            kind: "INHERITS".to_string(),
            source: qualify(context.file_path, name, None),
            target: base,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_declaration",
            }),
        });
    }
}

fn swift_emit_function(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "swift".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualify(context.file_path, name, enclosing_class),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn swift_emit_call(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    if let Some(call_name) = swift_call_name(node, context.source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = swift_call_signature(node, context.source) {
        if let Some(edge) = swift_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn swift_type_kind(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if node.kind() == "protocol_declaration" {
        return "protocol".to_string();
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = node_text(child, source);
        if matches!(
            text.as_str(),
            "class" | "struct" | "enum" | "actor" | "extension"
        ) {
            return text;
        }
    }
    "class".to_string()
}

fn swift_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    swift_direct_child(node, &["type_identifier"])
        .map(|child| node_text(child, source))
        .or_else(|| swift_first_descendant_text(node, source, &["type_identifier"]))
}

fn swift_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    swift_direct_child(node, &["simple_identifier"]).map(|child| node_text(child, source))
}

fn swift_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(specifier) = swift_direct_child(node, &["inheritance_specifier"]) else {
        return Vec::new();
    };
    swift_descendant_texts(specifier, source, &["type_identifier"])
}

fn swift_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = swift_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(callee, source)),
        "navigation_expression" => {
            swift_last_descendant_text(callee, source, &["simple_identifier"])
        }
        _ => None,
    }
}

fn swift_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = swift_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(callee, source)),
        "navigation_expression" => {
            let parts = swift_descendant_texts(callee, source, &["simple_identifier"]);
            (!parts.is_empty()).then(|| parts.join("."))
        }
        _ => None,
    }
}

fn swift_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "call_suffix");
    found
}

fn swift_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Process.run" => ("invokes_binary", "subprocess"),
        "String.contentsOf" | "Data.contentsOf" | "FileManager.contentsOfFile" => {
            ("reads_file", "file_io")
        }
        "FileManager.createFile" => ("writes_file", "file_io"),
        "dlopen" | "Bundle.load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match swift_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "swift",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn swift_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let args = swift_first_descendant(node, &["value_arguments"])?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() != "value_argument" {
            continue;
        }
        let mut value_cursor = child.walk();
        for value in child.children(&mut value_cursor) {
            if value.kind() == "line_string_literal" {
                return Some(swift_string_text(value, source));
            }
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn swift_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = node_text(node, source);
    strip_matching_quotes(text.trim()).to_string()
}

fn swift_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn swift_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = swift_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn swift_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    swift_first_descendant(node, kinds).map(|child| node_text(child, source))
}

fn swift_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    swift_collect_descendant_texts(node, source, kinds, &mut out);
    out
}

fn swift_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut out = Vec::new();
    swift_collect_descendant_texts(node, source, kinds, &mut out);
    out.pop()
}

fn swift_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    out: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
        swift_collect_descendant_texts(child, source, kinds, out);
    }
}
