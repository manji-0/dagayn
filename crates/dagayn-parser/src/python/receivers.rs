//! The types Python code writes down for the receivers of member calls:
//! parameter annotations (`def f(store: GraphStore)`), and the attributes a
//! class gives `self` (`self.store = GraphStore(...)`, `self.path: Path`,
//! `store: GraphStore` in the class body, `self.store = store` of an
//! annotated parameter). `store.upsert_node()` then names the type's
//! method instead of a bare `upsert_node` any class may define.

use std::collections::HashMap;

use super::super::util::node_text;

/// The type an annotation names, as written: `GraphStore`, `pkg.Store`;
/// `Optional[T]`, `T | None`, and `"T"` give `T`; a generic `list[str]`
/// gives `list`. `None` for `None` or anything else.
pub(super) fn python_annotation_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type" | "type_annotation" | "parenthesized_expression" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find_map(|child| python_annotation_type(child, source))
        }
        "identifier" | "attribute" => Some(node_text(node, source)),
        "string" => {
            let text = node_text(node, source);
            let inner = text.trim_matches(|c| c == '"' || c == '\'').trim();
            let valid = !inner.is_empty()
                && inner
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
            valid.then(|| inner.to_string())
        }
        // `T | None`
        "binary_operator" => {
            let mut cursor = node.walk();
            let types = node
                .named_children(&mut cursor)
                .filter_map(|child| python_annotation_type(child, source))
                .collect::<Vec<_>>();
            match types.as_slice() {
                [single] => Some(single.clone()),
                _ => None,
            }
        }
        // `Optional[T]` / `Union[T, None]` give `T`; `list[str]` gives `list`.
        "generic_type" | "subscript" => {
            let mut cursor = node.walk();
            let children = node.named_children(&mut cursor).collect::<Vec<_>>();
            let base = children.first()?;
            let base_name = python_annotation_type(*base, source)?;
            let wrapper = base_name.rsplit('.').next().unwrap_or(&base_name);
            if matches!(wrapper, "Optional" | "Union") {
                let arguments = children[1..]
                    .iter()
                    .flat_map(|child| {
                        if child.kind() == "type_parameter" {
                            let mut inner = child.walk();
                            child.named_children(&mut inner).collect::<Vec<_>>()
                        } else {
                            vec![*child]
                        }
                    })
                    .filter_map(|child| python_annotation_type(child, source))
                    .collect::<Vec<_>>();
                return match arguments.as_slice() {
                    [single] => Some(single.clone()),
                    _ => None,
                };
            }
            Some(base_name)
        }
        _ => None,
    }
}

/// `(name, annotated type)` of each parameter of a `parameters` node.
pub(super) fn python_parameter_types(
    parameters: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<(String, String)> {
    let mut types = Vec::new();
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !matches!(
            parameter.kind(),
            "typed_parameter" | "typed_default_parameter"
        ) {
            continue;
        }
        let name = parameter.child_by_field_name("name").or_else(|| {
            let mut inner = parameter.walk();
            parameter
                .named_children(&mut inner)
                .find(|child| child.kind() == "identifier")
        });
        let annotation = parameter
            .child_by_field_name("type")
            .and_then(|annotation| python_annotation_type(annotation, source));
        if let (Some(name), Some(annotation)) = (name, annotation) {
            types.push((node_text(name, source), annotation));
        }
    }
    types
}

/// Class name -> attribute -> type, for every class of the file.
pub(super) type AttributeTypes = HashMap<String, HashMap<String, String>>;

pub(super) fn collect_python_attribute_types(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> AttributeTypes {
    let mut types = AttributeTypes::new();
    collect_classes(root, source, &mut types);
    types
}

fn collect_classes(node: tree_sitter::Node<'_>, source: &[u8], types: &mut AttributeTypes) {
    if node.kind() == "class_definition"
        && let Some(name) = node.child_by_field_name("name")
        && let Some(body) = node.child_by_field_name("body")
    {
        let mut attributes = HashMap::new();
        let mut cursor = body.walk();
        for statement in body.named_children(&mut cursor) {
            // `store: GraphStore` in the class body (dataclasses, attrs).
            let assignment = if statement.kind() == "expression_statement" {
                statement.named_child(0)
            } else {
                Some(statement)
            };
            if let Some(assignment) = assignment
                && assignment.kind() == "assignment"
                && let Some(left) = assignment.child_by_field_name("left")
                && left.kind() == "identifier"
                && let Some(annotation) = assignment
                    .child_by_field_name("type")
                    .and_then(|annotation| python_annotation_type(annotation, source))
            {
                attributes.insert(node_text(left, source), annotation);
            }
        }
        let mut cursor = body.walk();
        for statement in body.named_children(&mut cursor) {
            let function = if statement.kind() == "decorated_definition" {
                statement.child_by_field_name("definition")
            } else {
                Some(statement)
            };
            if let Some(function) = function.filter(|f| f.kind() == "function_definition") {
                let parameters = function
                    .child_by_field_name("parameters")
                    .map(|parameters| python_parameter_types(parameters, source))
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<HashMap<_, _>>();
                if let Some(function_body) = function.child_by_field_name("body") {
                    collect_self_assignments(function_body, source, &parameters, &mut attributes);
                }
            }
        }
        types
            .entry(node_text(name, source))
            .or_default()
            .extend(attributes);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_classes(child, source, types);
    }
}

/// `self.x: T = ...`, `self.x = T(...)`, `self.x = param` (annotated).
fn collect_self_assignments(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    parameters: &HashMap<String, String>,
    attributes: &mut HashMap<String, String>,
) {
    if node.kind() == "assignment"
        && let Some(left) = node.child_by_field_name("left")
        && left.kind() == "attribute"
        && left
            .child_by_field_name("object")
            .is_some_and(|object| node_text(object, source) == "self")
        && let Some(attribute) = left.child_by_field_name("attribute")
    {
        let annotated = node
            .child_by_field_name("type")
            .and_then(|annotation| python_annotation_type(annotation, source));
        let assigned =
            node.child_by_field_name("right")
                .and_then(|right| match right.kind() {
                    // A class is capitalized; `self.x = make()` says nothing.
                    "call" => right
                        .child_by_field_name("function")
                        .filter(|callee| matches!(callee.kind(), "identifier" | "attribute"))
                        .map(|callee| node_text(callee, source))
                        .filter(|callee| {
                            callee.rsplit('.').next().is_some_and(|name| {
                                name.starts_with(|c: char| c.is_ascii_uppercase())
                            })
                        }),
                    "identifier" => parameters.get(&node_text(right, source)).cloned(),
                    _ => None,
                });
        if let Some(type_name) = annotated.or(assigned) {
            attributes
                .entry(node_text(attribute, source))
                .or_insert(type_name);
        }
    }
    // Nested functions and classes have their own `self`.
    if matches!(node.kind(), "function_definition" | "class_definition") {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_self_assignments(child, source, parameters, attributes);
    }
}
