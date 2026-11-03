//! Framework wiring one file records for the request-boundary pass, which
//! joins it across files (`dagayn_graph`'s `request_boundary`).

use super::js_modules::{JavaScriptImported, JavaScriptParseContext};
use super::util::node_text;

/// The Hono apps the module declares at its top level: `export const app =
/// new Hono()`, with `Hono` imported from `hono` or a `hono/*` preset, also
/// through calls that return the app (`new Hono().basePath("/api")`).
/// `default` is among them when the module's default export is one of them
/// (`export default app`). The handlers a module registers on them
/// (`app.get(path, (c) => ..)`) are arrows with no node, so the File owns
/// their calls.
pub(super) fn javascript_hono_apps(
    root: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Vec<String> {
    let mut apps = Vec::new();
    let mut default_export = None;
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        let declaration = if statement.kind() == "export_statement" {
            if let Some(value) = statement.child_by_field_name("value") {
                default_export = Some(value);
                continue;
            }
            match statement.child_by_field_name("declaration") {
                Some(declaration) => declaration,
                None => continue,
            }
        } else {
            statement
        };
        if !matches!(
            declaration.kind(),
            "lexical_declaration" | "variable_declaration"
        ) {
            continue;
        }
        let mut declarators = declaration.walk();
        for declarator in declaration.named_children(&mut declarators) {
            let (Some(name), Some(value)) = (
                declarator.child_by_field_name("name"),
                declarator.child_by_field_name("value"),
            ) else {
                continue;
            };
            if name.kind() == "identifier" && is_hono_app(value, context) {
                apps.push(node_text(name, context.source));
            }
        }
    }
    if let Some(value) = default_export
        && value.kind() == "identifier"
        && apps.contains(&node_text(value, context.source))
    {
        apps.push("default".to_string());
    }
    apps
}

/// `new Hono()`, or a method call chained on it: Hono's `basePath`,
/// `route`, and route registrations return the app.
fn is_hono_app(value: tree_sitter::Node<'_>, context: &JavaScriptParseContext<'_>) -> bool {
    match value.kind() {
        "new_expression" => value
            .child_by_field_name("constructor")
            .is_some_and(|constructor| is_hono_class(constructor, context)),
        "call_expression" => value
            .child_by_field_name("function")
            .filter(|callee| callee.kind() == "member_expression")
            .and_then(|callee| callee.child_by_field_name("object"))
            .is_some_and(|object| is_hono_app(object, context)),
        "parenthesized_expression" => value
            .named_child(0)
            .is_some_and(|inner| is_hono_app(inner, context)),
        _ => false,
    }
}

fn is_hono_class(constructor: tree_sitter::Node<'_>, context: &JavaScriptParseContext<'_>) -> bool {
    constructor.kind() == "identifier"
        && context
            .import_map
            .get(&node_text(constructor, context.source))
            .is_some_and(|binding| {
                (binding.module == "hono" || binding.module.starts_with("hono/"))
                    && binding.imported == JavaScriptImported::Named("Hono".to_string())
            })
}
