use serde_json::json;

use super::js_calls::{javascript_call_name, javascript_namespace_member_target};
use super::js_modules::{JavaScriptParseContext, resolve_javascript_call_target};
use super::js_types::javascript_emit_heritage_type_references;
use super::types::ParsedEdge;
use super::util::node_text;

struct JavaScriptHeritageBase {
    target: String,
    role: &'static str,
    heritage_expression: Option<String>,
}

pub(super) fn emit_javascript_inheritance_edges(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    qualified: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut bases = Vec::new();
    let mut mixin_calls = Vec::new();
    collect_javascript_bases(node, context, &mut bases, &mut mixin_calls);
    for base in bases {
        let mut extra = json!({"relationship_role": base.role, "syntax_source": node.kind()});
        if let (Some(expression), Some(map)) = (base.heritage_expression, extra.as_object_mut()) {
            map.insert("heritage_expression".to_string(), json!(expression));
        }
        edges.push(ParsedEdge {
            kind: if base.role == "implements" {
                crate::core::types::EdgeKind::Implements
            } else {
                crate::core::types::EdgeKind::Inherits
            },
            source: qualified.to_string(),
            target: base.target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
    }
    javascript_emit_heritage_type_references(node, qualified, context, edges);
    for (callee, line) in mixin_calls {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: qualified.to_string(),
            target: resolve_javascript_call_target(&callee, context),
            file_path: context.file_path.clone(),
            line,
            extra: json!({}),
        });
    }
}

/// Reads the heritage of one class / interface declaration (not of nested
/// declarations in its body).
///
/// TypeScript classes wrap `extends_clause` / `implements_clause` in
/// `class_heritage`; JavaScript `class_heritage` holds the base expression
/// directly; interfaces use `extends_type_clause`.
fn collect_javascript_bases(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    bases: &mut Vec<JavaScriptHeritageBase>,
    mixin_calls: &mut Vec<(String, i64)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_heritage" => {
                let mut heritage_cursor = child.walk();
                for part in child.named_children(&mut heritage_cursor) {
                    match part.kind() {
                        "extends_clause" => {
                            let mut clause_cursor = part.walk();
                            for value in part.children_by_field_name("value", &mut clause_cursor) {
                                javascript_expression_base(value, context, bases, mixin_calls);
                            }
                        }
                        "implements_clause" => {
                            javascript_type_bases(part, "implements", context, bases);
                        }
                        "comment" => {}
                        // JavaScript: `class_heritage` holds the expression itself.
                        _ => javascript_expression_base(part, context, bases, mixin_calls),
                    }
                }
            }
            "extends_type_clause" => javascript_type_bases(child, "extends", context, bases),
            _ => {}
        }
    }
}

fn javascript_expression_base(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    bases: &mut Vec<JavaScriptHeritageBase>,
    mixin_calls: &mut Vec<(String, i64)>,
) {
    let heritage_expression =
        (node.kind() != "identifier").then(|| node_text(node, context.source).trim().to_string());
    let mut current = node;
    loop {
        match current.kind() {
            "parenthesized_expression" => match current.named_child(0) {
                Some(inner) => current = inner,
                None => return,
            },
            "call_expression" => {
                if let Some(callee) = javascript_call_name(current, context.source) {
                    mixin_calls.push((callee, current.start_position().row as i64 + 1));
                }
                // The innermost identifiable argument is the real base:
                // `Mixin(Other(Base))` -> `Base`.
                let Some(argument) = current
                    .child_by_field_name("arguments")
                    .and_then(|arguments| arguments.named_child(0))
                else {
                    return;
                };
                current = argument;
            }
            "identifier" => {
                bases.push(JavaScriptHeritageBase {
                    target: node_text(current, context.source),
                    role: "extends",
                    heritage_expression,
                });
                return;
            }
            "member_expression" => {
                let Some(property) = current.child_by_field_name("property") else {
                    return;
                };
                let name = node_text(property, context.source);
                let target = current
                    .child_by_field_name("object")
                    .and_then(|object| javascript_namespace_member_target(object, &name, context))
                    .unwrap_or(name);
                bases.push(JavaScriptHeritageBase {
                    target,
                    role: "extends",
                    heritage_expression,
                });
                return;
            }
            _ => return,
        }
    }
}

fn javascript_type_bases(
    clause: tree_sitter::Node<'_>,
    role: &'static str,
    context: &JavaScriptParseContext<'_>,
    bases: &mut Vec<JavaScriptHeritageBase>,
) {
    let mut cursor = clause.walk();
    for child in clause.named_children(&mut cursor) {
        if let Some((target, heritage_expression)) = javascript_type_base(child, context) {
            bases.push(JavaScriptHeritageBase {
                target,
                role,
                heritage_expression,
            });
        }
    }
}

/// Base name of a heritage type: `Repo`, `Service<T>` -> `Service`,
/// `ns.Marker` -> the namespace-imported QN (or bare `Marker`).
fn javascript_type_base(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<(String, Option<String>)> {
    match node.kind() {
        "type_identifier" | "identifier" => Some((node_text(node, context.source), None)),
        "generic_type" => javascript_type_base(node.child_by_field_name("name")?, context),
        "nested_type_identifier" => {
            let name = node_text(node.child_by_field_name("name")?, context.source);
            let target = node
                .child_by_field_name("module")
                .and_then(|module| javascript_namespace_member_target(module, &name, context))
                .unwrap_or(name);
            Some((target, Some(node_text(node, context.source))))
        }
        _ => None,
    }
}
