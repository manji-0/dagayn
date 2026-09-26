use std::collections::HashMap;

use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_perl_with_parser(
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
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = PerlParseContext {
        source,
        file_path: file_path.clone(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        perl_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        let mut edges = resolve_perl_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct PerlParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
}

/// Walks `node`, tracking the current package. A `package X;` statement
/// switches the package for the rest of its enclosing block, while
/// `package X { ... }` scopes it to the block. `main` is the default package
/// and is left unqualified.
fn perl_walk_children(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut current: Option<String> = package.map(str::to_string);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let package = current.as_deref();
        match child.kind() {
            "use_statement" if enclosing_func.is_none() => {
                perl_emit_use(child, context, package, edges);
                continue;
            }
            "require_expression" if enclosing_func.is_none() => {
                if let Some(target) =
                    perl_direct_child_text(child, context.source, &["bareword", "package"])
                {
                    perl_push_import(child, context, target, edges);
                }
                continue;
            }
            "package_statement" | "class_statement" | "role_statement" => {
                if let Some(name) = perl_package_name(child, context.source) {
                    let scoped = (name != "main").then_some(name.as_str());
                    if let Some(name) = scoped {
                        perl_emit_class(child, context, name, nodes, edges);
                    }
                    if let Some(block) = perl_direct_child(child, &["block"]) {
                        perl_walk_children(block, context, scoped, enclosing_func, nodes, edges);
                    } else {
                        current = scoped.map(str::to_string);
                    }
                }
                continue;
            }
            "subroutine_declaration_statement" | "method_declaration_statement" => {
                if let Some(name) = perl_subroutine_name(child, context.source) {
                    perl_emit_function(child, context, &name, package, nodes, edges);
                    let scope = match package {
                        Some(package) => format!("{package}.{name}"),
                        None => name.clone(),
                    };
                    perl_walk_children(child, context, package, Some(&scope), nodes, edges);
                }
                continue;
            }
            "assignment_expression" if enclosing_func.is_none() => {
                perl_emit_isa_assignment(child, context, package, edges);
            }
            "function_call_expression"
            | "ambiguous_function_call_expression"
            | "method_call_expression"
            | "anonymous_function_call_expression" => {
                if let Some(call_name) = perl_call_name(child, context.source) {
                    perl_emit_call(child, context, &call_name, enclosing_func, edges);
                }
            }
            _ => {}
        }
        perl_walk_children(child, context, package, enclosing_func, nodes, edges);
    }
}

const PERL_PRAGMAS: &[&str] = &[
    "strict",
    "warnings",
    "utf8",
    "feature",
    "lib",
    "constant",
    "vars",
    "integer",
    "overload",
    "version",
    "experimental",
    "diagnostics",
    "bytes",
    "locale",
    "open",
];

fn perl_push_import(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    target: String,
    edges: &mut Vec<ParsedEdge>,
) {
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: context.file_path.to_string(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

/// `use Module ...` imports the module; `use parent`/`use base` declare
/// superclasses of the current package instead.
fn perl_emit_use(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(module) = perl_direct_child_text(node, context.source, &["package"]) else {
        return;
    };
    if matches!(module.as_str(), "parent" | "base") {
        let bases = perl_string_values(node, context.source);
        perl_emit_inherits(node, context, package, bases, module.as_str(), edges);
        return;
    }
    if PERL_PRAGMAS.contains(&module.as_str()) {
        return;
    }
    perl_push_import(node, context, module, edges);
}

fn perl_emit_isa_assignment(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(left) = node.child(0) else {
        return;
    };
    if perl_first_descendant_text(left, context.source, &["varname"]).as_deref() != Some("ISA") {
        return;
    }
    let bases = perl_string_values(node, context.source);
    perl_emit_inherits(node, context, package, bases, "@ISA", edges);
}

fn perl_emit_inherits(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    bases: Vec<String>,
    evidence: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(package) = package else {
        return;
    };
    for base in bases {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualify(&context.file_path, package, None),
            target: base,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({"relationship_role": "extends", "syntax_source": evidence}),
        });
    }
}

/// Every string literal or `qw(...)` word under `node`.
fn perl_string_values(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut values = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind() {
            "string_literal" | "interpolated_string_literal" => {
                values.extend(perl_string_text(current, source));
            }
            "quoted_word_list" => {
                let text = perl_first_descendant_text(current, source, &["string_content"])
                    .unwrap_or_default();
                values.extend(text.split_whitespace().map(str::to_string));
            }
            _ => {
                let mut cursor = current.walk();
                let children: Vec<_> = current.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev());
            }
        }
    }
    values
}

fn perl_emit_class(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn perl_emit_function(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    package: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, package);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: package.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: package
            .map(|package| qualify(&context.file_path, package, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn perl_emit_call(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    call_name: &str,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = perl_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn perl_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match call_name {
        "system" | "exec" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "File::Slurp::read_file" => ("reads_file", "file_io"),
        "File::Slurp::write_file" => ("writes_file", "file_io"),
        "DynaLoader::dl_load_file" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match perl_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{call_name}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": call_name,
            "source_language": "perl",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn perl_package_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == "package")
        .map(|child| node_text(child, source))
}

fn perl_subroutine_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    perl_direct_child_text(node, source, &["bareword", "identifier"])
}

fn perl_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "method_call_expression" {
        let method = perl_direct_child_text(node, source, &["method"])?;
        // `Class->method` names its package; `$obj->method` does not.
        return Some(match perl_direct_child_text(node, source, &["bareword"]) {
            Some(class) => format!("{class}::{method}"),
            None => method,
        });
    }
    perl_direct_child_text(node, source, &["function", "bareword", "identifier"])
}

fn perl_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut skipped_callee = false;
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function" | "method") && !skipped_callee {
            skipped_callee = true;
            continue;
        }
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpolated_string_literal" | "string_literal" | "quoted_word_list"
        ) {
            return perl_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn perl_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    perl_first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn perl_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn perl_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    perl_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn perl_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = perl_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_perl_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    // `Pkg::name` -> qualified node, plus bare names grouped by package.
    let mut by_path = HashMap::<String, String>::new();
    let mut by_name = HashMap::<String, Vec<(Option<String>, String)>>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        if let Some(package) = node.parent_name.as_deref() {
            by_path
                .entry(format!("{package}::{}", node.name))
                .or_insert_with(|| qualified.clone());
        }
        by_name
            .entry(node.name.clone())
            .or_default()
            .push((node.parent_name.clone(), qualified));
    }
    let prefix = format!("{file_path}::");
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind != "CALLS" {
                return edge;
            }
            if let Some(target) = by_path.get(&edge.target) {
                edge.target = target.clone();
            } else if !edge.target.contains("::")
                && let Some(candidates) = by_name.get(&edge.target)
            {
                let caller_package = edge
                    .source
                    .strip_prefix(&prefix)
                    .and_then(|rest| rest.rsplit_once('.').map(|(package, _)| package));
                let chosen = candidates
                    .iter()
                    .find(|(package, _)| package.as_deref() == caller_package)
                    .unwrap_or(&candidates[0]);
                edge.target = chosen.1.clone();
            }
            edge
        })
        .collect()
}
