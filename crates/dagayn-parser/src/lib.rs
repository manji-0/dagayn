//! Rust parser crate.
//!
//! The migration target is for this crate to own file discovery, language
//! detection, parser orchestration, Markdown, Terraform, and notebook
//! extraction. During Phase 1 it starts with parseable-file filtering so Python
//! can shrink back toward CLI/MCP interfaces.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use globset::{Glob, GlobSetBuilder};

pub fn grammar_status() -> dagayn_grammars::GrammarStatus {
    dagayn_grammars::status()
}

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

pub fn collect_parseable_files(repo_root: &Path, recurse_submodules: Option<bool>) -> Vec<String> {
    let ignore_patterns = load_ignore_patterns(repo_root);
    let candidates = get_git_tracked_files(repo_root, recurse_submodules)
        .filter(|files| !files.is_empty())
        .unwrap_or_else(|| walk_files(repo_root));
    filter_parseable_files(repo_root, &candidates, &ignore_patterns)
}

pub fn detect_language(path: &Path) -> Option<&'static str> {
    let suffix = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_ascii_lowercase()));
    if let Some(suffix) = suffix.as_deref() {
        if let Some(language) = extension_to_language().get(suffix) {
            return Some(language);
        }
    }
    if path.extension().is_none() {
        return detect_language_from_shebang(path);
    }
    None
}

fn build_globset(patterns: &[String]) -> Option<globset::GlobSet> {
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

fn load_ignore_patterns(repo_root: &Path) -> Vec<String> {
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
    cmd.arg("ls-files");
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
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn walk_files(repo_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![repo_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(repo_root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

fn should_ignore(path: &str, patterns: &[String], globset: Option<&globset::GlobSet>) -> bool {
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
    match std::fs::read(path) {
        Ok(bytes) => bytes.iter().take(8192).any(|byte| *byte == 0),
        Err(_) => true,
    }
}

fn detect_language_from_shebang(path: &Path) -> Option<&'static str> {
    let bytes = std::fs::read(path).ok()?;
    let head = &bytes[..bytes.len().min(256)];
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
    shebang_to_language().get(interpreter).copied()
}

fn extension_to_language() -> HashMap<&'static str, &'static str> {
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
        (".res", "rescript"),
        (".resi", "rescript"),
        (".gd", "gdscript"),
        (".tf", "terraform"),
        (".tfvars", "terraform"),
        (".md", "markdown"),
        (".markdown", "markdown"),
    ])
}

fn shebang_to_language() -> HashMap<&'static str, &'static str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_extensions_and_shebangs() {
        assert_eq!(detect_language(Path::new("main.py")), Some("python"));
        assert_eq!(detect_language(Path::new("main.R")), Some("r"));
        assert_eq!(detect_language(Path::new("main.unknown")), None);
    }

    #[test]
    fn nested_dir_ignore_matches_python_behavior() {
        let patterns = vec!["node_modules/**".to_string()];
        assert!(should_ignore(
            "pkg/app/node_modules/react/index.js",
            &patterns,
            None
        ));
        assert!(should_ignore(
            "node_modules/react/index.js",
            &patterns,
            None
        ));
        assert!(!should_ignore("pkg/app/src/index.js", &patterns, None));
    }
}
