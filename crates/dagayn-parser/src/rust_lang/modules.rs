//! Rust module resolution: which file a `use` path or a `module::item` path
//! names.
//!
//! A crate's module tree is read from its root (`src/lib.rs`, `src/main.rs`,
//! or a lone target such as `examples/x.rs`) by following `mod name;`
//! declarations, `#[path = "..."]` included, the way rustc does. Only the
//! declarations are needed, so other files are scanned as text (comments and
//! string literals blanked) rather than parsed; the tree is cached per parser
//! for the whole batch.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;

/// Module path (segments below the crate root) -> where it lives.
#[derive(Debug, Default)]
pub(crate) struct CrateModules {
    by_path: HashMap<Vec<String>, String>,
    by_file: HashMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
pub(crate) struct RustModuleCache {
    /// Crate root file (repo-relative) -> its module tree.
    crates: RefCell<HashMap<String, Rc<CrateModules>>>,
    /// Library crate name (`dagayn_graph`) -> root file, for the repository.
    libraries: RefCell<Option<Rc<HashMap<String, String>>>>,
    /// Package directory -> the crates its `Cargo.toml` depends on.
    dependencies: RefCell<HashMap<String, Rc<HashSet<String>>>>,
}

/// Where the file being parsed sits.
pub(crate) struct RustModuleScope<'a> {
    pub(crate) repo_root: &'a Path,
    pub(crate) cache: &'a RustModuleCache,
    crate_modules: Rc<CrateModules>,
    /// Module path of the file within its crate.
    module: Vec<String>,
    /// Library crate of the same package, reachable by name from a binary,
    /// example, test, or bench target.
    own_library: Option<(String, String)>,
    /// Crates the package depends on (`[dependencies]`, dev and build),
    /// by the name code uses (`serde_json`).
    dependencies: Rc<HashSet<String>>,
}

/// A resolved `use` / path prefix: the file holding the module, and the
/// segments after it (items, or `*`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResolvedPath {
    pub(crate) file: String,
    pub(crate) rest: Vec<String>,
}

static MOD_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)((?:#\s*\[[^\]]*\]\s*)*)(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(?:r#)?([A-Za-z_][A-Za-z0-9_]*)\s*([;{])",
    )
    .expect("mod declaration regex")
});
static PATH_ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]"#).expect("path attribute regex")
});
static PACKAGE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ms)^\[package\][^\[]*?^\s*name\s*=\s*"([^"]+)""#).expect("package name regex")
});
static LIB_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ms)^\[lib\][^\[]*?^\s*name\s*=\s*"([^"]+)""#).expect("lib name regex")
});
static MEMBERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ms)^\s*members\s*=\s*\[(.*?)\]"#).expect("workspace members regex")
});

