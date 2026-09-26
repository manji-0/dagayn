use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::js_members::{JavaScriptClassTable, collect_javascript_class_table};
use super::js_resolve::resolve_javascript_module;
use super::member_calls::MemberCallBindings;
use super::parsers::{new_javascript_parser, new_tsx_parser, new_typescript_parser};
use super::qualify;
use super::util::{ends_with_ascii_ignore_case, node_text};

pub(super) type JavaScriptExportCache = RefCell<HashMap<String, Option<JavaScriptExportIndex>>>;
pub(super) type JavaScriptModuleCache = RefCell<HashMap<(String, String), Option<String>>>;
pub(super) type JavaScriptTsconfigCache = RefCell<HashMap<PathBuf, Option<(PathBuf, Value)>>>;

#[derive(Clone, Copy, Default)]
pub(super) struct JavaScriptCaches<'a> {
    pub(super) export: Option<&'a JavaScriptExportCache>,
    pub(super) module: Option<&'a JavaScriptModuleCache>,
    pub(super) tsconfig: Option<&'a JavaScriptTsconfigCache>,
}

#[derive(Clone)]
pub(super) struct JavaScriptExportIndex {
    defined_names: HashSet<String>,
    /// Names declared with `export` (`export function f`, `export const x`,
    /// `export declare ...`): what an `export *` of this module provides
    /// besides `named_exports`.
    exported_declarations: HashSet<String>,
    /// `.d.ts`: every top-level declaration counts as exported.
    declaration_file: bool,
    named_exports: HashMap<String, JavaScriptExportTarget>,
    star_exports: Vec<String>,
    /// The module assigns `module.exports` / `exports.x` (CommonJS) or has a
    /// TypeScript `export =`: `require` returns its `default`.
    commonjs_value: bool,
    /// Class / interface shapes declared in the module, for member calls on
    /// receivers of an imported type.
    pub(super) class_table: Arc<JavaScriptClassTable>,
    /// Object-container and namespace member paths of the module
    /// (`api.get`, `Outer.helper`).
    pub(super) member_paths: Arc<HashSet<String>>,
    /// The module's own imports, to resolve type names written in it (the
    /// base of an imported class, the type of its fields).
    pub(super) import_map: Arc<JavaScriptImportMap>,
    /// Owner paths of the module's type declarations (classes, interfaces,
    /// enums, type aliases), the targets of type references into it.
    pub(super) type_paths: Arc<HashSet<String>>,
}

impl JavaScriptExportIndex {
    /// Whether the module declares a function, class, or namespace `name`
    /// at module scope.
    pub(super) fn declares(&self, name: &str) -> bool {
        self.defined_names.contains(name)
    }

    fn has_default_export(&self) -> bool {
        self.defined_names.contains("default") || self.named_exports.contains_key("default")
    }

    /// Whether `export * from` this module provides `name` directly
    /// (declared or listed as an export here, not through its own stars).
    fn exports_directly(&self, name: &str) -> bool {
        self.named_exports.contains_key(name)
            || self.exported_declarations.contains(name)
            || (self.declaration_file && self.defined_names.contains(name))
    }
}

#[derive(Clone)]
pub(super) enum JavaScriptExportTarget {
    /// A name of the module itself: a declaration, or an import it
    /// re-exports (`import { a } from "./x"; export { a as b }`).
    Local(String),
    External {
        module_file: String,
        symbol_name: String,
    },
    /// A module object: `export * as ns from "./m"`, CommonJS
    /// `exports.x = require("./m")`, or the exports object of a CommonJS
    /// module itself (its ES `default`).
    Namespace(String),
}

/// What an exported name (or an import binding) resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum JavaScriptExportResolution {
    /// The declaration's QN, or the best-effort `module::name`.
    Symbol(String),
    /// A module object (by module file) whose members are its exports.
    Namespace(String),
}

impl JavaScriptExportResolution {
    fn into_symbol(self) -> Option<String> {
        match self {
            Self::Symbol(symbol) => Some(symbol),
            Self::Namespace(_) => None,
        }
    }
}

