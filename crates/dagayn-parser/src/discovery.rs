use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use globset::{Glob, GlobSetBuilder};

static EXTENSION_TO_LANGUAGE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        (".py", "python"),
        (".js", "javascript"),
        (".jsx", "javascript"),
        (".ts", "typescript"),
        (".tsx", "tsx"),
        (".go", "go"),
        (".rs", "rust"),
        (".java", "java"),
        (".cs", "csharp"),
        (".rb", "ruby"),
        (".cpp", "cpp"),
        (".cc", "cpp"),
        (".cxx", "cpp"),
        (".c", "c"),
        (".h", "c"),
        (".hpp", "cpp"),
        (".kt", "kotlin"),
        (".kts", "kotlin"),
        (".swift", "swift"),
        (".php", "php"),
        (".scala", "scala"),
        (".sol", "solidity"),
        (".vue", "vue"),
        (".dart", "dart"),
        (".r", "r"),
        (".mjs", "javascript"),
        (".astro", "typescript"),
        (".pl", "perl"),
        (".pm", "perl"),
        (".t", "perl"),
        (".xs", "c"),
        (".lua", "lua"),
        (".luau", "luau"),
        (".m", "objc"),
        (".sh", "bash"),
        (".bash", "bash"),
        (".zsh", "bash"),
        (".ksh", "bash"),
        (".ex", "elixir"),
        (".exs", "elixir"),
        (".ipynb", "notebook"),
        (".zig", "zig"),
        (".ps1", "powershell"),
        (".psm1", "powershell"),
        (".psd1", "powershell"),
        (".svelte", "svelte"),
        (".jl", "julia"),
        (".gd", "gdscript"),
        (".tf", "terraform"),
        (".tfvars", "terraform"),
        (".md", "markdown"),
        (".markdown", "markdown"),
    ])
});
static SHEBANG_TO_LANGUAGE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("bash", "bash"),
        ("sh", "bash"),
        ("zsh", "bash"),
        ("ksh", "bash"),
        ("dash", "bash"),
        ("ash", "bash"),
        ("python", "python"),
        ("python2", "python"),
        ("python3", "python"),
        ("pypy", "python"),
        ("pypy3", "python"),
        ("node", "javascript"),
        ("nodejs", "javascript"),
        ("ruby", "ruby"),
        ("perl", "perl"),
        ("lua", "lua"),
        ("Rscript", "r"),
        ("php", "php"),
    ])
});
pub fn filter_parseable_files(
    repo_root: &Path,
    candidates: &[String],
    ignore_patterns: &[String],
) -> Vec<String> {
    let globset = build_globset(ignore_patterns);
    candidates
        .iter()
        .filter_map(|candidate| {
            let rel_path = candidate.as_str();
            if should_ignore(rel_path, ignore_patterns, globset.as_ref()) {
                return None;
            }
            let full_path = repo_root.join(rel_path);
            if !full_path.is_file() || full_path.is_symlink() {
                return None;
            }
            detect_language(&full_path)?;
            if is_binary(&full_path) {
                return None;
            }
            Some(candidate.clone())
        })
        .collect()
}

pub fn filter_incremental_candidates(
    repo_root: &Path,
    candidates: &[String],
    ignore_patterns: &[String],
) -> (Vec<String>, Vec<String>) {
    let globset = build_globset(ignore_patterns);
    let mut parseable_files = Vec::new();
    let mut removed_files = Vec::new();
    for candidate in candidates {
        let rel_path = candidate.as_str();
        if should_ignore(rel_path, ignore_patterns, globset.as_ref()) {
            continue;
        }
        let full_path = repo_root.join(rel_path);
        if !full_path.is_file() {
            removed_files.push(candidate.clone());
            continue;
        }
        if full_path.is_symlink() {
            continue;
        }
        if detect_language(&full_path).is_none() || is_binary(&full_path) {
            continue;
        }
        parseable_files.push(candidate.clone());
    }
    (parseable_files, removed_files)
}

pub fn collect_parseable_files(repo_root: &Path, recurse_submodules: Option<bool>) -> Vec<String> {
    let ignore_patterns = load_ignore_patterns(repo_root);
    let globset = build_globset(&ignore_patterns);
    let candidates = get_git_tracked_files(repo_root, recurse_submodules)
        .filter(|files| !files.is_empty())
        .unwrap_or_else(|| walk_files(repo_root, &ignore_patterns, globset.as_ref()));
    filter_parseable_files(repo_root, &candidates, &ignore_patterns)
}