/// Blanks comments and string / char literals, keeping offsets, so a `mod`
/// inside a doc comment or a test fixture string is not a declaration.
fn blank_non_code(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to.min(bytes.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let end = bytes[i..]
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(bytes.len(), |p| i + p);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |p| i + 2 + p + 2);
                blank(&mut out, i, end);
                i = end;
            }
            b'r' if matches!(bytes.get(i + 1), Some(b'"') | Some(b'#'))
                && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) =>
            {
                let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
                if bytes.get(i + 1 + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let closing = format!("\"{}", "#".repeat(hashes));
                let start = i + 2 + hashes;
                let end = text[start..]
                    .find(&closing)
                    .map_or(bytes.len(), |p| start + p + closing.len());
                blank(&mut out, i, end);
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut out, i, j + 1);
                i = j + 1;
            }
            b'\'' => {
                // A char literal ('a', '\n'), not a lifetime ('a).
                let end = if bytes.get(i + 1) == Some(&b'\\') {
                    bytes[i + 2..]
                        .iter()
                        .position(|b| *b == b'\'')
                        .map(|p| i + 2 + p)
                } else {
                    let len = text[i + 1..].chars().next().map_or(1, char::len_utf8);
                    (bytes.get(i + 1 + len) == Some(&b'\'')).then_some(i + 1 + len)
                };
                match end {
                    Some(end) => {
                        blank(&mut out, i, end + 1);
                        i = end + 1;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Whether a module file owns its directory (`lib.rs`, `main.rs`, `mod.rs`,
/// or a lone crate root): its child `mod x;` is `dir/x.rs`, not
/// `dir/<stem>/x.rs`.
fn is_mod_rs(file: &str, root: &str) -> bool {
    file == root || file.ends_with("/mod.rs") || file == "mod.rs"
}

fn parent_dir(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, tail: &str) -> String {
    if dir.is_empty() {
        tail.to_string()
    } else {
        format!("{dir}/{tail}")
    }
}

/// Lexically normalizes `a/b/../c` to `a/c`.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn exists(repo_root: &Path, rel: &str) -> bool {
    repo_root.join(rel).is_file()
}

fn build_crate_modules(repo_root: &Path, root: &str) -> CrateModules {
    let mut modules = CrateModules::default();
    let mut pending: Vec<(Vec<String>, String)> = vec![(Vec::new(), root.to_string())];
    while let Some((path, file)) = pending.pop() {
        if modules.by_file.contains_key(&file) {
            continue;
        }
        modules.by_path.insert(path.clone(), file.clone());
        modules.by_file.insert(file.clone(), path.clone());
        let Ok(text) = std::fs::read_to_string(repo_root.join(&file)) else {
            continue;
        };
        let code = blank_non_code(&text);
        let base_dir = if is_mod_rs(&file, root) {
            parent_dir(&file).to_string()
        } else {
            join(
                parent_dir(&file),
                file.rsplit('/')
                    .next()
                    .unwrap_or(&file)
                    .trim_end_matches(".rs"),
            )
        };
        for captures in MOD_DECL.captures_iter(&code) {
            let whole = captures.get(0).expect("match");
            // Only top-level declarations of the file: an inline module's own
            // `mod x;` children are rare and resolve below it.
            let depth = code[..whole.start()].matches('{').count() as i64
                - code[..whole.start()].matches('}').count() as i64;
            if depth != 0 {
                continue;
            }
            let name = captures[2].to_string();
            let mut child_path = path.clone();
            child_path.push(name.clone());
            if &captures[3] == "{" {
                // `mod name { ... }`: the module lives in this file.
                modules
                    .by_path
                    .entry(child_path)
                    .or_insert_with(|| file.clone());
                continue;
            }
            let attrs = &text[captures.get(1).expect("attrs").range()];
            let child = match PATH_ATTR.captures(attrs) {
                Some(path_attr) => Some(normalize(&join(parent_dir(&file), &path_attr[1]))),
                None => [
                    join(&base_dir, &format!("{name}.rs")),
                    join(&base_dir, &format!("{name}/mod.rs")),
                ]
                .into_iter()
                .find(|candidate| exists(repo_root, candidate)),
            };
            if let Some(child) = child.filter(|child| exists(repo_root, child)) {
                pending.push((child_path, child));
            }
        }
    }
    modules
}

/// Library crates of the repository: the root package and workspace members.
fn build_libraries(repo_root: &Path) -> HashMap<String, String> {
    let mut libraries = HashMap::new();
    let mut packages = vec![String::new()];
    if let Ok(manifest) = std::fs::read_to_string(repo_root.join("Cargo.toml"))
        && let Some(members) = MEMBERS.captures(&manifest)
    {
        for member in members[1].split(',') {
            let member = member.trim().trim_matches('"').trim_end_matches('/');
            if member.is_empty() {
                continue;
            }
            if let Some(prefix) = member.strip_suffix("/*") {
                if let Ok(entries) = std::fs::read_dir(repo_root.join(prefix)) {
                    for entry in entries.flatten() {
                        if entry.path().join("Cargo.toml").is_file() {
                            packages.push(join(prefix, &entry.file_name().to_string_lossy()));
                        }
                    }
                }
            } else {
                packages.push(member.to_string());
            }
        }
    }
    for package in packages {
        let Ok(manifest) = std::fs::read_to_string(repo_root.join(join(&package, "Cargo.toml")))
        else {
            continue;
        };
        let name = LIB_NAME
            .captures(&manifest)
            .or_else(|| PACKAGE_NAME.captures(&manifest))
            .map(|captures| captures[1].replace('-', "_"));
        let lib = join(&package, "src/lib.rs");
        if let Some(name) = name
            && exists(repo_root, &lib)
        {
            libraries.insert(name, lib);
        }
    }
    libraries
}

impl RustModuleCache {
    fn crate_modules(&self, repo_root: &Path, root: &str) -> Rc<CrateModules> {
        if let Some(modules) = self.crates.borrow().get(root) {
            return Rc::clone(modules);
        }
        let modules = Rc::new(build_crate_modules(repo_root, root));
        self.crates
            .borrow_mut()
            .insert(root.to_string(), Rc::clone(&modules));
        modules
    }

    fn dependencies(&self, repo_root: &Path, package: &str) -> Rc<HashSet<String>> {
        if let Some(dependencies) = self.dependencies.borrow().get(package) {
            return Rc::clone(dependencies);
        }
        let dependencies = Rc::new(
            std::fs::read_to_string(repo_root.join(join(package, "Cargo.toml")))
                .map(|manifest| manifest_dependencies(&manifest))
                .unwrap_or_default(),
        );
        self.dependencies
            .borrow_mut()
            .insert(package.to_string(), Rc::clone(&dependencies));
        dependencies
    }

    fn libraries(&self, repo_root: &Path) -> Rc<HashMap<String, String>> {
        if let Some(libraries) = self.libraries.borrow().as_ref() {
            return Rc::clone(libraries);
        }
        let libraries = Rc::new(build_libraries(repo_root));
        *self.libraries.borrow_mut() = Some(Rc::clone(&libraries));
        libraries
    }
}

/// The crates a `Cargo.toml` depends on, by the name code uses: the keys of
/// its `[dependencies]`, `[dev-dependencies]`, and `[build-dependencies]`
/// tables (target-specific ones too, and dotted keys such as
/// `tree-sitter.workspace = true`) and of `[dependencies.name]` tables, with
/// `-` read as `_`.
fn manifest_dependencies(manifest: &str) -> HashSet<String> {
    let is_dependency_table = |table: &str| {
        let last = table.rsplit('.').next().unwrap_or(table);
        matches!(
            last,
            "dependencies" | "dev-dependencies" | "build-dependencies"
        )
    };
    let mut dependencies = HashSet::new();
    let mut in_table = false;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(table) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let table = table.trim();
            in_table = is_dependency_table(table);
            // `[dependencies.serde]`
            if !in_table
                && let Some((parent, name)) = table.rsplit_once('.')
                && is_dependency_table(parent)
            {
                dependencies.insert(name.trim_matches('"').replace('-', "_"));
            }
            continue;
        }
        if !in_table || line.starts_with('#') {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            // `tree-sitter.workspace = true` names `tree-sitter`.
            let name = key
                .split('.')
                .next()
                .unwrap_or(key)
                .trim()
                .trim_matches('"');
            if !name.is_empty() {
                dependencies.insert(name.replace('-', "_"));
            }
        }
    }
    dependencies
}

/// The directory of the package (the nearest `Cargo.toml`) holding `file`.
fn package_dir(repo_root: &Path, file: &str) -> Option<String> {
    let mut dir = parent_dir(file).to_string();
    loop {
        if exists(repo_root, &join(&dir, "Cargo.toml")) {
            return Some(dir);
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent_dir(&dir).to_string();
    }
}

/// The crate roots that may contain `file`, most likely first.
fn crate_roots(repo_root: &Path, file: &str) -> (Vec<String>, Option<String>) {
    let mut dir = parent_dir(file).to_string();
    loop {
        if exists(repo_root, &join(&dir, "Cargo.toml")) {
            break;
        }
        if dir.is_empty() {
            return (vec![file.to_string()], None);
        }
        dir = parent_dir(&dir).to_string();
    }
    let package = dir;
    let rel = file.strip_prefix(&join(&package, "")).unwrap_or(file);
    let rel = if package.is_empty() {
        file
    } else {
        rel.trim_start_matches('/')
    };
    let lib = join(&package, "src/lib.rs");
    let own_library = exists(repo_root, &lib).then(|| lib.clone());
    let lone_target = ["examples/", "tests/", "benches/", "src/bin/"]
        .iter()
        .any(|prefix| rel.starts_with(prefix));
    if lone_target || !rel.starts_with("src/") {
        // `src/bin/x/main.rs` and `examples/x/main.rs` root their directory.
        return (vec![file.to_string()], own_library);
    }
    let mut roots = Vec::new();
    if let Some(lib) = own_library.clone() {
        roots.push(lib);
    }
    let main = join(&package, "src/main.rs");
    if exists(repo_root, &main) {
        roots.push(main);
    }
    (roots, None)
}

impl<'a> RustModuleScope<'a> {
    pub(crate) fn new(repo_root: &'a Path, cache: &'a RustModuleCache, file: &str) -> Self {
        let (roots, own_library_root) = crate_roots(repo_root, file);
        let mut chosen = None;
        for root in &roots {
            let modules = cache.crate_modules(repo_root, root);
            if let Some(module) = modules.by_file.get(file).cloned() {
                chosen = Some((modules, module));
                break;
            }
        }
        let (crate_modules, module) = chosen.unwrap_or_else(|| {
            // Not reachable through `mod` declarations: a crate of its own.
            (cache.crate_modules(repo_root, file), Vec::new())
        });
        let own_library = own_library_root.and_then(|root| {
            cache
                .libraries(repo_root)
                .iter()
                .find(|(_, lib)| **lib == root)
                .map(|(name, lib)| (name.clone(), lib.clone()))
        });
        let dependencies = package_dir(repo_root, file)
            .map(|package| cache.dependencies(repo_root, &package))
            .unwrap_or_default();
        Self {
            repo_root,
            cache,
            crate_modules,
            module,
            own_library,
            dependencies,
        }
    }

    /// Whether `name` is a crate the package depends on from outside the
    /// repository: not a library crate of the workspace (a path dependency
    /// such as `dagayn_graph`), nor a module of this crate (a child module
    /// shadows a crate of the same name).
    pub(crate) fn is_dependency(&self, name: &str) -> bool {
        if !self.dependencies.contains(name)
            || self.cache.libraries(self.repo_root).contains_key(name)
        {
            return false;
        }
        let mut child = self.module.clone();
        child.push(name.to_string());
        !self.crate_modules.by_path.contains_key(&child)
    }

    /// Resolves `segments` (a `use` path or the module part of a call path)
    /// to the file of its deepest module and the segments after it.
    pub(crate) fn resolve(&self, segments: &[String]) -> Option<ResolvedPath> {
        let first = segments.first()?.as_str();
        let (modules, mut base, mut rest) = match first {
            "crate" => (Rc::clone(&self.crate_modules), Vec::new(), &segments[1..]),
            "self" => (
                Rc::clone(&self.crate_modules),
                self.module.clone(),
                &segments[1..],
            ),
            "super" => {
                let mut base = self.module.clone();
                let mut rest = segments;
                while rest.first().map(String::as_str) == Some("super") {
                    base.pop()?;
                    rest = &rest[1..];
                }
                (Rc::clone(&self.crate_modules), base, rest)
            }
            _ => {
                // A child module of the current one (2018 uniform paths),
                // else a library crate of this repository.
                let mut child = self.module.clone();
                child.push(first.to_string());
                if self.crate_modules.by_path.contains_key(&child) {
                    (Rc::clone(&self.crate_modules), child, &segments[1..])
                } else {
                    let root = match &self.own_library {
                        Some((name, root)) if name == first => root.clone(),
                        _ => self.cache.libraries(self.repo_root).get(first)?.clone(),
                    };
                    (
                        self.cache.crate_modules(self.repo_root, &root),
                        Vec::new(),
                        &segments[1..],
                    )
                }
            }
        };
        while let Some(next) = rest.first() {
            let mut deeper = base.clone();
            deeper.push(next.clone());
            if !modules.by_path.contains_key(&deeper) {
                break;
            }
            base = deeper;
            rest = &rest[1..];
        }
        Some(ResolvedPath {
            file: modules.by_path.get(&base)?.clone(),
            rest: rest.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let mut root = std::env::temp_dir();
        root.push(format!("dagayn-rust-modules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    fn segments(path: &str) -> Vec<String> {
        path.split("::").map(str::to_string).collect()
    }

    #[test]
    fn resolves_module_files_path_attributes_and_workspace_crates() {
        let root = repo(
            "tree",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
                ("crates/core/Cargo.toml", "[package]\nname = \"my-core\"\n"),
                (
                    "crates/core/src/lib.rs",
                    "mod util;\n#[path = \"lang/py/mod.rs\"]\nmod python;\n// mod ghost;\nconst S: &str = \"mod fake;\";\nmod inline { }\n",
                ),
                ("crates/core/src/util.rs", "pub mod text;\n"),
                ("crates/core/src/util/text.rs", "pub fn node_text() {}\n"),
                ("crates/core/src/lang/py/mod.rs", "mod notebook;\n"),
                ("crates/core/src/lang/py/notebook.rs", ""),
                ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
                ("crates/app/src/main.rs", "fn main() {}\n"),
            ],
        );
        let cache = RustModuleCache::default();
        let scope = RustModuleScope::new(&root, &cache, "crates/core/src/util/text.rs");
        assert_eq!(scope.module, segments("util::text"));
        let resolve = |path: &str| scope.resolve(&segments(path));
        assert_eq!(
            resolve("crate::util::text::node_text"),
            Some(ResolvedPath {
                file: "crates/core/src/util/text.rs".into(),
                rest: segments("node_text")
            })
        );
        assert_eq!(
            resolve("super::text::node_text").unwrap().file,
            "crates/core/src/util/text.rs"
        );
        assert_eq!(
            resolve("crate::python::notebook::f").unwrap().file,
            "crates/core/src/lang/py/notebook.rs"
        );
        assert_eq!(
            resolve("crate::inline::f").unwrap().file,
            "crates/core/src/lib.rs"
        );
        assert_eq!(
            resolve("crate::ghost::f").unwrap().rest,
            segments("ghost::f")
        );
        assert_eq!(resolve("crate::fake").unwrap().rest, segments("fake"));
        assert_eq!(resolve("std::collections::HashMap"), None);

        let app = RustModuleScope::new(&root, &cache, "crates/app/src/main.rs");
        assert_eq!(
            app.resolve(&segments("my_core::util::text::node_text"))
                .unwrap()
                .file,
            "crates/core/src/util/text.rs"
        );
    }
}