/// What an import binds a local name to: the exporting module and the
/// export it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct JavaScriptImportBinding {
    pub(super) module: String,
    pub(super) imported: JavaScriptImported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum JavaScriptImported {
    /// `import { a }` / `import { a as b }`: the exported name `a`.
    Named(String),
    /// `import X from` / `import { default as X }`.
    Default,
    /// `import * as ns from`.
    Namespace,
    /// `const x = require("./m")` / `import x = require("./m")`: what
    /// `require` returns, the module's CommonJS value (`module.exports`,
    /// TypeScript `export =`) when it has one, otherwise its ES module
    /// namespace object.
    Require,
}

pub(super) type JavaScriptImportMap = HashMap<String, JavaScriptImportBinding>;

pub(super) struct JavaScriptParseContext<'a> {
    pub(super) source: &'a [u8],
    pub(super) file_path: crate::core::types::FilePath,
    pub(super) language: &'static str,
    pub(super) test_file: bool,
    pub(super) defined_names: &'a HashSet<String>,
    pub(super) import_map: &'a JavaScriptImportMap,
    /// Imported specifiers that name external packages -> package name
    /// ([`collect_javascript_external_packages`]).
    pub(super) external_packages: &'a HashMap<String, String>,
    /// Owner paths of object-container and namespace members (`api.get`,
    /// `api.nested.deep`, `Outer.helper`, `Outer.Inner`, `A.B.C.abc`).
    pub(super) member_paths: &'a HashSet<String>,
    /// Owner paths of namespaces and ambient modules (`Outer`, `A.B`,
    /// `global`): containers whose members do not see a `this`.
    pub(super) namespace_paths: &'a HashSet<String>,
    /// Class / interface shapes declared in this file.
    pub(super) class_table: &'a JavaScriptClassTable,
    /// Owner paths of the type declarations of this file
    /// ([`super::js_members::collect_javascript_type_paths`]).
    pub(super) type_paths: &'a HashSet<String>,
    /// Nesting depth of type subtrees being walked: references inside a
    /// type already collected from its outermost node are not collected
    /// again.
    pub(super) type_depth: Cell<usize>,
    /// Local names exported by a module-level `export { name }` clause.
    pub(super) exported_names: &'a HashSet<String>,
    /// `.d.ts` / `.d.mts` / `.d.cts`: every declaration is ambient.
    pub(super) declaration_file: bool,
    /// Nesting depth of `declare ...` / ambient-module bodies being walked.
    pub(super) ambient_depth: Cell<usize>,
    /// Local declarations of the function bodies being walked, innermost
    /// last; calls of these names stay internal to the function.
    pub(super) local_scopes: RefCell<Vec<HashSet<String>>>,
    pub(super) repo_root: Option<&'a Path>,
    pub(super) caches: JavaScriptCaches<'a>,
    pub(super) bindings: RefCell<MemberCallBindings>,
}

pub(super) fn collect_javascript_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &JavaScriptImportMap,
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" | "class" | "interface_declaration" => {
            if let Some(name) = javascript_class_like_name(node, source) {
                names.insert(name);
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            if let Some(name) = javascript_function_name(node, source) {
                names.insert(name);
            }
        }
        // `namespace Outer {}` defines `Outer`; its members are reachable
        // only as `Outer.x`, so the body is not a source of module names.
        "internal_module" | "module" => {
            if let Some(name) = node.child_by_field_name("name") {
                let root = match name.kind() {
                    "nested_identifier" => javascript_leftmost_segment(name, source),
                    "identifier" => Some(node_text(name, source)),
                    _ => None,
                };
                if let Some(root) = root {
                    names.insert(root);
                }
            }
            return;
        }
        "ambient_declaration"
            if node
                .children(&mut node.walk())
                .any(|child| child.kind() == "global") =>
        {
            return;
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for declarator in node.children(&mut cursor) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) =
                    javascript_variable_declarator_function_name(declarator, source, import_map)
                {
                    names.insert(name);
                }
            }
        }
        _ => {}
    }
    if javascript_is_function_scope(node) {
        // Declarations inside function bodies are locals, not module names.
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_defined_names(child, source, import_map, names);
    }
}

