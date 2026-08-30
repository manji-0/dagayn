use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::member_calls::MemberCallBindings;
use super::parsers::{new_javascript_parser, new_tsx_parser, new_typescript_parser};
use super::qualify;
use super::util::{ends_with_ascii_ignore_case, node_text, normalize_relative_path};

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
    named_exports: HashMap<String, JavaScriptExportTarget>,
    star_exports: Vec<String>,
}

#[derive(Clone)]
pub(super) enum JavaScriptExportTarget {
    Local(String),
    External {
        module_file: String,
        symbol_name: String,
    },
}

pub(super) struct JavaScriptParseContext<'a> {
    pub(super) source: &'a [u8],
    pub(super) file_path: crate::core::types::FilePath,
    pub(super) language: &'static str,
    pub(super) test_file: bool,
    pub(super) defined_names: &'a HashSet<String>,
    pub(super) import_map: &'a HashMap<String, String>,
    pub(super) repo_root: Option<&'a Path>,
    pub(super) caches: JavaScriptCaches<'a>,
    pub(super) bindings: RefCell<MemberCallBindings>,
}

pub(super) fn collect_javascript_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "class_declaration" | "class" | "interface_declaration" => {
            if let Some(name) =
                javascript_named_child(node, source, &["identifier", "type_identifier"])
            {
                names.insert(name);
            }
        }
        "function_declaration" => {
            if let Some(name) = javascript_function_name(node, source) {
                names.insert(name);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for declarator in node.children(&mut cursor) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) = javascript_variable_declarator_function_name(declarator, source)
                {
                    names.insert(name);
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_defined_names(child, source, names);
    }
}

pub(super) fn collect_javascript_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "class_declaration"
        | "class"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => {
            if let Some(name) =
                javascript_named_child(node, source, &["identifier", "type_identifier"])
            {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_type_names(child, source, names);
    }
}

pub(super) fn resolve_javascript_call_target(
    name: &str,
    context: &JavaScriptParseContext<'_>,
) -> String {
    if context.defined_names.contains(name) {
        return qualify(&context.file_path, name, None);
    }
    let Some(module) = context.import_map.get(name) else {
        return name.to_string();
    };
    resolve_javascript_imported_symbol(name, module, context).unwrap_or_else(|| name.to_string())
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
    resolve_javascript_exported_symbol(
        &module_file,
        symbol_name,
        context.repo_root,
        context.caches,
        &mut HashSet::new(),
    )
    .or_else(|| Some(qualify(&module_file, symbol_name, None)))
}

fn resolve_javascript_exported_symbol(
    module_file: &str,
    symbol_name: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
    seen: &mut HashSet<(String, String)>,
) -> Option<String> {
    let key = (module_file.to_string(), symbol_name.to_string());
    if !seen.insert(key) {
        return None;
    }
    let index = javascript_export_index(module_file, repo_root, caches)?;
    if index.defined_names.contains(symbol_name) {
        return Some(qualify(module_file, symbol_name, None));
    }
    if let Some(target) = index.named_exports.get(symbol_name) {
        return match target {
            JavaScriptExportTarget::Local(original_name) => {
                Some(qualify(module_file, original_name, None))
            }
            JavaScriptExportTarget::External {
                module_file,
                symbol_name,
            } => resolve_javascript_exported_symbol(
                module_file,
                symbol_name,
                repo_root,
                caches,
                seen,
            )
            .or_else(|| Some(qualify(module_file, symbol_name, None))),
        };
    }
    for exported_module in &index.star_exports {
        if let Some(result) = resolve_javascript_exported_symbol(
            exported_module,
            symbol_name,
            repo_root,
            caches,
            seen,
        ) {
            return Some(result);
        }
    }
    None
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

    let mut defined_names = HashSet::new();
    collect_javascript_defined_names(root, &source, &mut defined_names);
    let mut named_exports = HashMap::new();
    let mut star_exports = Vec::new();

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "export_statement" {
            continue;
        }
        let (export_clause, target_module, has_star_export) =
            javascript_export_statement_parts(child, &source);
        if let Some(export_clause) = export_clause {
            let mut clause_cursor = export_clause.walk();
            for spec in export_clause.children(&mut clause_cursor) {
                if spec.kind() != "export_specifier" {
                    continue;
                }
                let names = javascript_named_descendants(
                    spec,
                    &source,
                    &["identifier", "property_identifier"],
                );
                let Some(exported_name) = names.last() else {
                    continue;
                };
                let original_name = names.first().unwrap_or(exported_name);
                if let Some(target_module) = target_module.as_deref() {
                    if let Some(resolved_module) =
                        resolve_javascript_module(target_module, module_file, repo_root, caches)
                    {
                        named_exports.insert(
                            exported_name.clone(),
                            JavaScriptExportTarget::External {
                                module_file: resolved_module,
                                symbol_name: original_name.clone(),
                            },
                        );
                    }
                } else {
                    named_exports.insert(
                        exported_name.clone(),
                        JavaScriptExportTarget::Local(original_name.clone()),
                    );
                }
            }
        }

        if has_star_export {
            let Some(target_module) = target_module.as_deref() else {
                continue;
            };
            let Some(resolved_module) =
                resolve_javascript_module(target_module, module_file, repo_root, caches)
            else {
                continue;
            };
            star_exports.push(resolved_module);
        }
    }
    Some(JavaScriptExportIndex {
        defined_names,
        named_exports,
        star_exports,
    })
}

