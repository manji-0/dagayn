use std::path::Path;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    is_test_file, line_count, node_text, normalize_relative_path, strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_bash_with_parser(
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
        language: "bash".to_string(),
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
            let root = tree.root_node();
            bash_walk_children(
                root, source, file_path, repo_root, None, &mut nodes, &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn bash_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(name) = bash_function_name(child, source) {
                    let qualified = qualify(file_path, &name, None);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Function.as_str().to_string(),
                        name: name.clone(),
                        file_path: file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "bash".to_string(),
                        parent_name: None,
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({}),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
                        source: file_path.to_string(),
                        target: qualified,
                        file_path: file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    bash_walk_children(
                        child,
                        source,
                        file_path,
                        repo_root,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "command" => {
                bash_emit_command(child, source, file_path, repo_root, enclosing_func, edges);
            }
            _ => {}
        }
        bash_walk_children(
            child,
            source,
            file_path,
            repo_root,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn bash_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let name = node
        .children(&mut cursor)
        .find(|child| child.kind() == "word")
        .map(|child| node_text(child, source));
    name
}

fn bash_emit_command(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(command_name) = bash_command_name(node, source) else {
        return;
    };
    if matches!(command_name.as_str(), "source" | ".") {
        if let Some(target) = bash_first_command_arg(node, source) {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom
                    .as_str()
                    .to_string(),
                source: file_path.to_string(),
                target: resolve_bash_source_target(&target, file_path, repo_root).unwrap_or(target),
                file_path: file_path.to_string(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            });
        }
        return;
    }

    let caller = enclosing_func
        .map(|func| qualify(file_path, func, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls.as_str().to_string(),
        source: caller,
        target: command_name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn bash_command_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "command_name" {
            return Some(node_text(child, source).trim().to_string())
                .filter(|name| !name.is_empty());
        }
    }
    None
}

fn bash_first_command_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut seen_command = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "command_name" {
            seen_command = true;
            continue;
        }
        if seen_command && matches!(child.kind(), "word" | "string" | "raw_string") {
            let text = node_text(child, source);
            return Some(strip_matching_quotes(text.trim()).to_string())
                .filter(|arg| !arg.is_empty());
        }
    }
    None
}

fn resolve_bash_source_target(
    target: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    if let Some(repo_root) = repo_root {
        let candidate = repo_root.join(caller_dir).join(target);
        if candidate.is_file() {
            return candidate
                .strip_prefix(repo_root)
                .ok()
                .map(normalize_relative_path);
        }
        return None;
    }
    let candidate = caller_dir.join(target);
    candidate
        .is_file()
        .then(|| normalize_relative_path(&candidate))
}