/// Function-like nodes whose bodies hold local declarations.
pub(super) fn javascript_is_function_scope(node: tree_sitter::Node<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "function"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
            | "class_static_block"
    )
}

pub(super) fn collect_javascript_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "class_declaration"
        | "abstract_class_declaration"
        | "class"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => {
            if let Some(name) = javascript_class_like_name(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    if javascript_is_function_scope(node) {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_type_names(child, source, names);
    }
}

/// First segment of `A.B.C` (a `nested_identifier`).
fn javascript_leftmost_segment(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut current = node;
    while matches!(current.kind(), "nested_identifier" | "member_expression") {
        current = current
            .child_by_field_name("object")
            .or_else(|| current.named_child(0))?;
    }
    (current.kind() == "identifier").then(|| node_text(current, source))
}

/// Name under which a class-like declaration is reachable: the declared
/// name, or the binding for `const X = class [Inner] {}`.
fn javascript_class_like_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "class"
        && let Some(parent) = node.parent()
        && parent.kind() == "variable_declarator"
    {
        let binding = parent.child_by_field_name("name")?;
        return (binding.kind() == "identifier").then(|| node_text(binding, source));
    }
    javascript_named_child(node, source, &["identifier", "type_identifier"])
}

pub(super) fn resolve_javascript_call_target(
    name: &str,
    context: &JavaScriptParseContext<'_>,
) -> String {
    if context.defined_names.contains(name) {
        return qualify(&context.file_path, name, None);
    }
    let Some(binding) = context.import_map.get(name) else {
        return name.to_string();
    };
    resolve_javascript_import_binding(name, binding, context)
        .or_else(|| javascript_external_symbol(name, &[], context))
        .unwrap_or_else(|| name.to_string())
}

/// `pkg::path` for a use of the import binding `root` followed by `members`
/// (`useState` -> `react::useState`, `fs.readFile` -> `node:fs::readFile`,
/// `z.object` -> `zod::z.object`), keeping the specifier as written so the
/// target matches the file's `IMPORTS_FROM` edge. A default or `require`
/// binding used alone is `pkg::default`; its members are the module's own
/// (CommonJS interop, like in-repo default imports). A namespace binding is
/// not callable by itself.
pub(super) fn javascript_external_symbol(
    root: &str,
    members: &[&str],
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let binding = context.import_map.get(root)?;
    if !context.external_packages.contains_key(&binding.module) {
        return None;
    }
    let path = match &binding.imported {
        JavaScriptImported::Named(name) => std::iter::once(name.as_str())
            .chain(members.iter().copied())
            .collect::<Vec<_>>()
            .join("."),
        JavaScriptImported::Default | JavaScriptImported::Require if members.is_empty() => {
            "default".to_string()
        }
        JavaScriptImported::Namespace if members.is_empty() => return None,
        _ => members.join("."),
    };
    Some(format!("{}::{path}", binding.module))
}

/// Resolves a local import binding to the exporting declaration's QN.
///
/// Named imports look up the exported name (not the local alias). Default
/// imports look up the module's `default` export; a module with no default
/// export at all falls back to the importer's local name, which keeps
/// bundler-interop code resolvable. Namespace bindings are not callable
/// symbols and stay unresolved.
pub(super) fn resolve_javascript_import_binding(
    local_name: &str,
    binding: &JavaScriptImportBinding,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    resolve_javascript_import_binding_in(
        &context.file_path,
        local_name,
        binding,
        context.repo_root,
        context.caches,
    )
}

/// [`resolve_javascript_import_binding`] for an import written in
/// `importer` (any module, not only the file being parsed).
pub(super) fn resolve_javascript_import_binding_in(
    importer: &str,
    local_name: &str,
    binding: &JavaScriptImportBinding,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    resolve_javascript_import_binding_target_in(
        importer,
        local_name,
        binding,
        repo_root,
        caches,
        &mut HashSet::new(),
    )?
    .into_symbol()
}

