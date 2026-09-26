//! Type references (docs/TYPESCRIPT-EXTRACTION.md §7.4): every type a
//! declaration names in its signature or its body becomes
//! `REFERENCES owner -> type` (`relationship_role: "type_reference"`, or
//! `"type_query"` for `typeof X`), with the positions it appears in
//! (`type_positions`). Only types the repository declares are targets.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::js_members::resolve_javascript_type_reference;
use super::js_modules::JavaScriptParseContext;
use super::qualify;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::node_text;

/// Where a type subtree sits, when `node` is the outermost node of one: the
/// annotation of a parameter, field, return, or variable, a type parameter
/// list, an `as` / `satisfies` target, call type arguments, or the class of
/// an `instanceof` test.
pub(super) fn javascript_type_root_position(node: tree_sitter::Node<'_>) -> Option<&'static str> {
    if !node.is_named() {
        return None;
    }
    let parent = node.parent()?;
    match node.kind() {
        "type_annotation" => Some(match parent.kind() {
            "required_parameter" | "optional_parameter"
                if javascript_is_parameter_property(parent) =>
            {
                "parameter_property"
            }
            "required_parameter" | "optional_parameter" => "parameter",
            "public_field_definition" | "field_definition" | "property_signature" => "field",
            "index_signature" => "index_signature",
            "variable_declarator" | "catch_clause" => "variable_annotation",
            "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "function"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
            | "method_signature"
            | "abstract_method_signature"
            | "function_signature"
            | "call_signature"
            | "construct_signature" => "return",
            _ => "annotation",
        }),
        "type_predicate_annotation" | "asserts_annotation" => Some("type_predicate"),
        "type_parameters" => Some("type_parameter"),
        "type_arguments" if parent.kind() == "type_assertion" => Some("as"),
        "type_arguments" => Some("type_argument"),
        _ => match parent.kind() {
            "as_expression" | "satisfies_expression"
                if parent
                    .named_child(0)
                    .is_some_and(|expression| expression.id() != node.id()) =>
            {
                Some(if parent.kind() == "as_expression" {
                    "as"
                } else {
                    "satisfies"
                })
            }
            "binary_expression"
                if parent
                    .child_by_field_name("right")
                    .is_some_and(|right| right.id() == node.id())
                    && parent
                        .child_by_field_name("operator")
                        .is_some_and(|operator| operator.kind() == "instanceof") =>
            {
                Some("instanceof")
            }
            _ => None,
        },
    }
}

/// `constructor(private repo: Repo)`: the parameter declares a field.
fn javascript_is_parameter_property(parameter: tree_sitter::Node<'_>) -> bool {
    let mut cursor = parameter.walk();
    parameter.children(&mut cursor).any(|part| {
        matches!(
            part.kind(),
            "accessibility_modifier" | "readonly" | "override_modifier"
        )
    })
}

/// The node a type reference at `position` belongs to: the function being
/// walked, else the nearest container (class, interface, namespace), else
/// the file. A parameter property's type belongs to the class it declares a
/// field of. Code in a function body, including local declarations,
/// belongs to that function (§7.2).
pub(super) fn javascript_type_reference_source(
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    position: &str,
) -> String {
    match (enclosing_func, owner_path) {
        (Some("constructor"), Some(owner)) if position == "parameter_property" => {
            qualify(&context.file_path, owner, None)
        }
        (Some(func), _) => qualify(&context.file_path, func, owner_path),
        (None, Some(owner)) => qualify(&context.file_path, owner, None),
        (None, None) => context.file_path.to_string(),
    }
}

/// Emits one `REFERENCES source -> type` per type named in `root` (merged
/// per `(source, target)` by [`javascript_merge_type_references`]).
pub(super) fn javascript_emit_type_references(
    root: tree_sitter::Node<'_>,
    position: &'static str,
    source: &str,
    context: &JavaScriptParseContext<'_>,
    edges: &mut Vec<ParsedEdge>,
) {
    let scope = source
        .strip_prefix(context.file_path.as_str())
        .and_then(|rest| rest.strip_prefix("::"));
    let mut found = Vec::new();
    javascript_collect_type_references(root, position, scope, context, &mut found);
    for (target, role, position, line) in found {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::References,
            source: source.to_string(),
            target,
            file_path: context.file_path.clone(),
            line,
            extra: json!({"relationship_role": role, "type_positions": [position]}),
        });
    }
}