fn new_javascript_module_parser(module_file: &str) -> Option<tree_sitter::Parser> {
    if ends_with_ascii_ignore_case(module_file, ".tsx") {
        return new_tsx_parser();
    }
    if ends_with_ascii_ignore_case(module_file, ".ts") {
        return new_typescript_parser();
    }
    if ends_with_ascii_ignore_case(module_file, ".js")
        || ends_with_ascii_ignore_case(module_file, ".jsx")
        || ends_with_ascii_ignore_case(module_file, ".mjs")
    {
        return new_javascript_parser();
    }
    None
}

fn javascript_export_statement_parts<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> (Option<tree_sitter::Node<'a>>, Option<String>, bool) {
    let mut export_clause = None;
    let mut target_module = None;
    let mut has_star_export = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "export_clause" => export_clause = Some(child),
            "string" => target_module = Some(decode_javascript_string_literal(child, source)),
            "*" => has_star_export = true,
            _ => {}
        }
    }
    (export_clause, target_module, has_star_export)
}

pub(super) fn collect_javascript_import_map(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &mut HashMap<String, String>,
) {
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

fn collect_javascript_import_clause_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    module: &str,
    import_map: &mut HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                import_map.insert(node_text(child, source), module.to_string());
            }
            "namespace_import" => {
                if let Some(name) = javascript_last_named_descendant(
                    child,
                    source,
                    &["identifier", "property_identifier"],
                ) {
                    import_map.insert(name, module.to_string());
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
    import_map: &mut HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for spec in node.children(&mut cursor) {
        if spec.kind() != "import_specifier" {
            continue;
        }
        if let Some(name) =
            javascript_last_named_descendant(spec, source, &["identifier", "property_identifier"])
        {
            import_map.insert(name, module.to_string());
        }
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

fn javascript_named_descendants(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut names = Vec::new();
    collect_javascript_named_descendants(node, source, kinds, &mut names);
    names
}

fn collect_javascript_named_descendants(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    names: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            names.push(node_text(child, source));
        }
        collect_javascript_named_descendants(child, source, kinds, names);
    }
}

fn javascript_variable_declarator_function_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    let mut name = None;
    let mut has_function = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" && name.is_none() {
            name = Some(node_text(child, source));
        } else if is_javascript_function_value(child.kind()) {
            has_function = true;
        }
    }
    has_function.then_some(name).flatten()
}

fn is_javascript_function_value(kind: &str) -> bool {
    matches!(kind, "arrow_function" | "function_expression" | "function")
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

pub(super) fn resolve_javascript_module(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    let key = (file_path.to_string(), module.to_string());
    if let Some(cache) = caches.module
        && let Some(cached) = cache.borrow().get(&key).cloned()
    {
        return cached;
    }
    let result = resolve_javascript_module_uncached(module, file_path, repo_root, caches);
    if let Some(cache) = caches.module {
        cache.borrow_mut().insert(key, result.clone());
    }
    result
}

fn resolve_javascript_module_uncached(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    if !module.starts_with('.') {
        return resolve_javascript_alias(module, file_path, repo_root, caches);
    }
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let base = caller_dir.join(module);
    if javascript_module_candidate_is_file(&base, repo_root) {
        return javascript_module_candidate_path(base, repo_root);
    }
    for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
        let target = base.with_extension(ext.trim_start_matches('.'));
        if javascript_module_candidate_is_file(&target, repo_root) {
            return javascript_module_candidate_path(target, repo_root);
        }
    }
    if javascript_module_candidate_is_dir(&base, repo_root) {
        for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
            let target = base.join(format!("index{ext}"));
            if javascript_module_candidate_is_file(&target, repo_root) {
                return javascript_module_candidate_path(target, repo_root);
            }
        }
    }
    None
}

fn javascript_module_candidate_is_file(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_file())
        .unwrap_or_else(|| candidate.is_file())
}

fn javascript_module_candidate_is_dir(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_dir())
        .unwrap_or_else(|| candidate.is_dir())
}

