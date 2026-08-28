use std::path::Path;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    is_test_file, line_count, node_text, normalize_relative_path, strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

struct JavaParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    repo_root: Option<&'a Path>,
}

pub(super) fn parse_java_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File.as_str().to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "java".to_string(),
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
            let context = JavaParseContext {
                source,
                file_path,
                repo_root,
            };
            java_walk_children(
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

fn java_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                java_emit_import(
                    child,
                    context.source,
                    context.file_path,
                    context.repo_root,
                    edges,
                );
            }
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration" => {
                if let Some(name) = java_type_name(child, context.source) {
                    java_emit_type(
                        child,
                        context.source,
                        context.file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    java_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "method_declaration" | "constructor_declaration" => {
                if let Some(name) = java_function_name(child, context.source) {
                    java_emit_function(
                        child,
                        context.source,
                        context.file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    java_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "method_invocation" => {
                java_emit_call(
                    child,
                    context.source,
                    context.file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        java_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn java_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(import_target) = java_import_target(node, source) else {
        return;
    };
    let target =
        resolve_java_import_target(&import_target, file_path, repo_root).unwrap_or(import_target);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom
            .as_str()
            .to_string(),
        source: file_path.to_string(),
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn java_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let target = text
        .trim()
        .trim_start_matches("import")
        .trim()
        .trim_start_matches("static")
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();
    (!target.is_empty()).then_some(target)
}

fn resolve_java_import_target(
    target: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    if target.ends_with(".*") {
        return None;
    }
    java_resolve_module_to_file(target, file_path, repo_root).or_else(|| {
        target
            .rfind('.')
            .and_then(|dot| java_resolve_module_to_file(&target[..dot], file_path, repo_root))
    })
}

fn java_resolve_module_to_file(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    let relative = module.replace('.', "/") + ".java";
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    if let Some(repo_root) = repo_root {
        let mut current = repo_root.join(caller_dir);
        loop {
            let candidate = current.join(&relative);
            if candidate.is_file() {
                return candidate
                    .strip_prefix(repo_root)
                    .ok()
                    .map(normalize_relative_path);
            }
            if !current.pop() {
                break;
            }
        }
        return None;
    }

    let mut current = caller_dir.to_path_buf();
    loop {
        let candidate = current.join(&relative);
        if candidate.is_file() {
            return Some(normalize_relative_path(&candidate));
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn java_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = java_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
        if java_is_value_container(type_role) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class.as_str().to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "java".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: enclosing_class
            .map(|parent| qualify(file_path, parent, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for (base, role) in java_bases(node, source) {
        edges.push(ParsedEdge {
            kind: if role == "implements" {
                "IMPLEMENTS".to_string()
            } else {
                "INHERITS".to_string()
            },
            source: qualified.clone(),
            target: base,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": role,
                "syntax_source": node.kind(),
            }),
        });
    }
}

fn java_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_direct_child_text(node, source, &["identifier", "type_identifier"])
}

fn java_type_role(node: tree_sitter::Node<'_>, source: &[u8]) -> (&'static str, bool, bool) {
    if node.kind() == "interface_declaration" {
        return ("interface", true, true);
    }
    if node.kind() == "enum_declaration" {
        return ("enum", false, false);
    }
    if node.kind() == "record_declaration" {
        return ("record", false, false);
    }
    let is_abstract = java_direct_child_text(node, source, &["modifiers"])
        .is_some_and(|mods| mods.split_whitespace().any(|part| part == "abstract"));
    if is_abstract {
        ("abstract_class", true, false)
    } else {
        ("class", false, false)
    }
}

fn java_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "record" | "enum")
}

fn java_bases(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<(String, &'static str)> {
    let mut bases = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "superclass" => java_collect_type_names(child, source, "extends", &mut bases),
            "super_interfaces" => {
                java_collect_type_names(child, source, "implements", &mut bases);
            }
            "extends_interfaces" => java_collect_type_names(child, source, "extends", &mut bases),
            _ => {}
        }
    }
    bases
}

fn java_collect_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    role: &'static str,
    bases: &mut Vec<(String, &'static str)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "type_identifier" | "generic_type") {
            bases.push((node_text(child, source), role));
        } else {
            java_collect_type_names(child, source, role, bases);
        }
    }
}

fn java_emit_function(
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
        language: "java".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: java_direct_child_text(node, source, &["formal_parameters"]),
        return_type: None,
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

fn java_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_field_text(node, source, "name")
        .or_else(|| java_direct_child_text(node, source, &["identifier"]))
}

fn java_field_text(node: tree_sitter::Node<'_>, source: &[u8], field: &str) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    let text = node_text(child, source).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn java_emit_call(
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

    if let Some(call_name) = java_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls.as_str().to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }

    if let Some(signature) = java_call_signature(node, source) {
        if let Some(edge) = java_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

/// Reads the invoked method from the `name` field.
///
/// `method_invocation` puts the receiver first, so taking the first
/// non-`argument_list` child made `Broker.build(t)` point at `Broker` — the
/// class — instead of `build`, losing every qualified call.
fn java_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_field_text(node, source, "name")
}

fn java_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            break;
        }
        parts.push(node_text(child, source));
    }
    let signature = parts.join("").trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn java_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "Runtime.exec" => ("invokes_binary", "subprocess"),
        "System.loadLibrary"
        | "System.load"
        | "Runtime.getRuntime().loadLibrary"
        | "Runtime.getRuntime().load" => ("loads_shared_library", "ffi"),
        "Files.readString" | "Files.readAllBytes" => ("reads_file", "file_io"),
        "Files.writeString" | "Files.write" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match java_first_string_arg(node, source) {
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
            "source_language": "java",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn java_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string_literal" {
            return Some(java_string_text(child, source));
        }
        return None;
    }
    None
}

fn java_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn java_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}
