use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, is_test_file, line_count, node_text, set_declared_namespaces,
    strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_php_with_parser(
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
        language: "php".to_string(),
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
            php_walk_children(
                tree.root_node(),
                source,
                &file_path,
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
                    &["namespace_definition"],
                    Some("name"),
                    &["namespace_name"],
                ),
            );
            let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn php_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "namespace_use_declaration" => {
                php_emit_import(child, source, file_path, edges);
            }
            "class_declaration" | "interface_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_type(child, file_path, &name, enclosing_class, nodes, edges);
                    php_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" | "method_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    php_walk_children(
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
            "function_call_expression"
            | "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression" => {
                php_emit_call(
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
        php_walk_children(
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

fn php_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    for target in php_import_targets(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: file_path.to_string(),
            target,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

/// The imported symbol paths of a `use` declaration.
///
/// The whole statement used to be the target (`use Exception;`), which no
/// namespace or file index could ever match. Group form
/// (`use App\Util\{One, Two};`) expands to one target per clause, and an
/// `as` alias is dropped.
fn php_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let prefix = php_direct_child_text(node, source, &["namespace_name"]);
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "namespace_use_clause" => {
                if let Some(target) = php_use_clause_target(child, source, prefix.as_deref()) {
                    targets.push(target);
                }
            }
            "namespace_use_group" => {
                let mut group = child.walk();
                for clause in child.children(&mut group) {
                    if clause.kind() != "namespace_use_clause" {
                        continue;
                    }
                    if let Some(target) = php_use_clause_target(clause, source, prefix.as_deref()) {
                        targets.push(target);
                    }
                }
            }
            _ => {}
        }
    }
    targets
}

fn php_use_clause_target(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    prefix: Option<&str>,
) -> Option<String> {
    let path = php_direct_child_text(node, source, &["qualified_name", "name"])?;
    let path = path.trim().trim_start_matches('\\');
    if path.is_empty() {
        return None;
    }
    Some(match prefix {
        Some(prefix) => format!("{}\\{path}", prefix.trim().trim_start_matches('\\')),
        None => path.to_string(),
    })
}

fn php_emit_type(
    node: tree_sitter::Node<'_>,
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = if node.kind() == "interface_declaration" {
        ("interface", true, true)
    } else {
        ("class", false, false)
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: file_path.to_string(),
        target: qualify(file_path, name, enclosing_class),
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: php_direct_child_text(node, source, &["formal_parameters"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(signature) = php_call_signature(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = php_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target: call_name,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(edge) = php_bridge_edge(node, source, file_path, &caller, &signature) {
        edges.push(edge);
    }
}

/// The invoked symbol, used as the CALLS target.
///
/// `Broker::build()` resolves to `build`. Emitting `Broker::build` made the
/// target look already-qualified to bare-name resolution, which only ever
/// matches `file::Class.method`, so the edge could never bind to a node.
fn php_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "scoped_call_expression" => php_direct_child_texts(node, source, &["name"]).pop(),
        _ => php_call_signature(node, source),
    }
}

/// The call as written, used to match cross-artifact bridges (`FFI::cdef`).
fn php_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_call_expression" => {
            php_direct_child_text(node, source, &["name", "qualified_name"])
                .map(|name| name.trim_start_matches('\\').to_string())
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            php_last_direct_child_text(node, source, "name")
        }
        "scoped_call_expression" => {
            let names = php_direct_child_texts(node, source, &["name"]);
            if names.len() >= 2 {
                return Some(format!("{}::{}", names[0], names[1]));
            }
            if let Some(scope) = php_direct_child_text(node, source, &["relative_scope"]) {
                if matches!(scope.as_str(), "parent" | "self") {
                    return names.last().cloned();
                }
            }
            names.last().cloned()
        }
        _ => None,
    }
}

fn php_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "exec" | "shell_exec" | "system" | "passthru" | "proc_open" | "popen" => {
            ("invokes_binary", "subprocess")
        }
        "file_get_contents" | "fread" | "readfile" => ("reads_file", "file_io"),
        "file_put_contents" | "fwrite" => ("writes_file", "file_io"),
        "fopen" => ("opens_file", "file_io"),
        "FFI::cdef" | "FFI::load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match php_first_string_arg(node, source) {
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
            "source_language": "php",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn php_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "argument" {
            php_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if matches!(arg.kind(), "encapsed_string" | "string") {
            return Some(php_string_text(arg, source));
        }
        return None;
    }
    None
}

fn php_first_non_punctuation_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"));
    child
}

fn php_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn php_direct_child_text(
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

fn php_last_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            found = Some(node_text(child, source));
        }
    }
    found
}

fn php_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}
