use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::rescript_legacy;
use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    contains_ascii_ignore_case, ends_with_ascii_ignore_case, is_test_file, line_count, node_text,
    starts_with_ascii_ignore_case,
};
use super::{add_tested_by_edges, qualify, resolve_rust_call_targets};

pub(super) fn parse_rescript_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let Some(parser) = parser else {
        return rescript_legacy::parse_rescript(file_path, source);
    };
    let Some(tree) = parser.parse(source, None) else {
        return rescript_legacy::parse_rescript(file_path, source);
    };
    let root = tree.root_node();
    if root.has_error() {
        return rescript_legacy::parse_rescript(file_path, source);
    }

    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: line_count(source),
        language: "rescript".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: rescript_is_test_file(file_path),
        extra: if ends_with_ascii_ignore_case(file_path, ".resi") {
            json!({"rescript_interface": true})
        } else {
            json!({})
        },
    }];
    let mut edges = Vec::new();
    let mut context = RescriptContext {
        source,
        file_path,
        test_file: rescript_is_test_file(file_path),
        modules: HashMap::new(),
    };
    rescript_collect_modules(root, &mut context, None);
    rescript_walk_children(root, &mut context, None, None, &mut nodes, &mut edges);
    tag_rescript_js_binding_modules(&mut nodes);
    let mut edges = resolve_rust_call_targets(&nodes, edges, file_path);
    if context.test_file {
        add_tested_by_edges(&nodes, &mut edges);
    }
    (nodes, dedupe_rescript_imports(edges))
}

struct RescriptContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    test_file: bool,
    modules: HashMap<usize, String>,
}

fn rescript_collect_modules(
    node: tree_sitter::Node<'_>,
    context: &mut RescriptContext<'_>,
    parent: Option<&str>,
) {
    if node.kind() == "module_binding" {
        if let Some(name) = rescript_node_name(node, context.source, "name") {
            let qualified_parent = parent.map(str::to_string);
            let full_name = qualified_parent
                .as_deref()
                .map(|parent| format!("{parent}.{name}"))
                .unwrap_or_else(|| name.clone());
            context.modules.insert(node.id(), full_name.clone());
            if let Some(definition) = node.child_by_field_name("definition") {
                rescript_collect_modules(definition, context, Some(&full_name));
            }
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        rescript_collect_modules(child, context, parent);
    }
}

fn rescript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &mut RescriptContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_let: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "module_binding" => {
                if let Some(name) = rescript_node_name(child, context.source, "name") {
                    let full_name =
                        context
                            .modules
                            .get(&child.id())
                            .cloned()
                            .unwrap_or_else(|| {
                                enclosing_module
                                    .map(|module| format!("{module}.{name}"))
                                    .unwrap_or_else(|| name.clone())
                            });
                    let parent = full_name.rsplit_once('.').map(|(parent, _)| parent);
                    push_rescript_node(
                        context.file_path,
                        nodes,
                        edges,
                        RescriptNodeSpec {
                            kind: "Class",
                            name: &name,
                            parent,
                            line_start: child.start_position().row as i64 + 1,
                            line_end: child.end_position().row as i64 + 1,
                            is_test: false,
                            extra: json!({"rescript_kind": "module"}),
                        },
                    );
                    if let Some(definition) = child.child_by_field_name("definition") {
                        rescript_walk_children(
                            definition,
                            context,
                            Some(&full_name),
                            None,
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "let_binding" => {
                if let Some(name) = rescript_let_name(child, context.source) {
                    if enclosing_let.is_some() {
                        if let Some(body) = child.child_by_field_name("body") {
                            if body.kind() == "call_expression" {
                                emit_rescript_call(
                                    body,
                                    context,
                                    enclosing_module,
                                    enclosing_let,
                                    edges,
                                );
                            }
                            rescript_walk_children(
                                body,
                                context,
                                enclosing_module,
                                enclosing_let,
                                nodes,
                                edges,
                            );
                        }
                        continue;
                    }
                    let is_test = rescript_is_test_function(&name, context.file_path);
                    push_rescript_node(
                        context.file_path,
                        nodes,
                        edges,
                        RescriptNodeSpec {
                            kind: if is_test { "Test" } else { "Function" },
                            name: &name,
                            parent: enclosing_module,
                            line_start: child.start_position().row as i64 + 1,
                            line_end: child.end_position().row as i64 + 1,
                            is_test,
                            extra: json!({}),
                        },
                    );
                    if let Some(body) = child.child_by_field_name("body") {
                        rescript_walk_children(
                            body,
                            context,
                            enclosing_module,
                            Some(&name),
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "external_declaration" => {
                if let Some(name) =
                    rescript_first_child_text(child, context.source, "value_identifier")
                {
                    push_rescript_node(
                        context.file_path,
                        nodes,
                        edges,
                        RescriptNodeSpec {
                            kind: "Function",
                            name: &name,
                            parent: enclosing_module,
                            line_start: child.start_position().row as i64 + 1,
                            line_end: child.end_position().row as i64 + 1,
                            is_test: false,
                            extra: json!({"rescript_external": true}),
                        },
                    );
                    continue;
                }
            }
            "type_binding" => {
                if let Some(name) = rescript_node_name(child, context.source, "name") {
                    push_rescript_node(
                        context.file_path,
                        nodes,
                        edges,
                        RescriptNodeSpec {
                            kind: "Type",
                            name: &name,
                            parent: enclosing_module,
                            line_start: child.start_position().row as i64 + 1,
                            line_end: child.end_position().row as i64 + 1,
                            is_test: false,
                            extra: json!({}),
                        },
                    );
                    continue;
                }
            }
            "open_statement" | "include_statement" => {
                if let Some(target) = rescript_module_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({"rescript_import_kind": child.kind().trim_end_matches("_statement")}),
                    });
                }
                continue;
            }
            "call_expression" => {
                emit_rescript_call(child, context, enclosing_module, enclosing_let, edges);
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                if let Some(target) = rescript_node_name(child, context.source, "name") {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target: target.clone(),
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({"rescript_import_kind": "jsx"}),
                    });
                    if let Some(caller) =
                        enclosing_let.map(|name| qualify(context.file_path, name, enclosing_module))
                    {
                        edges.push(ParsedEdge {
                            kind: "CALLS".to_string(),
                            source: caller,
                            target,
                            file_path: context.file_path.to_string(),
                            line: child.start_position().row as i64 + 1,
                            extra: json!({"rescript_call_kind": "jsx"}),
                        });
                    }
                }
            }
            _ => {}
        }
        rescript_walk_children(
            child,
            context,
            enclosing_module,
            enclosing_let,
            nodes,
            edges,
        );
    }
}

fn emit_rescript_call(
    node: tree_sitter::Node<'_>,
    context: &RescriptContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_let: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let (Some(caller), Some(target)) = (
        enclosing_let.map(|name| qualify(context.file_path, name, enclosing_module)),
        rescript_call_target(node, context.source),
    ) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller,
            target,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

struct RescriptNodeSpec<'a> {
    kind: &'a str,
    name: &'a str,
    parent: Option<&'a str>,
    line_start: i64,
    line_end: i64,
    is_test: bool,
    extra: serde_json::Value,
}

fn push_rescript_node(
    file_path: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
    spec: RescriptNodeSpec<'_>,
) {
    nodes.push(ParsedNode {
        kind: spec.kind.to_string(),
        name: spec.name.to_string(),
        file_path: file_path.to_string(),
        line_start: spec.line_start,
        line_end: spec.line_end,
        language: "rescript".to_string(),
        parent_name: spec.parent.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: spec.is_test,
        extra: spec.extra,
    });
    let source = spec
        .parent
        .map(|parent| qualify(file_path, parent, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source,
        target: qualify(file_path, spec.name, spec.parent),
        file_path: file_path.to_string(),
        line: spec.line_start,
        extra: json!({}),
    });
}

fn rescript_node_name(node: tree_sitter::Node<'_>, source: &[u8], field: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|child| node_text(child, source).replace(' ', ""))
}

fn rescript_let_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let pattern = node.child_by_field_name("pattern")?;
    match pattern.kind() {
        "value_identifier" => Some(node_text(pattern, source)),
        _ => rescript_first_child_text(pattern, source, "value_identifier"),
    }
}

