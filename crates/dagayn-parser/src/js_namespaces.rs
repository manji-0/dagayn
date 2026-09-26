use serde_json::json;

use super::js_like::{
    javascript_container_qn, javascript_member_owner, javascript_push_node,
    javascript_walk_children,
};
use super::js_modules::{JavaScriptParseContext, decode_javascript_string_literal};
use super::qualify;
use super::types::{ParsedEdge, ParsedNode};
use super::util::node_text;

/// Segments and role of `namespace A.B.C` / `module Legacy` (`namespace`) or
/// `module "x"` (`ambient_module`).
pub(super) fn javascript_namespace_segments(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Vec<String>, &'static str)> {
    let name = node.child_by_field_name("name")?;
    match name.kind() {
        "identifier" => Some((vec![node_text(name, source)], "namespace")),
        "nested_identifier" => {
            let mut segments = Vec::new();
            javascript_nested_identifier_segments(name, source, &mut segments);
            (!segments.is_empty()).then_some((segments, "namespace"))
        }
        "string" => {
            let module = decode_javascript_string_literal(name, source);
            (!module.is_empty()).then(|| (vec![module], "ambient_module"))
        }
        _ => None,
    }
}

fn javascript_nested_identifier_segments(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    segments: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "nested_identifier" | "member_expression" => {
                javascript_nested_identifier_segments(child, source, segments)
            }
            "identifier" | "property_identifier" => segments.push(node_text(child, source)),
            _ => {}
        }
    }
}

pub(super) fn javascript_named_child_node<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
) -> Option<tree_sitter::Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Emits `namespace A.B.C { ... }` / `module "x" { ... }` as nested `Class`
/// nodes and walks the body with the innermost path as the owner.
pub(super) fn javascript_emit_namespace(
    node: tree_sitter::Node<'_>,
    segments: &[String],
    role: &'static str,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    // `module "x" {}` outside `declare` is still an ambient module.
    let ambient = role == "ambient_module";
    if ambient {
        context.ambient_depth.set(context.ambient_depth.get() + 1);
    }
    javascript_emit_namespace_body(
        node, body, segments, role, context, owner_path, nodes, edges,
    );
    if ambient {
        context.ambient_depth.set(context.ambient_depth.get() - 1);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn javascript_emit_namespace_body(
    node: tree_sitter::Node<'_>,
    body: tree_sitter::Node<'_>,
    segments: &[String],
    role: &'static str,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let line_start = node.start_position().row as i64 + 1;
    let line_end = node.end_position().row as i64 + 1;
    let mut owner = owner_path.map(str::to_string);
    for segment in segments {
        // Declaration merging: `namespace Outer {}` twice is one node.
        let exists = nodes.iter().any(|existing| {
            existing.kind == crate::core::types::NodeKind::Class
                && existing.name == *segment
                && existing.parent_name == owner
        });
        if !exists {
            javascript_push_node(
                context,
                nodes,
                node,
                ParsedNode {
                    kind: crate::core::types::NodeKind::Class,
                    name: segment.clone(),
                    file_path: context.file_path.clone(),
                    line_start,
                    line_end,
                    language: context.language.to_string(),
                    parent_name: owner.clone(),
                    params: None,
                    return_type: None,
                    modifiers: None,
                    is_test: false,
                    extra: json!({"type_role": role}),
                },
            );
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::Contains,
                source: javascript_container_qn(context, owner.as_deref()),
                target: qualify(&context.file_path, segment, owner.as_deref()),
                file_path: context.file_path.clone(),
                line: line_start,
                extra: json!({}),
            });
        }
        owner = Some(javascript_member_owner(owner.as_deref(), segment));
    }
    javascript_walk_children(body, context, owner.as_deref(), None, nodes, edges);
}

/// `export as namespace MyLib;` -> `MyLib`.
pub(super) fn javascript_umd_global_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    let is_umd = children.iter().any(|child| child.kind() == "as")
        && children.iter().any(|child| child.kind() == "namespace");
    if !is_umd {
        return None;
    }
    children
        .iter()
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(*child, source))
}