fn javascript_module_candidate_path(
    candidate: PathBuf,
    repo_root: Option<&Path>,
) -> Option<String> {
    if let Some(repo_root) = repo_root {
        let relative = candidate
            .strip_prefix(repo_root)
            .ok()
            .unwrap_or(candidate.as_path());
        return Some(normalize_relative_path(relative));
    }
    candidate
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

fn resolve_javascript_alias(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    let (tsconfig_path, config) = find_javascript_tsconfig(file_path, repo_root, caches.tsconfig)?;
    let compiler_options = config.get("compilerOptions")?;
    let paths = compiler_options.get("paths")?.as_object()?;
    let base_url = compiler_options
        .get("baseUrl")
        .and_then(Value::as_str)
        .unwrap_or("");
    let base_dir = tsconfig_path.parent().unwrap_or_else(|| Path::new(""));
    let base_dir = base_dir.join(base_url);

    let mut patterns = paths.iter().collect::<Vec<_>>();
    patterns.sort_by_key(|(pattern, _)| std::cmp::Reverse(javascript_alias_specificity(pattern)));
    for (pattern, replacements) in patterns {
        let Some(suffix) = javascript_alias_match(pattern, module) else {
            continue;
        };
        let Some(replacements) = replacements.as_array() else {
            continue;
        };
        for replacement in replacements {
            let Some(replacement) = replacement.as_str() else {
                continue;
            };
            let mapped = if replacement.contains('*') {
                replacement.replacen('*', &suffix, 1)
            } else {
                replacement.to_string()
            };
            let candidate = base_dir.join(mapped);
            if let Some(path) = probe_javascript_module_candidate(&candidate, repo_root) {
                return Some(path);
            }
        }
    }
    None
}

fn find_javascript_tsconfig(
    file_path: &str,
    repo_root: Option<&Path>,
    tsconfig_cache: Option<&JavaScriptTsconfigCache>,
) -> Option<(PathBuf, Value)> {
    let mut current = if let Some(repo_root) = repo_root {
        repo_root.join(file_path)
    } else {
        PathBuf::from(file_path)
    };
    current = current.parent()?.to_path_buf();
    if let Some(cache) = tsconfig_cache
        && let Some(cached) = cache.borrow().get(&current).cloned()
    {
        return cached;
    }
    let start_dir = current.clone();
    let result = find_javascript_tsconfig_uncached(current);
    if let Some(cache) = tsconfig_cache {
        cache.borrow_mut().insert(start_dir, result.clone());
    }
    result
}

fn find_javascript_tsconfig_uncached(mut current: PathBuf) -> Option<(PathBuf, Value)> {
    loop {
        for name in ["tsconfig.json", "tsconfig.app.json"] {
            let candidate = current.join(name);
            if candidate.is_file() {
                let value = read_javascript_tsconfig(&candidate)?;
                return Some((candidate, value));
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    None
}

fn read_javascript_tsconfig(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let stripped = strip_jsonc_comments(&raw);
    serde_json::from_str(&stripped).ok()
}

fn strip_jsonc_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            out.push('"');
            i += 1;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                out.push(ch);
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i] as char);
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    strip_json_trailing_commas(&out)
}

fn strip_json_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b',' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && matches!(bytes[j], b'}' | b']') {
                i += 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn javascript_alias_specificity(pattern: &str) -> usize {
    pattern
        .split_once('*')
        .map(|(prefix, _)| prefix.len())
        .unwrap_or(pattern.len())
}

fn javascript_alias_match(pattern: &str, module: &str) -> Option<String> {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return (pattern == module).then(String::new);
    };
    if !module.starts_with(prefix) || !module.ends_with(suffix) {
        return None;
    }
    let end = module.len().saturating_sub(suffix.len());
    Some(module[prefix.len()..end].to_string())
}

fn probe_javascript_module_candidate(candidate: &Path, repo_root: Option<&Path>) -> Option<String> {
    if javascript_module_candidate_is_file(candidate, repo_root) {
        return javascript_module_candidate_path(candidate.to_path_buf(), repo_root);
    }
    for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
        let target = if candidate.extension().is_none() {
            candidate.with_extension(ext.trim_start_matches('.'))
        } else {
            PathBuf::from(format!("{}{}", candidate.to_string_lossy(), ext))
        };
        if javascript_module_candidate_is_file(&target, repo_root) {
            return javascript_module_candidate_path(target, repo_root);
        }
    }
    if javascript_module_candidate_is_dir(candidate, repo_root) {
        for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
            let target = candidate.join(format!("index{ext}"));
            if javascript_module_candidate_is_file(&target, repo_root) {
                return javascript_module_candidate_path(target, repo_root);
            }
        }
    }
    None
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
