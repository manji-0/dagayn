use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::js_modules::{JavaScriptCaches, JavaScriptImportMap, JavaScriptTsconfigCache};
use super::util::normalize_relative_path;

/// The npm package a bare specifier names: `react`, `@scope/name` for
/// `@scope/name/sub`, `lodash` for `lodash/fp`, `node:fs` for
/// `node:fs/promises`. Relative and absolute paths, `#` subpath imports,
/// `~` aliases, URLs, and malformed scopes (`@/x`) are not packages.
pub(super) fn javascript_package_name(specifier: &str) -> Option<&str> {
    if specifier.is_empty()
        || specifier.starts_with(['.', '/', '#', '~'])
        || specifier.contains("://")
        || specifier.contains('\\')
    {
        return None;
    }
    let end = match specifier.strip_prefix('@') {
        Some(scoped) => {
            let (scope, rest) = scoped.split_once('/')?;
            let name_len = rest.find('/').unwrap_or(rest.len());
            if scope.is_empty() || name_len == 0 {
                return None;
            }
            1 + scope.len() + 1 + name_len
        }
        None => specifier.find('/').unwrap_or(specifier.len()),
    };
    Some(&specifier[..end])
}

/// The imported specifiers of `importer` that name external packages, with
/// their package names: a package-name specifier that resolves to no
/// repository file and is not a tsconfig alias (an in-repo module that
/// failed to resolve is not a package).
pub(super) fn collect_javascript_external_packages(
    import_map: &JavaScriptImportMap,
    importer: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> HashMap<String, String> {
    let mut packages = HashMap::new();
    for binding in import_map.values() {
        let module = binding.module.as_str();
        if packages.contains_key(module) {
            continue;
        }
        let Some(package) = javascript_package_name(module) else {
            continue;
        };
        if resolve_javascript_module(module, importer, repo_root, caches).is_none()
            && !javascript_is_repo_alias(module, importer, repo_root, caches)
        {
            packages.insert(module.to_string(), package.to_string());
        }
    }
    packages
}

/// Whether `module` names a repository path through the nearest tsconfig:
/// a `paths` pattern matches it (other than the catch-all `*`), or its first
/// segment exists under `baseUrl`.
fn javascript_is_repo_alias(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> bool {
    let Some((tsconfig_path, config)) =
        find_javascript_tsconfig(file_path, repo_root, caches.tsconfig)
    else {
        return false;
    };
    let Some(options) = config.get("compilerOptions") else {
        return false;
    };
    if options
        .get("paths")
        .and_then(Value::as_object)
        .is_some_and(|paths| {
            paths
                .keys()
                .any(|pattern| pattern != "*" && javascript_alias_match(pattern, module).is_some())
        })
    {
        return true;
    }
    let Some(base_url) = options.get("baseUrl").and_then(Value::as_str) else {
        return false;
    };
    let first = module.split('/').next().unwrap_or(module);
    let candidate = tsconfig_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(base_url)
        .join(first);
    javascript_module_candidate_is_dir(&candidate, repo_root)
        || probe_javascript_module_candidate(&candidate, repo_root).is_some()
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
    probe_javascript_module_candidate(&caller_dir.join(module), repo_root)
}

/// Extensions appended to an extensionless (or dotted, like
/// `./user.service`) specifier, in priority order. Implementation files come
/// before `.d.ts` so a declaration file never shadows its source.
const JAVASCRIPT_MODULE_EXTENSIONS: [&str; 10] = [
    ".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".vue",
];

/// Runtime extension -> source extensions tried when the written path does
/// not exist (`import "./x.js"` compiled from `x.ts`).
const JAVASCRIPT_RUNTIME_TO_SOURCE_EXTENSIONS: [(&str, &[&str]); 4] = [
    (".js", &[".ts", ".tsx", ".d.ts"]),
    (".jsx", &[".tsx"]),
    (".mjs", &[".mts", ".d.mts"]),
    (".cjs", &[".cts", ".d.cts"]),
];

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
    let base_url = compiler_options.get("baseUrl").and_then(Value::as_str);
    let base_dir = tsconfig_path.parent().unwrap_or_else(|| Path::new(""));
    let base_dir = base_dir.join(base_url.unwrap_or(""));

    let mut patterns = compiler_options
        .get("paths")
        .and_then(Value::as_object)
        .map(|paths| paths.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    patterns.sort_by_key(|(pattern, _)| std::cmp::Reverse(javascript_alias_specificity(pattern)));
    let mut matched = false;
    for (pattern, replacements) in patterns {
        let Some(suffix) = javascript_alias_match(pattern, module) else {
            continue;
        };
        matched = true;
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
    // As in TypeScript, `baseUrl` is the fallback for specifiers no `paths`
    // pattern matches: `"baseUrl": "src"` + `import "services/user"` gives
    // `src/services/user.ts`. Only an existing file counts, so a package
    // name without a same-named file under `baseUrl` stays external.
    if matched {
        return None;
    }
    base_url.and_then(|_| probe_javascript_module_candidate(&base_dir.join(module), repo_root))
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

/// Project config files, in priority order within one directory.
const JAVASCRIPT_PROJECT_CONFIGS: [&str; 3] =
    ["tsconfig.json", "tsconfig.app.json", "jsconfig.json"];

/// The nearest directory (from the importer up) holding a project config
/// wins. Within it, the first config that sets `paths` or `baseUrl` is used,
/// so a solution-style `tsconfig.json` (`files: []` plus `references`) does
/// not hide the aliases of its `tsconfig.app.json`; otherwise the first
/// readable one. `extends` is not followed.
fn find_javascript_tsconfig_uncached(mut current: PathBuf) -> Option<(PathBuf, Value)> {
    loop {
        let mut first = None;
        for name in JAVASCRIPT_PROJECT_CONFIGS {
            let candidate = current.join(name);
            if !candidate.is_file() {
                continue;
            }
            let Some(value) = read_javascript_tsconfig(&candidate) else {
                continue;
            };
            let options = value.get("compilerOptions");
            if options.is_some_and(|options| {
                options.get("paths").is_some() || options.get("baseUrl").is_some()
            }) {
                return Some((candidate, value));
            }
            first.get_or_insert((candidate, value));
        }
        if first.is_some() {
            return first;
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

/// Resolves one module path candidate (relative specifier joined to the
/// importer directory, or a tsconfig `paths` replacement) to a file.
///
/// Order: the path as written, runtime-to-source extension mapping, each
/// [`JAVASCRIPT_MODULE_EXTENSIONS`] appended to the full path (never
/// replacing a dotted segment such as `.service`), then `index.*` when the
/// path is a directory. A file therefore wins over a same-stem directory.
fn probe_javascript_module_candidate(candidate: &Path, repo_root: Option<&Path>) -> Option<String> {
    if javascript_module_candidate_is_file(candidate, repo_root) {
        return javascript_module_candidate_path(candidate.to_path_buf(), repo_root);
    }
    let raw = candidate.to_string_lossy();
    for (runtime, sources) in JAVASCRIPT_RUNTIME_TO_SOURCE_EXTENSIONS {
        let Some(stem) = raw.strip_suffix(runtime) else {
            continue;
        };
        for source_ext in sources {
            let target = PathBuf::from(format!("{stem}{source_ext}"));
            if javascript_module_candidate_is_file(&target, repo_root) {
                return javascript_module_candidate_path(target, repo_root);
            }
        }
    }
    for ext in JAVASCRIPT_MODULE_EXTENSIONS {
        let target = PathBuf::from(format!("{raw}{ext}"));
        if javascript_module_candidate_is_file(&target, repo_root) {
            return javascript_module_candidate_path(target, repo_root);
        }
    }
    if javascript_module_candidate_is_dir(candidate, repo_root) {
        for ext in JAVASCRIPT_MODULE_EXTENSIONS {
            let target = candidate.join(format!("index{ext}"));
            if javascript_module_candidate_is_file(&target, repo_root) {
                return javascript_module_candidate_path(target, repo_root);
            }
        }
    }
    None
}