type JavaScriptTypeReference = (String, &'static str, &'static str, i64);

fn javascript_collect_type_references(
    node: tree_sitter::Node<'_>,
    position: &'static str,
    scope: Option<&str>,
    context: &JavaScriptParseContext<'_>,
    found: &mut Vec<JavaScriptTypeReference>,
) {
    let (name, role) = match node.kind() {
        "type_identifier" | "nested_type_identifier" => (
            node_text(node, context.source)
                .split_whitespace()
                .collect::<String>(),
            "type_reference",
        ),
        "type_query" => {
            let Some(operand) = node
                .named_child(0)
                .filter(|operand| matches!(operand.kind(), "identifier" | "member_expression"))
            else {
                return;
            };
            (
                node_text(operand, context.source)
                    .split_whitespace()
                    .collect::<String>(),
                "type_query",
            )
        }
        // `x instanceof Repo`, `x instanceof ns.Repo`.
        "identifier" | "member_expression" if position == "instanceof" => (
            node_text(node, context.source)
                .split_whitespace()
                .collect::<String>(),
            "type_reference",
        ),
        // `infer U` declares `U`.
        "infer_type" => return,
        _ => {
            let mut cursor = node.walk();
            for (index, child) in node.children(&mut cursor).enumerate() {
                if !child.is_named() {
                    continue;
                }
                // Declared names: `T` of `<T extends X>`, `K` of `[K in ...]`,
                // and the name of a local interface / alias / enum.
                let field = node.field_name_for_child(index as u32);
                if field == Some("name")
                    && matches!(
                        node.kind(),
                        "type_parameter"
                            | "mapped_type_clause"
                            | "interface_declaration"
                            | "type_alias_declaration"
                            | "enum_declaration"
                    )
                {
                    continue;
                }
                let position = match (position, child.kind()) {
                    ("type_parameter", "constraint") => "type_parameter_constraint",
                    ("type_parameter", "default_type") => "type_parameter_default",
                    _ => position,
                };
                javascript_collect_type_references(child, position, scope, context, found);
            }
            return;
        }
    };
    let root = name.split('.').next().unwrap_or_default();
    if root.is_empty()
        || context
            .local_scopes
            .borrow()
            .iter()
            .any(|locals| locals.contains(root))
    {
        return;
    }
    let Some(target) =
        resolve_javascript_type_reference(context, &name, scope, role == "type_query")
    else {
        return;
    };
    if javascript_type_name_is_bound(node, root, context.source) {
        return;
    }
    found.push((target, role, position, node.start_position().row as i64 + 1));
}