fn rescript_first_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source).replace(' ', ""));
        }
        if let Some(found) = rescript_first_child_text(child, source, kind) {
            return Some(found);
        }
    }
    None
}

fn rescript_module_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    rescript_first_child_text(node, source, "module_identifier_path")
        .or_else(|| rescript_first_child_text(node, source, "module_identifier"))
}

fn rescript_call_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let function = node.child_by_field_name("function")?;
    let target = node_text(function, source).replace(' ', "");
    if target.is_empty() || rescript_is_keyword(&target) {
        None
    } else {
        Some(target)
    }
}

fn tag_rescript_js_binding_modules(nodes: &mut [ParsedNode]) {
    let mut externals_by_parent: HashMap<String, usize> = HashMap::new();
    let mut functions_by_parent: HashMap<String, usize> = HashMap::new();
    for node in nodes.iter() {
        if node.kind == "Function" {
            if let Some(parent) = node.parent_name.as_deref() {
                *functions_by_parent.entry(parent.to_string()).or_default() += 1;
                if node
                    .extra
                    .get("rescript_external")
                    .and_then(|value| value.as_bool())
                    == Some(true)
                {
                    *externals_by_parent.entry(parent.to_string()).or_default() += 1;
                }
            }
        }
    }
    for node in nodes.iter_mut() {
        if node.kind != "Class" {
            continue;
        }
        let Some(total) = functions_by_parent.get(&node.name) else {
            continue;
        };
        if externals_by_parent.get(&node.name) == Some(total) {
            node.extra = json!({"rescript_kind": "js_binding"});
        }
    }
}

fn dedupe_rescript_imports(edges: Vec<ParsedEdge>) -> Vec<ParsedEdge> {
    let mut seen = HashSet::new();
    edges
        .into_iter()
        .filter(|edge| {
            if edge.kind != "IMPORTS_FROM" {
                return true;
            }
            seen.insert((
                edge.source.clone(),
                edge.target.clone(),
                edge.extra
                    .get("rescript_import_kind")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
            ))
        })
        .collect()
}

fn rescript_is_keyword(name: &str) -> bool {
    matches!(
        name,
        "and"
            | "as"
            | "assert"
            | "await"
            | "catch"
            | "constraint"
            | "else"
            | "exception"
            | "external"
            | "false"
            | "for"
            | "if"
            | "include"
            | "let"
            | "module"
            | "open"
            | "rec"
            | "switch"
            | "true"
            | "try"
            | "type"
            | "when"
            | "while"
    )
}

fn rescript_is_test_file(file_path: &str) -> bool {
    is_test_file(file_path)
        || ends_with_ascii_ignore_case(file_path, "_test.res")
        || ends_with_ascii_ignore_case(file_path, "_spec.res")
        || contains_ascii_ignore_case(file_path, "/__tests__/")
}

fn rescript_is_test_function(name: &str, file_path: &str) -> bool {
    starts_with_ascii_ignore_case(name, "test")
        || name.ends_with("_test")
        || name.ends_with("_spec")
        || (rescript_is_test_file(file_path)
            && matches!(name, "describe" | "it" | "test" | "expect"))
}
