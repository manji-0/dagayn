use std::collections::HashSet;

use serde_json::json;

use super::stdlib::r::{is_r_base_package, r_default_package};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child, direct_child_text, first_descendant_text, line_count, line_of, node_text,
    strip_matching_quotes,
};
use super::{add_tested_by_edges, is_test_function, qualify, resolve_rust_call_targets};

pub(super) fn parse_r_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "r")];
    let mut edges = Vec::new();
    let context = RParseContext {
        source,
        file_path: file_path.clone(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        r_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        r_mark_stdlib_calls(&nodes, &mut edges);
        let mut edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct RParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
}

fn r_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "binary_operator"
                if r_handle_binary_operator(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) =>
            {
                continue;
            }
            "call"
                if r_handle_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) =>
            {
                continue;
            }
            _ => {}
        }
        r_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn r_handle_binary_operator(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some((left, operator, right)) = r_binary_operator_parts(node) else {
        return false;
    };
    // `->` and `->>` bind right-to-left, so the target is on the right.
    let (target, value) = match operator.kind() {
        "<-" | "=" | "<<-" => (left, right),
        "->" | "->>" => (right, left),
        _ => return false,
    };
    if target.kind() != "identifier" {
        return false;
    }
    let mut value = value;
    while value.kind() == "parenthesized_expression" {
        let mut cursor = value.walk();
        let Some(inner) = value.named_children(&mut cursor).next() else {
            break;
        };
        value = inner;
    }
    let (name, right) = (node_text(target, context.source), value);
    if right.kind() == "function_definition" {
        // `f <- function` in a function body binds a local; `<<-` / `->>`
        // assign in an enclosing environment.
        let local_parent = enclosing_func
            .filter(|_| matches!(operator.kind(), "<-" | "=" | "->"))
            .map(|func| match enclosing_class {
                Some(class) => format!("{class}.{func}"),
                None => func.to_string(),
            });
        let parent = local_parent.as_deref().or(enclosing_class);
        r_emit_function(right, context, &name, parent, nodes, edges);
        r_walk_children(right, context, parent, Some(&name), nodes, edges);
        return true;
    }
    if right.kind() == "call"
        && let Some(call_name) = r_call_name(right, context.source)
        && r_is_class_constructor(&call_name)
    {
        r_emit_class_call(right, context, Some(&name), enclosing_class, nodes, edges);
        return true;
    }
    false
}

fn r_handle_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(call_name) = r_call_name(node, context.source) else {
        return false;
    };

    if matches!(call_name.as_str(), "library" | "require" | "source") {
        if let Some(target) = r_import_target(node, context.source) {
            let mut edge = ParsedEdge::new(
                crate::core::types::EdgeKind::ImportsFrom,
                context.file_path.to_string(),
                target,
                context.file_path.clone(),
                line_of(node),
            );
            // `library(stats)` attaches a package shipped with R;
            // `source()` always reads a script.
            if call_name != "source" && is_r_base_package(&edge.target) {
                let package = edge.target.clone();
                mark_stdlib_edge(
                    &mut edge.target,
                    &mut edge.extra,
                    &package,
                    StdlibEvidence::Certain,
                );
            }
            edges.push(edge);
        }
        return true;
    }

    if r_is_class_constructor(&call_name) {
        r_emit_class_call(node, context, None, enclosing_class, nodes, edges);
        return true;
    }

    r_emit_call(
        node,
        context,
        &call_name,
        enclosing_class,
        enclosing_func,
        edges,
    );
    r_walk_children(node, context, enclosing_class, enclosing_func, nodes, edges);
    true
}

/// Points the calls into R's base packages at the package: `stats::median`
/// (or `:::`) certainly, and a bare name of a package R attaches by default
/// (`paste` → `base`, `sd` → `stats`, `head` → `utils`) likely, since a
/// package attached later may mask it. A function this file assigns
/// (`paste <- function`) is its own.
fn r_mark_stdlib_calls(nodes: &[ParsedNode], edges: &mut [ParsedEdge]) {
    let defined = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::Calls {
            continue;
        }
        let qualified = edge
            .target
            .split_once(":::")
            .or_else(|| edge.target.split_once("::"));
        let (package, evidence) = match qualified {
            Some((package, _)) if is_r_base_package(package) => {
                (package.to_string(), StdlibEvidence::Certain)
            }
            Some(_) => continue,
            None if defined.contains(edge.target.as_str()) => continue,
            None => match r_default_package(&edge.target) {
                Some(package) => (package.to_string(), StdlibEvidence::Likely),
                None => continue,
            },
        };
        mark_stdlib_edge(&mut edge.target, &mut edge.extra, &package, evidence);
    }
}

fn r_is_class_constructor(call_name: &str) -> bool {
    matches!(
        call_name.strip_prefix("R6::").unwrap_or(call_name),
        "setRefClass" | "setClass" | "setGeneric" | "R6Class"
    )
}

