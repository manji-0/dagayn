//! Per-module export indexes: ES exports and re-exports, default exports, CommonJS module.exports / exports.x, and declaration files, cached per module file.

use super::*;

/// The export index of a resolved module file (cached).
pub(crate) fn javascript_module_index(
    module_file: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<JavaScriptExportIndex> {
    javascript_export_index(module_file, context.repo_root, context.caches)
}

pub(super) fn javascript_export_index(
    module_file: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<JavaScriptExportIndex> {
    if let Some(cache) = caches.export
        && let Some(cached) = cache.borrow().get(module_file).cloned()
    {
        return cached;
    }
    let result = javascript_export_index_uncached(module_file, repo_root, caches);
    if let Some(cache) = caches.export {
        cache
            .borrow_mut()
            .insert(module_file.to_string(), result.clone());
    }
    result
}

fn javascript_export_index_uncached(
    module_file: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<JavaScriptExportIndex> {
    let source_path = repo_root
        .map(|root| root.join(module_file))
        .unwrap_or_else(|| PathBuf::from(module_file));
    let source = std::fs::read(&source_path).ok()?;
    let mut parser = new_javascript_module_parser(module_file)?;
    let tree = parser.parse(&source, None)?;
    let root = tree.root_node();

    let mut import_map = JavaScriptImportMap::new();
    collect_javascript_import_map(root, &source, &mut import_map);
    let mut defined_names = HashSet::new();
    collect_javascript_defined_names(root, &source, &import_map, &mut defined_names);
    let mut named_exports = HashMap::new();
    let mut star_exports = Vec::new();
    let mut exported_declarations = HashSet::new();
    let mut commonjs_value = false;
    let resolve =
        |specifier: &str| resolve_javascript_module(specifier, module_file, repo_root, caches);

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if javascript_is_commonjs_module_file(module_file) && child.kind() == "expression_statement"
        {
            commonjs_value |= collect_javascript_commonjs_export(
                child,
                &source,
                module_file,
                &resolve,
                &mut named_exports,
            );
            continue;
        }
        if child.kind() != "export_statement" {
            continue;
        }
        if let Some(target) =
            javascript_default_export_target(child, &source, &defined_names, &import_map)
        {
            // TypeScript `export = main;`
            commonjs_value |= child
                .children(&mut child.walk())
                .any(|token| token.kind() == "=");
            match target {
                JavaScriptDefaultExport::Anonymous => {
                    defined_names.insert("default".to_string());
                }
                JavaScriptDefaultExport::Named(name) => {
                    named_exports
                        .insert("default".to_string(), JavaScriptExportTarget::Local(name));
                }
            }
            continue;
        }
        if let Some(declaration) = child.child_by_field_name("declaration") {
            collect_javascript_declaration_names(declaration, &source, &mut exported_declarations);
            continue;
        }
        let parts = javascript_export_statement_parts(child, &source);
        let resolved_module = parts.target_module.as_deref().and_then(&resolve);
        if let Some(export_clause) = parts.export_clause {
            let mut clause_cursor = export_clause.walk();
            for spec in export_clause.children(&mut clause_cursor) {
                if spec.kind() != "export_specifier" {
                    continue;
                }
                let Some(original_name) = spec
                    .child_by_field_name("name")
                    .map(|name| javascript_module_export_name(name, &source))
                else {
                    continue;
                };
                let exported_name = spec
                    .child_by_field_name("alias")
                    .map(|alias| javascript_module_export_name(alias, &source))
                    .unwrap_or_else(|| original_name.clone());
                let target = match (&parts.target_module, &resolved_module) {
                    (None, _) => JavaScriptExportTarget::Local(original_name),
                    (Some(_), Some(resolved_module)) => JavaScriptExportTarget::External {
                        module_file: resolved_module.clone(),
                        symbol_name: original_name,
                    },
                    (Some(_), None) => continue,
                };
                named_exports.insert(exported_name, target);
            }
        }
        let Some(resolved_module) = resolved_module else {
            continue;
        };
        if let Some(namespace) = parts.namespace_export {
            // `export * as ns from "./m"`: `ns` is the module object of `m`.
            named_exports.insert(
                namespace,
                JavaScriptExportTarget::Namespace(resolved_module),
            );
        } else if parts.has_star_export {
            star_exports.push(resolved_module);
        }
    }
    Some(JavaScriptExportIndex {
        defined_names,
        exported_declarations,
        declaration_file: javascript_is_declaration_file(module_file),
        named_exports,
        star_exports,
        commonjs_value,
        class_table: Arc::new(collect_javascript_class_table(root, &source)),
        member_paths: Arc::new(
            super::super::js_objects::collect_javascript_member_paths(root, &source).members,
        ),
        import_map: Arc::new(import_map),
        type_paths: Arc::new(super::super::js_members::collect_javascript_type_paths(
            root, &source,
        )),
    })
}

enum JavaScriptDefaultExport {
    /// `export default function () {}` / `class {}` / `{ ... }` / `() => ...`:
    /// the extractor names the node `default`.
    Anonymous,
    /// `export default function Page() {}` or `export default impl;`.
    Named(String),
}

/// The target of an `export default` statement, if `node` is one.
fn javascript_default_export_target(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    defined_names: &HashSet<String>,
    import_map: &JavaScriptImportMap,
) -> Option<JavaScriptDefaultExport> {
    let known = |name: &String| defined_names.contains(name) || import_map.contains_key(name);
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    // TypeScript `export = main;` is the module's default for ES imports.
    if let Some(position) = children.iter().position(|child| child.kind() == "=") {
        let value = children[position + 1..]
            .iter()
            .find(|child| child.is_named())?;
        let name = node_text(*value, source);
        return (value.kind() == "identifier" && known(&name))
            .then_some(JavaScriptDefaultExport::Named(name));
    }
    if !children.iter().any(|child| child.kind() == "default") {
        return None;
    }
    if let Some(declaration) = node.child_by_field_name("declaration") {
        return declaration
            .child_by_field_name("name")
            .map(|name| JavaScriptDefaultExport::Named(node_text(name, source)));
    }
    let value = node.child_by_field_name("value")?;
    match value.kind() {
        // `export default impl;`, including an imported `impl` (followed
        // to its origin like any other local re-export).
        "identifier" => {
            let name = node_text(value, source);
            known(&name).then_some(JavaScriptDefaultExport::Named(name))
        }
        "class" | "function_expression" | "function" | "generator_function" => Some(
            value
                .child_by_field_name("name")
                .map(|name| JavaScriptDefaultExport::Named(node_text(name, source)))
                .unwrap_or(JavaScriptDefaultExport::Anonymous),
        ),
        "arrow_function" | "object" | "as_expression" | "satisfies_expression" => {
            Some(JavaScriptDefaultExport::Anonymous)
        }
        // `export default memo(function Page() {})`: the node is `default`.
        "call_expression" if javascript_wrapped_function(value, source, import_map).is_some() => {
            Some(JavaScriptDefaultExport::Anonymous)
        }
        _ => None,
    }
}

fn javascript_is_declaration_file(module_file: &str) -> bool {
    [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| ends_with_ascii_ignore_case(module_file, suffix))
}

/// Files whose top-level `module.exports` / `exports.x` assignments are
/// read as CommonJS exports.
fn javascript_is_commonjs_module_file(module_file: &str) -> bool {
    [".js", ".jsx", ".cjs"]
        .iter()
        .any(|suffix| ends_with_ascii_ignore_case(module_file, suffix))
}

/// Names bound by the declaration of an `export <declaration>` statement.
fn collect_javascript_declaration_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for declarator in node.children(&mut cursor) {
                if declarator.kind() == "variable_declarator"
                    && let Some(name) = declarator.child_by_field_name("name")
                    && name.kind() == "identifier"
                {
                    names.insert(node_text(name, source));
                }
            }
        }
        "internal_module" | "module" => {
            if let Some(name) = node.child_by_field_name("name") {
                let root = match name.kind() {
                    "nested_identifier" => javascript_leftmost_segment(name, source),
                    "identifier" => Some(node_text(name, source)),
                    _ => None,
                };
                names.extend(root);
            }
        }
        // `export declare function f(): void;` / `export declare const x`.
        "ambient_declaration" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_javascript_declaration_names(child, source, names);
            }
        }
        _ => {
            if let Some(name) = node.child_by_field_name("name")
                && matches!(name.kind(), "identifier" | "type_identifier")
            {
                names.insert(node_text(name, source));
            }
        }
    }
}