pub fn detect_language(path: &Path) -> Option<&'static str> {
    let suffix = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_ascii_lowercase()));
    if let Some(suffix) = suffix.as_deref() {
        if let Some(language) = EXTENSION_TO_LANGUAGE.get(suffix) {
            return Some(language);
        }
    }
    if path.extension().is_none() {
        return detect_language_from_shebang(path);
    }
    None
}

pub(crate) fn build_globset(patterns: &[String]) -> Option<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut added = false;
    for pattern in patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
            added = true;
        }
    }
    added.then(|| builder.build().ok()).flatten()
}

pub(crate) fn load_ignore_patterns(repo_root: &Path) -> Vec<String> {
    let mut patterns = default_ignore_patterns()
        .iter()
        .map(|pattern| pattern.to_string())
        .collect::<Vec<_>>();
    let ignore_file = repo_root.join(".dagaynignore");
    if let Ok(raw) = std::fs::read_to_string(ignore_file) {
        patterns.extend(
            raw.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string),
        );
    }
    patterns
}

fn get_git_tracked_files(
    repo_root: &Path,
    recurse_submodules: Option<bool>,
) -> Option<Vec<String>> {
    if !repo_root.join(".git").exists() {
        return None;
    }
    let mut cmd = Command::new("git");
    // `-z`: `core.quotePath` is on by default, so without it every non-ASCII
    // path arrives C-quoted (`"caf\303\251.py"`), fails the `is_file()` check
    // below, and the file is silently missing from the graph.
    cmd.arg("ls-files").arg("-z");
    if recurse_submodules.unwrap_or(false) {
        cmd.arg("--recurse-submodules");
    }
    let output = cmd.current_dir(repo_root).output().ok()?;
    if !output.status.success() {
        return Some(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(
        stdout
            .split('\0')
            .filter(|field| !field.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

pub(crate) fn walk_files(
    repo_root: &Path,
    ignore_patterns: &[String],
    globset: Option<&globset::GlobSet>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![repo_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel_path = path
                .strip_prefix(repo_root)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"));
            if path.is_dir() {
                if rel_path
                    .as_deref()
                    .is_some_and(|rel| should_ignore(rel, ignore_patterns, globset))
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if let Some(rel) = rel_path {
                out.push(rel);
            }
        }
    }
    out
}

pub(crate) fn should_ignore(
    path: &str,
    patterns: &[String],
    globset: Option<&globset::GlobSet>,
) -> bool {
    if globset.is_some_and(|set| set.is_match(path)) {
        return true;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    for pattern in patterns {
        let Some(prefix) = pattern.strip_suffix("/**") else {
            continue;
        };
        if prefix.is_empty() || prefix.contains('/') {
            continue;
        }
        if parts.contains(&prefix) {
            return true;
        }
    }
    false
}

fn is_binary(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return true,
    };
    let mut head = [0_u8; 8192];
    match file.read(&mut head) {
        Ok(size) => head[..size].contains(&0),
        Err(_) => true,
    }
}

fn detect_language_from_shebang(path: &Path) -> Option<&'static str> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0_u8; 256];
    let size = file.read(&mut head).ok()?;
    detect_language_from_shebang_bytes(&head[..size])
}

pub(crate) fn detect_language_from_shebang_bytes(head: &[u8]) -> Option<&'static str> {
    if !head.starts_with(b"#!") {
        return None;
    }
    let first_line = head.split(|byte| *byte == b'\n').next()?;
    let first_line = first_line.split(|byte| *byte == 0).next()?;
    let line = std::str::from_utf8(&first_line[2..]).ok()?.trim();
    if line.is_empty() {
        return None;
    }
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    let first = tokens.first()?;
    let interpreter = if first.ends_with("/env") || *first == "env" {
        tokens
            .iter()
            .skip(1)
            .find(|token| !token.starts_with('-'))?
            .rsplit('/')
            .next()?
    } else {
        first.rsplit('/').next()?
    };
    SHEBANG_TO_LANGUAGE.get(interpreter).copied()
}

fn default_ignore_patterns() -> &'static [&'static str] {
    &[
        ".dagayn/**",
        "node_modules/**",
        ".git/**",
        ".svn/**",
        "__pycache__/**",
        "*.pyc",
        ".venv/**",
        "venv/**",
        "dist/**",
        "build/**",
        ".next/**",
        "target/**",
        "vendor/**",
        "bootstrap/cache/**",
        "public/build/**",
        ".bundle/**",
        ".gradle/**",
        "*.jar",
        ".dart_tool/**",
        ".pub-cache/**",
        "coverage/**",
        ".cache/**",
        "*.min.js",
        "*.min.css",
        "*.map",
        "*.lock",
        "package-lock.json",
        "yarn.lock",
        "*.db",
        "*.sqlite",
        "*.db-journal",
        "*.db-wal",
    ]
}