fn r_emit_function(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, enclosing_class);
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
        language: "r".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: direct_child_text(node, context.source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn r_emit_class_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    assigned_name: Option<&str>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(class_name) = r_first_string_arg(node, context.source).or_else(|| {
        assigned_name
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }) else {
        return;
    };
    let qualified = qualify(&context.file_path, &class_name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: class_name.clone(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "r".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        context.file_path.to_string(),
        qualified.clone(),
        context.file_path.clone(),
        line_of(node),
    ));
    // S4/RC use `contains`; R6 uses `inherit`.
    for key in ["contains", "inherit"] {
        let Some(value) = r_find_named_arg(node, context.source, key) else {
            continue;
        };
        for base in r_class_references(value, context.source) {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::Inherits,
                source: qualified.clone(),
                target: base,
                file_path: context.file_path.clone(),
                line: node.start_position().row as i64 + 1,
                extra: json!({"relationship_role": "extends", "syntax_source": key}),
            });
        }
    }
    // RC declares `methods`; R6 splits them across `public`/`private`/`active`.
    for key in ["methods", "public", "private", "active"] {
        if let Some(methods) = r_find_named_arg(node, context.source, key) {
            r_extract_methods(methods, context, &class_name, nodes, edges);
        }
    }
}

/// Class names in a `contains`/`inherit` value: `"P"`, `Base`, or `c("A", "B")`.
fn r_class_references(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    match node.kind() {
        "identifier" => vec![node_text(node, source)],
        "string" => first_descendant_text(node, source, &["string_content"])
            .into_iter()
            .collect(),
        "call" => {
            let mut out = Vec::new();
            let mut stack = vec![node];
            while let Some(current) = stack.pop() {
                if current.kind() == "string" {
                    out.extend(first_descendant_text(current, source, &["string_content"]));
                    continue;
                }
                let mut cursor = current.walk();
                let children: Vec<_> = current.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev());
            }
            out
        }
        _ => Vec::new(),
    }
}

fn r_extract_methods(
    list_call: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    class_name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    for (method_name, value) in r_iter_args(list_call, context.source) {
        let Some(method_name) = method_name else {
            continue;
        };
        if value.kind() != "function_definition" {
            continue;
        }
        r_emit_function(value, context, &method_name, Some(class_name), nodes, edges);
        r_walk_children(
            value,
            context,
            Some(class_name),
            Some(&method_name),
            nodes,
            edges,
        );
    }
}

fn r_emit_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    call_name: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Calls,
        caller.clone(),
        call_name.to_string(),
        context.file_path.clone(),
        line_of(node),
    ));
    if let Some(edge) = r_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn r_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "system2" => ("invokes_binary", "subprocess"),
        ".Call" | ".External" => ("loads_native_module", "ffi"),
        "dyn.load" | "library.dynam" => ("loads_shared_library", "ffi"),
        "readLines" | "read.csv" | "read.table" => ("reads_file", "file_io"),
        "writeLines" | "write.csv" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match r_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
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
            "evidence_source": signature,
            "source_language": "r",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn r_binary_operator_parts<'a>(
    node: tree_sitter::Node<'a>,
) -> Option<(
    tree_sitter::Node<'a>,
    tree_sitter::Node<'a>,
    tree_sitter::Node<'a>,
)> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    if children.len() < 3 {
        return None;
    }
    Some((children[0], children[1], children[2]))
}

fn r_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "identifier" | "namespace_operator") {
            return Some(node_text(child, source));
        }
    }
    None
}

fn r_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (_, value) = r_iter_args(node, source).into_iter().next()?;
    match value.kind() {
        "identifier" => Some(node_text(value, source)),
        "string" => r_string_text(value, source),
        _ => None,
    }
}

fn r_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (_, value) = r_iter_args(node, source).into_iter().next()?;
    if value.kind() == "string" {
        r_string_text(value, source)
    } else {
        None
    }
}

fn r_find_named_arg<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    arg_name: &str,
) -> Option<tree_sitter::Node<'a>> {
    r_iter_args(node, source)
        .into_iter()
        .find_map(|(name, value)| (name.as_deref() == Some(arg_name)).then_some(value))
}

fn r_iter_args<'a>(
    call_node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Vec<(Option<String>, tree_sitter::Node<'a>)> {
    let Some(arguments) = direct_child(call_node, &["arguments"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = arguments.walk();
    for argument in arguments.children(&mut cursor) {
        if argument.kind() != "argument" {
            continue;
        }
        let mut name = None;
        let mut value = None;
        let mut seen_equals = false;
        let mut arg_cursor = argument.walk();
        for child in argument.children(&mut arg_cursor) {
            if child.kind() == "=" {
                seen_equals = true;
                continue;
            }
            if !child.is_named() {
                continue;
            }
            if seen_equals {
                value = Some(child);
                break;
            }
            if name.is_none() && child.kind() == "identifier" {
                name = Some(node_text(child, source));
                continue;
            }
            if value.is_none() {
                value = Some(child);
                break;
            }
        }
        if let Some(value) = value.or_else(|| r_first_named_child(argument)) {
            out.push((seen_equals.then_some(name).flatten(), value));
        }
    }
    out
}

fn r_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn r_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor).find(|child| child.is_named())
}