/// Reads one top-level CommonJS export statement:
///
/// - `module.exports = { a, b: fn, c() {} }` exports `a`, `b` (-> `fn`),
///   and `c`; the object itself is the module's ES `default`
/// - `module.exports = X` makes `X` the `default`
/// - `module.exports.x = V` / `exports.x = V` export `x` (-> `V` when it is
///   an identifier); the exports object is the `default`
///
/// A `require("./m")` value is `m`'s module object. Other values (function
/// expressions, calls) keep the exported name itself as the target.
/// Returns whether the statement is a CommonJS export.
fn collect_javascript_commonjs_export(
    statement: tree_sitter::Node<'_>,
    source: &[u8],
    module_file: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    named_exports: &mut HashMap<String, JavaScriptExportTarget>,
) -> bool {
    let Some(assignment) = statement
        .named_child(0)
        .filter(|node| node.kind() == "assignment_expression")
    else {
        return false;
    };
    let (Some(left), Some(right)) = (
        assignment.child_by_field_name("left"),
        assignment.child_by_field_name("right"),
    ) else {
        return false;
    };
    let self_namespace = || JavaScriptExportTarget::Namespace(module_file.to_string());
    if javascript_is_module_exports(left, source) {
        if right.kind() == "object" {
            collect_javascript_commonjs_object_exports(right, source, resolve, named_exports);
            named_exports.insert("default".to_string(), self_namespace());
        } else if let Some(target) = javascript_commonjs_value_target(right, source, resolve) {
            named_exports.insert("default".to_string(), target);
        }
        return true;
    }
    let Some(name) = javascript_commonjs_export_property(left, source) else {
        return false;
    };
    let target = javascript_commonjs_value_target(right, source, resolve)
        .unwrap_or_else(|| JavaScriptExportTarget::Local(name.clone()));
    named_exports.insert(name, target);
    named_exports
        .entry("default".to_string())
        .or_insert_with(self_namespace);
    true
}

