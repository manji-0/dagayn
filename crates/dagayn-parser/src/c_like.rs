use std::collections::HashMap;
use std::path::Path;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    is_test_file, line_count, node_text, resolve_import_path, strip_matching_quotes,
};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_c_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "c", parser, repo_root)
}

pub(super) fn parse_cpp_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "cpp", parser, repo_root)
}

pub(super) fn parse_objc_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "objc", parser, repo_root)
}

fn parse_c_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
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
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = CParseContext {
        source,
        file_path,
        language,
        repo_root,
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            c_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_c_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct CParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    language: &'a str,
    repo_root: Option<&'a Path>,
}

fn c_walk_children(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "preproc_include" if enclosing_func.is_none() => {
                if let Some(target) = c_include_target(child, context) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom
                            .as_str()
                            .to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    continue;
                }
            }
            "type_definition" | "struct_specifier" | "class_specifier"
                if enclosing_func.is_none() =>
            {
                if let Some(name) = c_type_name(child, context.source) {
                    c_emit_type(child, context, &name, nodes, edges);
                    c_emit_inheritance(child, context, &name, edges);
                    if context.language == "cpp" {
                        c_walk_children(child, context, Some(&name), enclosing_func, nodes, edges);
                    }
                    continue;
                }
            }
            "class_interface"
            | "class_implementation"
            | "category_interface"
            | "protocol_declaration"
                if context.language == "objc" && enclosing_func.is_none() =>
            {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_type(child, context, &name, nodes, edges);
                    if child.kind() == "class_implementation" {
                        c_walk_children(child, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "function_definition" => {
                if let Some((name, scope)) = c_function_name(child, context.source) {
                    // An out-of-line `Widget::draw` belongs to Widget, so it
                    // qualifies the same way an in-class definition would.
                    let owner = scope.as_deref().or(enclosing_class);
                    c_emit_function(child, context, &name, owner, nodes, edges);
                    c_walk_children(child, context, owner, Some(&name), nodes, edges);
                    continue;
                }
            }
            "method_definition" if context.language == "objc" => {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    c_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "message_expression" if context.language == "objc" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        c_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn c_emit_type(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class.as_str().to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn c_emit_function(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn c_emit_call(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    if let Some(call_name) = c_call_name(node, context.source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls.as_str().to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = c_call_signature(node, context.source) {
        if let Some(edge) = c_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn c_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if context.language != "cpp" {
        return;
    }
    let Some(base_clause) = c_direct_child(node, &["base_class_clause"]) else {
        return;
    };
    let Some(base) = c_last_descendant_text(base_clause, context.source, &["type_identifier"])
    else {
        return;
    };
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Inherits.as_str().to_string(),
        source: qualify(context.file_path, name, None),
        target: base,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({
            "relationship_role": "extends",
            "syntax_source": "class_specifier",
        }),
    });
}

fn c_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" | "qualified_identifier" => {
            Some(node_text(callee, source).replace(" :: ", "::"))
        }
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list");
    found
}

/// The included header, as a repo-relative file path when one exists.
///
/// Objective-C `#import` used to keep the whole directive as the target, so
/// `#import "Logger.h"` never matched the header it names. An include is also
/// written relative to the including file or to a compiler search path, so the
/// literal text alone (`util.h`) matches no file in the graph.
fn c_include_target(node: tree_sitter::Node<'_>, context: &CParseContext<'_>) -> Option<String> {
    let target = c_direct_child(node, &["system_lib_string", "string_literal"])?;
    let literal = strip_matching_quotes(
        node_text(target, context.source)
            .trim()
            .trim_matches(['<', '>'].as_ref()),
    )
    .trim()
    .to_string();
    if literal.is_empty() {
        return None;
    }
    Some(c_resolve_include(&literal, context.file_path, context.repo_root).unwrap_or(literal))
}

/// Resolves an include against the including directory, then its ancestors,
/// standing in for the `-I` search paths the graph cannot know. A public
/// header usually sits in a parallel `include/` tree, so probe that too. A
/// system header such as `<vector>` matches nothing and keeps its literal name.
fn c_resolve_include(literal: &str, file_path: &str, repo_root: Option<&Path>) -> Option<String> {
    resolve_import_path(literal, file_path, repo_root, &["include"], true)
}

fn c_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    c_direct_child_text(node, source, &["type_identifier"])
}

/// Resolves a `function_definition` name plus the class it was declared under.
///
/// Only free functions name themselves with a plain `identifier`. An in-class
/// member uses `field_identifier`, and an out-of-line definition
/// (`void Widget::draw() {}`) uses `qualified_identifier`, whose scope names
/// the owning class. Matching on `identifier` alone dropped every C++ method.
fn c_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<(String, Option<String>)> {
    let declarator = node
        .child_by_field_name("declarator")
        .or_else(|| c_first_descendant(node, &["function_declarator"]))?;
    c_declarator_name(declarator, source)
}

fn c_declarator_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
        | "operator_name" => {
            let name = node_text(node, source).trim().to_string();
            (!name.is_empty()).then_some((name, None))
        }
        "qualified_identifier" => {
            // `A::B::method` should belong to `B`, so an inner scope wins.
            let scope = node
                .child_by_field_name("scope")
                .map(|scope| node_text(scope, source).trim().to_string());
            let (name, inner_scope) = c_declarator_name(node.child_by_field_name("name")?, source)?;
            Some((name, inner_scope.or(scope)))
        }
        "template_function" | "template_method" => {
            c_declarator_name(node.child_by_field_name("name")?, source)
        }
        "function_declarator"
        | "pointer_declarator"
        | "reference_declarator"
        | "parenthesized_declarator"
        | "array_declarator" => {
            let inner = node
                .child_by_field_name("declarator")
                .or_else(|| c_declarator_child(node))?;
            c_declarator_name(inner, source)
        }
        _ => None,
    }
}

/// `reference_declarator` carries no `declarator` field, so fall back to the
/// first child that can hold a name.
fn c_declarator_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    c_direct_child(
        node,
        &[
            "identifier",
            "field_identifier",
            "type_identifier",
            "destructor_name",
            "operator_name",
            "qualified_identifier",
            "template_function",
            "template_method",
            "function_declarator",
            "pointer_declarator",
            "reference_declarator",
            "parenthesized_declarator",
            "array_declarator",
        ],
    )
}