/// What an import binding written in `importer` names: a declaration, or a
/// module object (`import * as ns`, a re-exported namespace, the exports
/// object of a CommonJS module).
fn resolve_javascript_import_binding_target_in(
    importer: &str,
    local_name: &str,
    binding: &JavaScriptImportBinding,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
    seen: &mut HashSet<(String, String)>,
) -> Option<JavaScriptExportResolution> {
    let module_file = resolve_javascript_module(&binding.module, importer, repo_root, caches)?;
    let symbol = match &binding.imported {
        JavaScriptImported::Named(exported) => exported.as_str(),
        JavaScriptImported::Default => "default",
        JavaScriptImported::Namespace => {
            return Some(JavaScriptExportResolution::Namespace(module_file));
        }
        JavaScriptImported::Require => {
            if javascript_export_index(&module_file, repo_root, caches)
                .is_some_and(|index| index.commonjs_value)
                && let Some(target) =
                    resolve_javascript_export(&module_file, "default", repo_root, caches, seen)
            {
                return Some(target);
            }
            return Some(JavaScriptExportResolution::Namespace(module_file));
        }
    };
    if let Some(target) = resolve_javascript_export(&module_file, symbol, repo_root, caches, seen) {
        return Some(target);
    }
    let fallback = if binding.imported == JavaScriptImported::Default {
        if javascript_export_index(&module_file, repo_root, caches)
            .is_some_and(|index| !index.has_default_export())
        {
            if let Some(target) =
                resolve_javascript_export(&module_file, local_name, repo_root, caches, seen)
            {
                return Some(target);
            }
            local_name
        } else {
            "default"
        }
    } else {
        symbol
    };
    Some(JavaScriptExportResolution::Symbol(qualify(
        &module_file,
        fallback,
        None,
    )))
}