fn collect_javascript_commonjs_object_exports(
    object: tree_sitter::Node<'_>,
    source: &[u8],
    resolve: &dyn Fn(&str) -> Option<String>,
    named_exports: &mut HashMap<String, JavaScriptExportTarget>,
) {
    let mut cursor = object.walk();
    for member in object.named_children(&mut cursor) {
        let (name, target) = match member.kind() {
            "shorthand_property_identifier" => {
                let name = node_text(member, source);
                (name.clone(), JavaScriptExportTarget::Local(name))
            }
            "pair" => {
                let Some(name) = member
                    .child_by_field_name("key")
                    .and_then(|key| javascript_property_key_name(key, source))
                else {
                    continue;
                };
                let target = member
                    .child_by_field_name("value")
                    .and_then(|value| javascript_commonjs_value_target(value, source, resolve))
                    .unwrap_or_else(|| JavaScriptExportTarget::Local(name.clone()));
                (name, target)
            }
            "method_definition" => {
                let Some(name) = member
                    .child_by_field_name("name")
                    .and_then(|key| javascript_property_key_name(key, source))
                else {
                    continue;
                };
                (name.clone(), JavaScriptExportTarget::Local(name))
            }
            _ => continue,
        };
        named_exports.insert(name, target);
    }
}

/// `module.exports`.
fn javascript_is_module_exports(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    node.kind() == "member_expression"
        && node.child_by_field_name("object").is_some_and(|object| {
            object.kind() == "identifier" && node_text(object, source) == "module"
        })
        && node
            .child_by_field_name("property")
            .is_some_and(|property| node_text(property, source) == "exports")
}