/// Whether `name` is bound by an enclosing type parameter list, mapped type
/// (`[K in keyof T]`), or conditional type (`infer U`) rather than naming a
/// declaration.
fn javascript_type_name_is_bound(node: tree_sitter::Node<'_>, name: &str, source: &[u8]) -> bool {
    let declares = |declared: Option<tree_sitter::Node<'_>>| {
        declared.is_some_and(|declared| node_text(declared, source) == name)
    };
    let mut current = node.parent();
    while let Some(ancestor) = current {
        let mut cursor = ancestor.walk();
        for child in ancestor.named_children(&mut cursor) {
            let bound = match child.kind() {
                "type_parameters" => {
                    let mut parameters = child.walk();
                    child
                        .named_children(&mut parameters)
                        .any(|parameter| declares(parameter.child_by_field_name("name")))
                }
                "mapped_type_clause" => declares(child.child_by_field_name("name")),
                _ => false,
            };
            if bound {
                return true;
            }
        }
        if ancestor.kind() == "conditional_type"
            && javascript_declares_infer(ancestor, name, source)
        {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

fn javascript_declares_infer(node: tree_sitter::Node<'_>, name: &str, source: &[u8]) -> bool {
    if node.kind() == "infer_type" {
        return node
            .named_child(0)
            .is_some_and(|declared| node_text(declared, source) == name);
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| javascript_declares_infer(child, name, source))
}

/// Type arguments of a class or interface heritage clause
/// (`implements Service<User>`, `extends Base<Props>`); the bases
/// themselves are `INHERITS` / `IMPLEMENTS`.
pub(super) fn javascript_emit_heritage_type_references(
    node: tree_sitter::Node<'_>,
    qualified: &str,
    context: &JavaScriptParseContext<'_>,
    edges: &mut Vec<ParsedEdge>,
) {
    fn visit(
        node: tree_sitter::Node<'_>,
        qualified: &str,
        context: &JavaScriptParseContext<'_>,
        edges: &mut Vec<ParsedEdge>,
    ) {
        if node.kind() == "type_arguments" {
            javascript_emit_type_references(
                node,
                "heritage_type_argument",
                qualified,
                context,
                edges,
            );
            return;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, qualified, context, edges);
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "class_heritage" | "extends_type_clause") {
            visit(child, qualified, context, edges);
        }
    }
}

fn javascript_is_type_reference_edge(edge: &ParsedEdge) -> bool {
    edge.kind == crate::core::types::EdgeKind::References
        && matches!(
            edge.extra.get("relationship_role").and_then(Value::as_str),
            Some("type_reference" | "type_query")
        )
}

/// One type-reference edge per `(source, target)`: the first occurrence
/// keeps its line, `type_positions` lists every position in source order
/// (each once), and the role is `type_query` only when every occurrence is
/// a `typeof`. A source that is not a node (merged away, or code outside
/// any declaration) becomes the file; self references are dropped.
pub(super) fn javascript_merge_type_references(
    nodes: &[ParsedNode],
    edges: &mut Vec<ParsedEdge>,
    file_path: &FilePath,
) {
    if !edges.iter().any(javascript_is_type_reference_edge) {
        return;
    }
    let known = nodes
        .iter()
        .map(|node| match node.kind {
            crate::core::types::NodeKind::File => file_path.to_string(),
            _ => qualify(&node.file_path, &node.name, node.parent_name.as_deref()),
        })
        .collect::<HashSet<_>>();
    let mut merged: HashMap<(String, String), usize> = HashMap::new();
    let mut kept = Vec::with_capacity(edges.len());
    for mut edge in edges.drain(..) {
        if !javascript_is_type_reference_edge(&edge) {
            kept.push(edge);
            continue;
        }
        if !known.contains(&edge.source) {
            edge.source = file_path.to_string();
        }
        if edge.source == edge.target {
            continue;
        }
        let key = (edge.source.clone(), edge.target.clone());
        let Some(&index) = merged.get(&key) else {
            merged.insert(key, kept.len());
            kept.push(edge);
            continue;
        };
        let existing: &mut ParsedEdge = &mut kept[index];
        if edge.extra["relationship_role"] == "type_reference" {
            existing.extra["relationship_role"] = json!("type_reference");
        }
        let positions = edge.extra["type_positions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(list) = existing.extra["type_positions"].as_array_mut() {
            for position in positions {
                if !list.contains(&position) {
                    list.push(position);
                }
            }
        }
    }
    *edges = kept;
}

/// Emits the type references of the type subtrees below `node` that the
/// walk does not visit, skipping the `skip` fields (a body the walk covers
/// itself): the signatures of local-class members and of object-literal
/// methods in bodies.
pub(super) fn javascript_emit_type_roots(
    node: tree_sitter::Node<'_>,
    skip: &[&str],
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for (index, child) in node.children(&mut cursor).enumerate() {
        if node
            .field_name_for_child(index as u32)
            .is_some_and(|field| skip.contains(&field))
        {
            continue;
        }
        match javascript_type_root_position(child) {
            Some(position) => {
                let source =
                    javascript_type_reference_source(context, owner_path, enclosing_func, position);
                javascript_emit_type_references(child, position, &source, context, edges);
            }
            None => {
                javascript_emit_type_roots(child, &[], context, owner_path, enclosing_func, edges)
            }
        }
    }
}