/// Walks `root.a.b` written in `importer`, where `root` is an import
/// binding: while the value is a module object (`import * as ns`,
/// `export * as ns from`, a CommonJS exports object), the next segment is
/// looked up among that module's exports. A declaration stops the walk.
/// Returns the final value and the number of `segments` consumed; the
/// caller reads the rest as a member path of the declaration.
pub(super) fn resolve_javascript_import_path_in(
    importer: &str,
    root: &str,
    binding: &JavaScriptImportBinding,
    segments: &[&str],
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<(JavaScriptExportResolution, usize)> {
    let mut current = resolve_javascript_import_binding_target_in(
        importer,
        root,
        binding,
        repo_root,
        caches,
        &mut HashSet::new(),
    )?;
    let mut consumed = 0;
    while let JavaScriptExportResolution::Namespace(module_file) = &current {
        let Some(segment) = segments.get(consumed) else {
            break;
        };
        current =
            resolve_javascript_export(module_file, segment, repo_root, caches, &mut HashSet::new())
                .unwrap_or_else(|| {
                    JavaScriptExportResolution::Symbol(qualify(module_file, segment, None))
                });
        consumed += 1;
    }
    Some((current, consumed))
}

/// `ns.member` where `ns` is a namespace (or default) import binding, or a
/// named import of a re-exported namespace: the member exported by that
/// module.
pub(super) fn resolve_javascript_namespace_member(
    root: &str,
    member: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let binding = context.import_map.get(root)?;
    let target = resolve_javascript_import_binding_target_in(
        &context.file_path,
        root,
        binding,
        context.repo_root,
        context.caches,
        &mut HashSet::new(),
    );
    match target {
        Some(JavaScriptExportResolution::Namespace(module_file)) => {
            resolve_javascript_module_symbol(
                &module_file,
                member,
                context.repo_root,
                context.caches,
            )
        }
        // `<X.C />` / `extends X.C` on a default import: CommonJS interop.
        _ if binding.imported == JavaScriptImported::Default => {
            resolve_javascript_imported_symbol(member, &binding.module, context)
        }
        _ => None,
    }
}

pub(super) fn resolve_javascript_imported_symbol(
    symbol_name: &str,
    module: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let module_file = resolve_javascript_module(
        module,
        &context.file_path,
        context.repo_root,
        context.caches,
    )?;
    resolve_javascript_module_symbol(&module_file, symbol_name, context.repo_root, context.caches)
}

/// `symbol_name` exported by `module_file` as a declaration QN: the origin
/// it resolves to, `module::name` when unresolved, `None` when it names a
/// module object.
fn resolve_javascript_module_symbol(
    module_file: &str,
    symbol_name: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    match resolve_javascript_export(
        module_file,
        symbol_name,
        repo_root,
        caches,
        &mut HashSet::new(),
    ) {
        Some(resolution) => resolution.into_symbol(),
        None => Some(qualify(module_file, symbol_name, None)),
    }
}

/// `symbol_name` as `module_file` exports it, following re-exports to the
/// origin. The module's own declarations resolve even when they are not
/// exported (a lenient lookup that keeps bundler-style code resolvable);
/// what `export *` sources contribute follows ES semantics
/// ([`resolve_javascript_declared_export`]). `seen` breaks cycles.
fn resolve_javascript_export(
    module_file: &str,
    symbol_name: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
    seen: &mut HashSet<(String, String)>,
) -> Option<JavaScriptExportResolution> {
    if !seen.insert((module_file.to_string(), symbol_name.to_string())) {
        return None;
    }
    let index = javascript_export_index(module_file, repo_root, caches)?;
    if index.defined_names.contains(symbol_name) {
        return Some(JavaScriptExportResolution::Symbol(qualify(
            module_file,
            symbol_name,
            None,
        )));
    }
    resolve_javascript_declared_export(module_file, &index, symbol_name, repo_root, caches, seen)
}

/// `symbol_name` among what `module_file` actually exports: explicit
/// exports first (they shadow star exports), then `export *` sources.
/// `export *` never re-exports `default`, and a name that several star
/// sources export as different bindings is ambiguous and not exported.
fn resolve_javascript_declared_export(
    module_file: &str,
    index: &JavaScriptExportIndex,
    symbol_name: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
    seen: &mut HashSet<(String, String)>,
) -> Option<JavaScriptExportResolution> {
    if let Some(target) = index.named_exports.get(symbol_name) {
        return Some(resolve_javascript_export_target(
            module_file,
            index,
            target,
            repo_root,
            caches,
            seen,
        ));
    }
    if index.exports_directly(symbol_name) {
        return Some(JavaScriptExportResolution::Symbol(qualify(
            module_file,
            symbol_name,
            None,
        )));
    }
    if symbol_name == "default" {
        return None;
    }
    let mut found = None;
    for star_module in &index.star_exports {
        // Each source walks with its own copy of the visited set, so two
        // sources re-exporting one origin agree instead of the second one
        // stopping early at a shared intermediate module.
        let mut seen = seen.clone();
        if !seen.insert((star_module.clone(), symbol_name.to_string())) {
            continue;
        }
        let Some(star_index) = javascript_export_index(star_module, repo_root, caches) else {
            continue;
        };
        let Some(result) = resolve_javascript_declared_export(
            star_module,
            &star_index,
            symbol_name,
            repo_root,
            caches,
            &mut seen,
        ) else {
            continue;
        };
        match &found {
            None => found = Some(result),
            Some(existing) if *existing == result => {}
            Some(_) => return None,
        }
    }
    found
}

fn resolve_javascript_export_target(
    module_file: &str,
    index: &JavaScriptExportIndex,
    target: &JavaScriptExportTarget,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
    seen: &mut HashSet<(String, String)>,
) -> JavaScriptExportResolution {
    match target {
        JavaScriptExportTarget::Local(original_name) => {
            // `import { a } from "./x"; export { a as b }` follows the import.
            if !index.defined_names.contains(original_name)
                && let Some(binding) = index.import_map.get(original_name)
                && let Some(resolved) = resolve_javascript_import_binding_target_in(
                    module_file,
                    original_name,
                    binding,
                    repo_root,
                    caches,
                    seen,
                )
            {
                return resolved;
            }
            JavaScriptExportResolution::Symbol(qualify(module_file, original_name, None))
        }
        JavaScriptExportTarget::External {
            module_file,
            symbol_name,
        } => resolve_javascript_export(module_file, symbol_name, repo_root, caches, seen)
            .unwrap_or_else(|| {
                JavaScriptExportResolution::Symbol(qualify(module_file, symbol_name, None))
            }),
        JavaScriptExportTarget::Namespace(module_file) => {
            JavaScriptExportResolution::Namespace(module_file.clone())
        }
    }
}

/// The export index of a resolved module file (cached).
pub(super) fn javascript_module_index(
    module_file: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<JavaScriptExportIndex> {
    javascript_export_index(module_file, context.repo_root, context.caches)
}

fn javascript_export_index(
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
            super::js_objects::collect_javascript_member_paths(root, &source).members,
        ),
        import_map: Arc::new(import_map),
        type_paths: Arc::new(super::js_members::collect_javascript_type_paths(
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

/// `"./m"` of `require("./m")`: a call of the bare identifier `require`
/// whose first argument is a string literal. Whether `require` is shadowed
/// is the caller's concern.
pub(super) fn javascript_require_specifier(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    let function = node.child_by_field_name("function")?;
    if node.kind() != "call_expression"
        || function.kind() != "identifier"
        || node_text(function, source) != "require"
    {
        return None;
    }
    javascript_first_string_argument(node, source)
}

/// `"./m"` of a dynamic `import("./m")` with a string literal.
pub(super) fn javascript_dynamic_import_specifier(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    let function = node.child_by_field_name("function")?;
    if node.kind() != "call_expression" || function.kind() != "import" {
        return None;
    }
    javascript_first_string_argument(node, source)
}

fn javascript_first_string_argument(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let specifier = arguments
        .named_children(&mut cursor)
        .next()
        .filter(|argument| argument.kind() == "string")?;
    let specifier = decode_javascript_string_literal(specifier, source);
    (!specifier.is_empty()).then_some(specifier)
}

/// `import x = require("./m")`: the local name and the specifier.
pub(super) fn javascript_import_equals(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, String)> {
    if node.kind() != "import_statement" {
        return None;
    }
    let mut cursor = node.walk();
    let clause = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "import_require_clause")?;
    let mut cursor = clause.walk();
    let children = clause.named_children(&mut cursor).collect::<Vec<_>>();
    let local = children.iter().find(|child| child.kind() == "identifier")?;
    let specifier = children.iter().find(|child| child.kind() == "string")?;
    let specifier = decode_javascript_string_literal(*specifier, source);
    (!specifier.is_empty()).then(|| (node_text(*local, source), specifier))
}

/// A static object key: `a`, `"a"`, `'a'`.
fn javascript_property_key_name(key: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match key.kind() {
        "property_identifier" | "identifier" => Some(node_text(key, source)),
        "string" => Some(decode_javascript_string_literal(key, source)),
        _ => None,
    }
}

/// Text of an import/export specifier name: identifiers, the `default`
/// keyword, or a string literal (`export { x as "y" }`).
fn javascript_module_export_name(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
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

pub(super) fn collect_javascript_import_map(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &mut JavaScriptImportMap,
) {
    if let Some((local, module)) = javascript_import_equals(node, source) {
        import_map.insert(
            local,
            JavaScriptImportBinding {
                module,
                imported: JavaScriptImported::Require,
            },
        );
    }
    if node.kind() == "variable_declarator" && javascript_is_module_scope_declarator(node) {
        collect_javascript_require_bindings(node, source, import_map);
    }
    if node.kind() == "import_statement"
        && let Some(module) = javascript_import_targets(node, source).into_iter().next()
    {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "import_clause" {
                collect_javascript_import_clause_names(child, source, &module, import_map);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_import_map(child, source, import_map);
    }
}

/// A declarator of a `const` / `let` / `var` statement at module scope
/// (`export const` included).
fn javascript_is_module_scope_declarator(node: tree_sitter::Node<'_>) -> bool {
    let Some(mut statement) = node.parent().filter(|parent| {
        matches!(
            parent.kind(),
            "lexical_declaration" | "variable_declaration"
        )
    }) else {
        return false;
    };
    if let Some(parent) = statement
        .parent()
        .filter(|parent| parent.kind() == "export_statement")
    {
        statement = parent;
    }
    statement
        .parent()
        .is_some_and(|parent| parent.kind() == "program")
}

/// Module-scope `require` bindings:
///
/// - `const m = require("./m")` binds `m` to what `require` returns
/// - `const { a, b: c } = require("./m")` binds `a` / `c` to the exports
///   `a` / `b`
/// - `const a = require("./m").b` binds `a` to the export `b`
fn collect_javascript_require_bindings(
    declarator: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &mut JavaScriptImportMap,
) {
    let (Some(name), Some(value)) = (
        declarator.child_by_field_name("name"),
        declarator.child_by_field_name("value"),
    ) else {
        return;
    };
    let (module, member) = match value.kind() {
        "call_expression" => (javascript_require_specifier(value, source), None),
        "member_expression" => {
            let property = value
                .child_by_field_name("property")
                .filter(|property| property.kind() == "property_identifier");
            let module = value
                .child_by_field_name("object")
                .and_then(|object| javascript_require_specifier(object, source));
            match property {
                Some(property) => (module, Some(node_text(property, source))),
                None => (None, None),
            }
        }
        _ => (None, None),
    };
    let Some(module) = module else {
        return;
    };
    let mut bind = |local: String, imported| {
        import_map.insert(
            local,
            JavaScriptImportBinding {
                module: module.clone(),
                imported,
            },
        );
    };
    match (name.kind(), member) {
        ("identifier", Some(member)) => {
            bind(node_text(name, source), JavaScriptImported::Named(member));
        }
        ("identifier", None) => bind(node_text(name, source), JavaScriptImported::Require),
        ("object_pattern", None) => {
            let mut cursor = name.walk();
            for property in name.named_children(&mut cursor) {
                match property.kind() {
                    "shorthand_property_identifier_pattern" => {
                        let local = node_text(property, source);
                        bind(local.clone(), JavaScriptImported::Named(local));
                    }
                    "pair_pattern" => {
                        let key = property
                            .child_by_field_name("key")
                            .and_then(|key| javascript_property_key_name(key, source));
                        let local = property
                            .child_by_field_name("value")
                            .filter(|value| value.kind() == "identifier")
                            .map(|value| node_text(value, source));
                        if let (Some(key), Some(local)) = (key, local) {
                            bind(local, JavaScriptImported::Named(key));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn collect_javascript_import_clause_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    module: &str,
    import_map: &mut JavaScriptImportMap,
) {
    let binding = |imported| JavaScriptImportBinding {
        module: module.to_string(),
        imported,
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                import_map.insert(
                    node_text(child, source),
                    binding(JavaScriptImported::Default),
                );
            }
            "namespace_import" => {
                if let Some(name) = javascript_last_named_descendant(
                    child,
                    source,
                    &["identifier", "property_identifier"],
                ) {
                    import_map.insert(name, binding(JavaScriptImported::Namespace));
                }
            }
            "named_imports" => collect_javascript_named_imports(child, source, module, import_map),
            _ => {}
        }
    }
}

fn collect_javascript_named_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    module: &str,
    import_map: &mut JavaScriptImportMap,
) {
    let mut cursor = node.walk();
    for spec in node.children(&mut cursor) {
        if spec.kind() != "import_specifier" {
            continue;
        }
        let Some(imported) = spec
            .child_by_field_name("name")
            .map(|name| javascript_module_export_name(name, source))
        else {
            continue;
        };
        let local = spec
            .child_by_field_name("alias")
            .map(|alias| node_text(alias, source))
            .unwrap_or_else(|| imported.clone());
        let imported = if imported == "default" {
            JavaScriptImported::Default
        } else {
            JavaScriptImported::Named(imported)
        };
        import_map.insert(
            local,
            JavaScriptImportBinding {
                module: module.to_string(),
                imported,
            },
        );
    }
}

fn javascript_last_named_descendant(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_last_named_descendant(child, source, kinds) {
            return Some(name);
        }
    }
    None
}

fn javascript_variable_declarator_function_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &JavaScriptImportMap,
) -> Option<String> {
    let mut name = None;
    let mut has_function = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" && name.is_none() {
            name = Some(node_text(child, source));
        } else if is_javascript_function_value(child.kind())
            || javascript_wrapped_function(child, source, import_map).is_some()
        {
            has_function = true;
        }
    }
    has_function.then_some(name).flatten()
}

/// A function literal wrapped in higher-order calls
/// (`memo(function Inner() {})`, `React.forwardRef((props, ref) => ...)`,
/// `memo(forwardRef(fn), areEqual)`, `observer(() => ...)`).
pub(super) struct JavaScriptWrappedFunction<'tree> {
    /// The inline function literal.
    pub(super) function: tree_sitter::Node<'tree>,
    /// The wrapper calls, outermost first.
    pub(super) calls: Vec<tree_sitter::Node<'tree>>,
}

impl JavaScriptWrappedFunction<'_> {
    /// Callee text of each wrapper, outermost first (`["memo", "forwardRef"]`).
    pub(super) fn wrapper_names(&self, source: &[u8]) -> Vec<String> {
        self.calls
            .iter()
            .filter_map(|call| call.child_by_field_name("function"))
            .map(|callee| node_text(callee, source))
            .collect()
    }
}

const JAVASCRIPT_MAX_WRAPPER_DEPTH: usize = 4;

/// Whether `value` is a wrapper call whose first argument is an inline
/// function literal, or another such wrapper call. A wrapper's callee is a
/// plain identifier (`memo`, `observer`, `debounce`) or a member of an
/// imported / required binding or of the `React` global (`React.memo`,
/// `mobx.observer`); a method of a local value (`items.map(x => ...)`) is
/// not a wrapper. Arguments after the first are ordinary module-scope code.
pub(super) fn javascript_wrapped_function<'tree>(
    value: tree_sitter::Node<'tree>,
    source: &[u8],
    import_map: &JavaScriptImportMap,
) -> Option<JavaScriptWrappedFunction<'tree>> {
    let mut calls = Vec::new();
    let mut current = value;
    while calls.len() < JAVASCRIPT_MAX_WRAPPER_DEPTH {
        if current.kind() != "call_expression"
            || !javascript_is_wrapper_callee(current, source, import_map)
        {
            return None;
        }
        let arguments = current.child_by_field_name("arguments")?;
        if arguments.kind() != "arguments" {
            return None;
        }
        let first = arguments
            .named_children(&mut arguments.walk())
            .find(|argument| argument.kind() != "comment")?;
        calls.push(current);
        if is_javascript_function_value(first.kind()) {
            return Some(JavaScriptWrappedFunction {
                function: first,
                calls,
            });
        }
        current = first;
    }
    None
}

fn javascript_is_wrapper_callee(
    call: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &JavaScriptImportMap,
) -> bool {
    let Some(callee) = call.child_by_field_name("function") else {
        return false;
    };
    match callee.kind() {
        "identifier" => node_text(callee, source) != "require",
        "member_expression" => {
            callee
                .child_by_field_name("property")
                .is_some_and(|property| property.kind() == "property_identifier")
                && callee
                    .child_by_field_name("object")
                    .filter(|object| object.kind() == "identifier")
                    .is_some_and(|object| {
                        let object = node_text(object, source);
                        object == "React" || import_map.contains_key(&object)
                    })
        }
        _ => false,
    }
}

fn is_javascript_function_value(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
}

pub(super) fn javascript_function_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    javascript_named_child(
        node,
        source,
        &["identifier", "property_identifier", "type_identifier"],
    )
}

pub(super) fn javascript_named_child(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

pub(super) fn javascript_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

pub(super) fn javascript_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string" {
            let target = decode_javascript_string_literal(child, source);
            if !target.is_empty() {
                targets.push(target);
            }
        }
    }
    targets
}

pub(super) fn decode_javascript_string_literal(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_fragment" | "template_chars") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .to_string()
}