/// `x` of `module.exports.x` / `exports.x`.
fn javascript_commonjs_export_property(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    if node.kind() != "member_expression" {
        return None;
    }
    let object = node.child_by_field_name("object")?;
    let property = node.child_by_field_name("property")?;
    let exports_object = (object.kind() == "identifier" && node_text(object, source) == "exports")
        || javascript_is_module_exports(object, source);
    (exports_object && property.kind() == "property_identifier")
        .then(|| node_text(property, source))
}

/// The export target of a CommonJS export value: an identifier, or the
/// module object of `require("./m")`.
fn javascript_commonjs_value_target(
    value: tree_sitter::Node<'_>,
    source: &[u8],
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Option<JavaScriptExportTarget> {
    match value.kind() {
        "identifier" => Some(JavaScriptExportTarget::Local(node_text(value, source))),
        "call_expression" => {
            let specifier = javascript_require_specifier(value, source)?;
            let module = resolve(&specifier)?;
            Some(JavaScriptExportTarget::Namespace(module))
        }
        _ => None,
    }
}

/// A static object key: `a`, `"a"`, `'a'`.
pub(super) fn javascript_property_key_name(
    key: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    match key.kind() {
        "property_identifier" | "identifier" => Some(node_text(key, source)),
        "string" => Some(decode_javascript_string_literal(key, source)),
        _ => None,
    }
}

/// Text of an import/export specifier name: identifiers, the `default`
/// keyword, or a string literal (`export { x as "y" }`).
pub(super) fn javascript_module_export_name(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if node.kind() == "string" {
        decode_javascript_string_literal(node, source)
    } else {
        node_text(node, source)
    }
}

fn new_javascript_module_parser(module_file: &str) -> Option<tree_sitter::Parser> {
    if ends_with_ascii_ignore_case(module_file, ".tsx") {
        return new_tsx_parser();
    }
    if ends_with_ascii_ignore_case(module_file, ".ts")
        || ends_with_ascii_ignore_case(module_file, ".mts")
        || ends_with_ascii_ignore_case(module_file, ".cts")
    {
        return new_typescript_parser();
    }
    if ends_with_ascii_ignore_case(module_file, ".js")
        || ends_with_ascii_ignore_case(module_file, ".jsx")
        || ends_with_ascii_ignore_case(module_file, ".mjs")
        || ends_with_ascii_ignore_case(module_file, ".cjs")
    {
        return new_javascript_parser();
    }
    None
}

/// The pieces of a (non-default) `export` statement.
struct JavaScriptExportStatementParts<'a> {
    /// `{ a, b as c }`.
    export_clause: Option<tree_sitter::Node<'a>>,
    /// The `from "..."` specifier.
    target_module: Option<String>,
    /// `export * from`.
    has_star_export: bool,
    /// `ns` of `export * as ns from`.
    namespace_export: Option<String>,
}

fn javascript_export_statement_parts<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> JavaScriptExportStatementParts<'a> {
    let mut parts = JavaScriptExportStatementParts {
        export_clause: None,
        target_module: None,
        has_star_export: false,
        namespace_export: None,
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "export_clause" => parts.export_clause = Some(child),
            "string" => parts.target_module = Some(decode_javascript_string_literal(child, source)),
            "*" => parts.has_star_export = true,
            "namespace_export" => {
                let mut inner = child.walk();
                parts.namespace_export = child
                    .named_children(&mut inner)
                    .last()
                    .map(|name| javascript_module_export_name(name, source));
            }
            _ => {}
        }
    }
    parts
}
