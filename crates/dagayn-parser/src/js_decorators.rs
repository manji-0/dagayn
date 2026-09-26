use serde_json::{Value, json};

use super::js_calls::{javascript_external_member_target, javascript_namespace_member_target};
use super::js_like::{
    is_javascript_function_value, javascript_bind_this, javascript_walk_children,
};
use super::js_modules::{JavaScriptParseContext, resolve_javascript_call_target};
use super::types::{ParsedEdge, ParsedNode};
use super::util::node_text;

/// Decorators of a class declaration: its own `decorator` children and, for
/// `@dec export class X`, those of the enclosing `export` statement.
pub(super) fn javascript_class_decorators(
    node: tree_sitter::Node<'_>,
) -> Vec<tree_sitter::Node<'_>> {
    let mut decorators = Vec::new();
    if let Some(parent) = node
        .parent()
        .filter(|parent| parent.kind() == "export_statement")
    {
        let mut cursor = parent.walk();
        decorators.extend(
            parent
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "decorator"),
        );
    }
    let mut cursor = node.walk();
    decorators.extend(
        node.named_children(&mut cursor)
            .filter(|child| child.kind() == "decorator"),
    );
    decorators
}

/// Decorators of a class member: the `decorator` siblings right before a
/// method in the class body, or a field's own `decorator` children.
pub(super) fn javascript_member_decorators(
    node: tree_sitter::Node<'_>,
) -> Vec<tree_sitter::Node<'_>> {
    let mut decorators = Vec::new();
    if matches!(node.kind(), "public_field_definition" | "field_definition") {
        let mut cursor = node.walk();
        decorators.extend(
            node.named_children(&mut cursor)
                .filter(|child| child.kind() == "decorator"),
        );
        return decorators;
    }
    if node
        .parent()
        .is_none_or(|parent| parent.kind() != "class_body")
    {
        return decorators;
    }
    let mut current = node.prev_named_sibling();
    while let Some(sibling) = current {
        match sibling.kind() {
            "decorator" => decorators.push(sibling),
            "comment" => {}
            _ => break,
        }
        current = sibling.prev_named_sibling();
    }
    decorators.reverse();
    decorators
}

/// Decorator names of the non-function fields of a class body
/// (`@Input() title = ""`), de-duplicated in source order.
pub(super) fn javascript_field_decorator_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let Some(body) = node.child_by_field_name("body") else {
        return names;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if !matches!(
            member.kind(),
            "public_field_definition" | "field_definition"
        ) || member
            .child_by_field_name("value")
            .is_some_and(|value| is_javascript_function_value(value.kind()))
        {
            continue;
        }
        for name in javascript_decorator_names(&javascript_member_decorators(member), source) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The decorator expression's callee: `@dec`, `@ns.dec`, `@dec(...)`.
fn javascript_decorator_callee(decorator: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let expression = decorator.named_child(0)?;
    let callee = match expression.kind() {
        "call_expression" => expression.child_by_field_name("function")?,
        _ => expression,
    };
    matches!(callee.kind(), "identifier" | "member_expression").then_some(callee)
}

/// Callee names in the format Python decorators use (`Injectable`,
/// `ng.Component`).
pub(super) fn javascript_decorator_names(
    decorators: &[tree_sitter::Node<'_>],
    source: &[u8],
) -> Vec<String> {
    decorators
        .iter()
        .filter_map(|decorator| javascript_decorator_callee(*decorator))
        .map(|callee| node_text(callee, source).trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

pub(super) fn javascript_has_data_model_decorator_name(names: &[String]) -> bool {
    names.iter().any(|name| {
        let short = name.rsplit('.').next().unwrap_or(name);
        matches!(
            short,
            "Entity" | "ObjectType" | "InputType" | "ArgsType" | "Schema" | "model" | "Table"
        )
    })
}

/// `REFERENCES decorated -> decorator` (`relationship_role: "decorator"`)
/// for each decorator, and the calls in their arguments attributed to
/// `caller_owner` + `caller_name` (the decorated node). `this_owner` is the
/// owner `this` refers to inside the arguments.
#[allow(clippy::too_many_arguments)]
pub(super) fn javascript_emit_decorators(
    decorators: &[tree_sitter::Node<'_>],
    decorated: &str,
    context: &JavaScriptParseContext<'_>,
    caller_owner: Option<&str>,
    caller_name: &str,
    this_owner: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    if decorators.is_empty() {
        return;
    }
    let snapshot = context.bindings.borrow().snapshot();
    javascript_bind_this(context, this_owner);
    for decorator in decorators {
        javascript_emit_decorator(
            *decorator,
            decorated,
            context,
            caller_owner,
            Some(caller_name),
            nodes,
            edges,
        );
    }
    context.bindings.borrow_mut().restore(snapshot);
}

/// One decorator: the `REFERENCES` edge from `decorated`, then a walk of the
/// decorator call's arguments with the given caller.
pub(super) fn javascript_emit_decorator(
    decorator: tree_sitter::Node<'_>,
    decorated: &str,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let Some(callee) = javascript_decorator_callee(decorator) {
        let target = match callee.kind() {
            "member_expression" => {
                let name = callee
                    .child_by_field_name("property")
                    .map(|property| node_text(property, context.source))
                    .unwrap_or_default();
                callee
                    .child_by_field_name("object")
                    .and_then(|object| javascript_namespace_member_target(object, &name, context))
                    .or_else(|| javascript_external_member_target(callee, context))
                    .unwrap_or(name)
            }
            _ => resolve_javascript_call_target(&node_text(callee, context.source), context),
        };
        if !target.is_empty() {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::References,
                source: decorated.to_string(),
                target,
                file_path: context.file_path.clone(),
                line: decorator.start_position().row as i64 + 1,
                extra: json!({"relationship_role": "decorator"}),
            });
        }
    }
    if let Some(arguments) = decorator
        .named_child(0)
        .filter(|expression| expression.kind() == "call_expression")
        .and_then(|call| call.child_by_field_name("arguments"))
    {
        javascript_walk_children(arguments, context, owner_path, enclosing_func, nodes, edges);
    }
}

/// Decorators that their declaration's emitter handles (class, method, and
/// function-valued field decorators); the walk skips them.
pub(super) fn javascript_decorator_is_owned(decorator: tree_sitter::Node<'_>) -> bool {
    let Some(parent) = decorator.parent() else {
        return false;
    };
    match parent.kind() {
        "export_statement"
        | "class_declaration"
        | "abstract_class_declaration"
        | "class"
        | "class_body" => true,
        "public_field_definition" | "field_definition" => parent
            .child_by_field_name("value")
            .is_some_and(|value| is_javascript_function_value(value.kind())),
        _ => false,
    }
}

pub(super) fn javascript_insert_decorator_names(
    extra: &mut Value,
    decorators: &[tree_sitter::Node<'_>],
    source: &[u8],
) {
    let names = javascript_decorator_names(decorators, source);
    if !names.is_empty()
        && let Some(map) = extra.as_object_mut()
    {
        map.insert("decorators".to_string(), json!(names));
    }
}
