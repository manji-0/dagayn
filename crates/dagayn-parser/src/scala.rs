use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_scala_with_parser(
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
        language: "scala".to_string(),
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
            scala_walk_children(
                tree.root_node(),
                source,
                file_path,
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

fn scala_walk_children(
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
            "import_declaration" => {
                scala_emit_imports(child, source, file_path, edges);
            }
            "trait_definition" | "class_definition" | "object_definition" | "enum_definition" => {
                if let Some(name) = scala_direct_child_text(child, source, &["identifier"]) {
                    scala_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    scala_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" | "function_declaration" => {
                if let Some(name) = scala_direct_child_text(child, source, &["identifier"]) {
                    scala_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    scala_walk_children(
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
            "call_expression" => {
                scala_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "instance_expression" => {
                scala_emit_instance_call(
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
        scala_walk_children(
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

fn scala_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    for target in scala_import_targets(node, source) {
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn scala_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let text = node_text(node, source);
    let import = text.trim().trim_start_matches("import").trim();
    if let (Some(open), Some(close)) = (import.find('{'), import.rfind('}')) {
        let prefix = import[..open].trim_end_matches('.').trim();
        return import[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| format!("{prefix}.{}", scala_normalize_import_selector(item)))
            .collect();
    }
    vec![scala_normalize_import_selector(import)]
}

fn scala_normalize_import_selector(value: &str) -> String {
    value
        .strip_suffix("._")
        .map(|prefix| format!("{prefix}.*"))
        .unwrap_or_else(|| value.to_string())
}

fn scala_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = match node.kind() {
        "trait_definition" => ("trait", true, true),
        "enum_definition" => ("enum", false, false),
        _ if scala_is_case_class(node, source) => ("record", false, false),
        _ => ("class", false, false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
        if scala_is_value_container(type_role) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "scala".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if node.kind() == "class_definition" {
        for (idx, target) in scala_inheritance_targets(node, source)
            .into_iter()
            .enumerate()
        {
            edges.push(ParsedEdge {
                kind: if idx == 0 { "INHERITS" } else { "IMPLEMENTS" }.to_string(),
                source: qualified.clone(),
                target,
                file_path: file_path.to_string(),
                line: node.start_position().row as i64 + 1,
                extra: json!({
                    "relationship_role": if idx == 0 { "extends" } else { "implements" },
                    "syntax_source": "class_definition",
                }),
            });
        }
    }
}

fn scala_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "record" | "enum")
}

fn scala_is_case_class(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    let is_case = node
        .children(&mut cursor)
        .any(|child| node_text(child, source).trim() == "case");
    is_case
}

fn scala_emit_function(
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
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "scala".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: scala_direct_child_text(node, source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn scala_emit_call(
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
    if let Some(call_name) = scala_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = scala_call_signature(node, source) {
        if let Some(edge) = scala_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn scala_emit_instance_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = scala_first_descendant_text(node, source, &["type_identifier"]) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn scala_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = scala_call_callee(node)?;
    if callee.kind() == "identifier" {
        return Some(node_text(callee, source));
    }
    if callee.kind() == "generic_function" {
        if let Some(function) = scala_direct_child(callee, &["field_expression", "identifier"]) {
            return scala_last_descendant_text(
                function,
                source,
                &["identifier", "type_identifier"],
            )
            .or_else(|| Some(node_text(function, source)));
        }
    }
    scala_last_descendant_text(callee, source, &["identifier", "type_identifier"])
}

fn scala_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = scala_call_callee(node)?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn scala_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments");
    found
}

fn scala_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "scala.sys.process.Process" => {
            ("invokes_binary", "subprocess")
        }
        "System.loadLibrary" | "System.load" => ("loads_shared_library", "ffi"),
        "Files.readString" | "Files.readAllBytes" | "scala.io.Source.fromFile" => {
            ("reads_file", "file_io")
        }
        "Files.writeString" | "Files.write" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match scala_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "scala",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn scala_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = scala_direct_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(strip_matching_quotes(node_text(child, source).trim()).to_string());
        }
        return None;
    }
    None
}

fn scala_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(extends) = scala_direct_child(node, &["extends_clause"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = extends.walk();
    for child in extends.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => out.push(node_text(child, source)),
            "generic_type" => {
                if let Some(target) =
                    scala_first_descendant_text(child, source, &["type_identifier"])
                {
                    out.push(target);
                }
            }
            _ => {}
        }
    }
    out
}

fn scala_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn scala_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    scala_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn scala_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = scala_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn scala_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(value) = scala_last_descendant_text(child, source, kinds) {
            found = Some(value);
        }
    }
    found
}
