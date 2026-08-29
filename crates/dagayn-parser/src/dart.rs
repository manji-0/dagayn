use std::path::Path;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    import_candidate_exists, is_test_file, line_count, node_text, resolve_import_path,
    strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_dart_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "dart".to_string(),
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
            dart_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            dart_resolve_import_targets(&mut edges, file_path, repo_root);
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

/// Rewrites import targets that name a file in this repository.
///
/// A Dart import is a URI, so the literal (`../util.dart`,
/// `package:myapp/util.dart`) matches no file in the graph. `dart:` URIs and
/// third-party packages have no file here and keep their literal form.
fn dart_resolve_import_targets(
    edges: &mut [ParsedEdge],
    file_path: &str,
    repo_root: Option<&Path>,
) {
    for edge in edges
        .iter_mut()
        .filter(|edge| edge.kind == crate::core::types::EdgeKind::ImportsFrom)
    {
        if let Some(resolved) = dart_resolve_import(&edge.target, file_path, repo_root) {
            edge.target = resolved;
        }
    }
}

fn dart_resolve_import(literal: &str, file_path: &str, repo_root: Option<&Path>) -> Option<String> {
    if let Some(rest) = literal.strip_prefix("package:") {
        let (package, path) = rest.split_once('/')?;
        // Only this repository's own package maps to a path here.
        let package_root = dart_package_root(package, file_path, repo_root)?;
        let candidate = format!("{package_root}lib/{path}");
        return import_candidate_exists(Path::new(&candidate), repo_root).then_some(candidate);
    }
    if literal.starts_with("dart:") || literal.contains(':') {
        return None;
    }
    resolve_import_path(literal, file_path, repo_root, &[], false)
}

/// The nearest ancestor directory whose `pubspec.yaml` declares *package*,
/// as a prefix ending in `/` (empty at the repository root).
fn dart_package_root(package: &str, file_path: &str, repo_root: Option<&Path>) -> Option<String> {
    let mut current = Path::new(file_path).parent()?.to_path_buf();
    loop {
        let pubspec = current.join("pubspec.yaml");
        let full = repo_root
            .map(|root| root.join(&pubspec))
            .unwrap_or_else(|| pubspec.clone());
        if let Ok(text) = std::fs::read_to_string(&full) {
            if dart_pubspec_name(&text).as_deref() == Some(package) {
                let prefix = current.to_string_lossy().replace('\\', "/");
                return Some(if prefix.is_empty() {
                    String::new()
                } else {
                    format!("{prefix}/")
                });
            }
        }
        if !current.pop() {
            return None;
        }
    }
}

fn dart_pubspec_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("name:"))
        .map(|name| name.trim().trim_matches(['"', '\''].as_ref()).to_string())
}

fn dart_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    dart_emit_calls_from_children(
        node,
        source,
        file_path,
        enclosing_class,
        enclosing_func,
        edges,
    );

    // A Dart body is a *sibling* of its signature rather than a child, so the
    // signature's name has to carry across to the following `function_body`.
    // Without it every call in the body was attributed to the file.
    let mut pending_func: Option<(String, usize)> = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_or_export" => {
                dart_emit_import(child, source, file_path, edges);
            }
            "class_definition" | "mixin_declaration" | "enum_declaration" => {
                if let Some(name) = dart_direct_child_text(child, source, &["identifier"]) {
                    dart_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    dart_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_signature" | "method_signature" => {
                if let Some((signature, name)) = dart_signature_name(child, source) {
                    dart_emit_function(
                        signature,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    pending_func = Some((name, nodes.len() - 1));
                    continue;
                }
            }
            "function_body" => {
                let current = pending_func.take();
                if let Some((_, index)) = current.as_ref() {
                    // The node spanned the signature line only until now.
                    nodes[*index].line_end = child.end_position().row as i64 + 1;
                }
                let func = current.as_ref().map(|(name, _)| name.as_str());
                dart_walk_children(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    func.or(enclosing_func),
                    nodes,
                    edges,
                );
                continue;
            }
            _ => {}
        }
        pending_func = None;
        dart_walk_children(
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

/// Returns the `function_signature` node and its declared name.
///
/// Class members wrap the signature in a `method_signature`; getters, setters
/// and constructors have no `function_signature` and yield `None`.
fn dart_signature_name<'tree>(
    node: tree_sitter::Node<'tree>,
    source: &[u8],
) -> Option<(tree_sitter::Node<'tree>, String)> {
    let signature = if node.kind() == "function_signature" {
        node
    } else {
        dart_direct_child(node, &["function_signature"])?
    };
    let name = signature
        .child_by_field_name("name")
        .map(|name| node_text(name, source).trim().to_string())
        .or_else(|| dart_direct_child_text(signature, source, &["identifier"]))?;
    (!name.is_empty()).then_some((signature, name))
}

fn dart_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = dart_first_descendant_text(node, source, &["string_literal"]) else {
        return;
    };
    let target = strip_matching_quotes(target.trim()).to_string();
    if target.is_empty() {
        return;
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: file_path.to_string(),
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract) = match node.kind() {
        "mixin_declaration" => ("mixin", false),
        "enum_declaration" => ("enum", false),
        _ if dart_has_direct_child_kind(node, "abstract") => ("abstract_class", true),
        _ => ("class", false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if dart_is_value_container(type_role) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "dart".to_string(),
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
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for target in dart_inheritance_targets(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualified.clone(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_definition",
            }),
        });
    }
}

fn dart_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "enum")
}

fn dart_emit_function(
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
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "dart".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: dart_direct_child_text(node, source, &["formal_parameter_list"]),
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
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_calls_from_children(
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
    let mut call_name = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                call_name = Some(node_text(child, source));
            }
            "selector" => {
                if let Some(method_name) = dart_selector_method_name(child, source) {
                    call_name = Some(method_name);
                }
                if dart_selector_has_arguments(child) {
                    if let Some(target) = call_name.take() {
                        edges.push(ParsedEdge {
                            kind: crate::core::types::EdgeKind::Calls,
                            source: caller.clone(),
                            target,
                            file_path: file_path.to_string(),
                            line: node.start_position().row as i64 + 1,
                            extra: json!({}),
                        });
                    }
                }
            }
            "return" | "await" | "yield" | "this" | "const" | "new" => {}
            _ => {
                call_name = None;
            }
        }
    }
}

fn dart_selector_method_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "unconditional_assignable_selector" {
            return dart_first_descendant_text(child, source, &["identifier"]);
        }
    }
    None
}

fn dart_selector_has_arguments(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| child.kind() == "argument_part");
    found
}

fn dart_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "superclass" | "interfaces") {
            dart_collect_type_identifiers(child, source, &mut out);
        }
    }
    out
}

fn dart_collect_type_identifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" {
            out.push(node_text(child, source));
        } else {
            dart_collect_type_identifiers(child, source, out);
        }
    }
}

fn dart_has_direct_child_kind(node: tree_sitter::Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|child| child.kind() == kind);
    found
}

fn dart_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn dart_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    dart_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn dart_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = dart_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}