fn c_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        // `Factory::create()` must resolve to `create`, not to the scope.
        "qualified_identifier" | "template_function" => {
            c_declarator_name(callee, source).map(|(name, _)| name)
        }
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_message_selector(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut skipped_receiver = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "[" | "]" | ":") {
            continue;
        }
        if !skipped_receiver {
            skipped_receiver = true;
            continue;
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn c_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "popen" | "execvp" | "execv" | "execl" | "posix_spawn" => {
            ("invokes_binary", "subprocess")
        }
        "fopen" | "open" => ("opens_file", "file_io"),
        "fread" => ("reads_file", "file_io"),
        "fwrite" => ("writes_file", "file_io"),
        "dlopen" | "LoadLibrary" => ("loads_shared_library", "ffi"),
        "std::system" | "boost::process::child" => ("invokes_binary", "subprocess"),
        "std::ifstream" | "std::ofstream" | "std::fstream" => ("opens_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match c_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
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
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn c_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = c_direct_child(node, &["argument_list"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(c_string_text(child, source));
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn c_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn c_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn c_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    c_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn c_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = c_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn c_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    found: &mut Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            *found = Some(node_text(child, source));
        }
        c_collect_descendant_texts(child, source, kinds, found);
    }
}

fn c_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    c_collect_descendant_texts(node, source, kinds, &mut found);
    found
}

fn resolve_c_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}
