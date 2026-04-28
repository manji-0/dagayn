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
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

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

#[derive(Debug, Serialize)]
pub struct ParsedNode {
    pub kind: String,
    pub name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub parent_name: Option<String>,
    pub params: Option<String>,
    pub return_type: Option<String>,
    pub modifiers: Option<String>,
    pub is_test: bool,
    pub extra: Value,
}

#[derive(Debug, Serialize)]
pub struct ParsedEdge {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub file_path: String,
    pub line: i64,
    pub extra: Value,
}

#[derive(Clone, Debug)]
struct Heading {
    text: String,
    slug: String,
    level: i64,
    line: i64,
}

pub fn parse_markdown(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let line_end = source.iter().filter(|byte| **byte == b'\n').count() as i64 + 1;
    let headings = collect_markdown_headings_from_text(&text);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "markdown".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    let mut stack: Vec<(i64, String)> = Vec::new();
    for heading in &headings {
        while stack
            .last()
            .is_some_and(|(level, _)| *level >= heading.level)
        {
            stack.pop();
        }
        let section_qname = format!("{}::{}", file_path, heading.slug);
        let container = stack
            .last()
            .map(|(_, qname)| qname.clone())
            .unwrap_or_else(|| file_path.to_string());
        nodes.push(ParsedNode {
            kind: "Class".to_string(),
            name: heading.slug.clone(),
            file_path: file_path.to_string(),
            line_start: heading.line,
            line_end: heading.line,
            language: "markdown".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({
                "markdown_kind": "section",
                "display_name": heading.text,
                "heading_level": heading.level,
            }),
        });
        edges.push(ParsedEdge {
            kind: "CONTAINS".to_string(),
            source: container,
            target: section_qname.clone(),
            file_path: file_path.to_string(),
            line: heading.line,
            extra: json!({}),
        });
        stack.push((heading.level, section_qname));
    }

    extract_markdown_directives(file_path, &text, &headings, &mut edges);
    extract_markdown_links(file_path, &text, &headings, &mut edges);
    extract_markdown_code_spans(file_path, &text, &headings, &mut edges);
    (nodes, dedupe_edges(edges))
}

pub fn parse_markdown_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_markdown(file_path, source);
    let compact_nodes = nodes
        .into_iter()
        .map(|node| {
            json!([
                node.kind,
                node.name,
                node.file_path,
                node.line_start,
                node.line_end,
                node.language,
                node.parent_name,
                node.params,
                node.return_type,
                node.modifiers,
                node.is_test,
                node.extra,
            ])
        })
        .collect::<Vec<_>>();
    let compact_edges = edges
        .into_iter()
        .map(|edge| {
            json!([
                edge.kind,
                edge.source,
                edge.target,
                edge.file_path,
                edge.line,
                edge.extra,
            ])
        })
        .collect::<Vec<_>>();
    json!([compact_nodes, compact_edges]).to_string()
}

fn collect_markdown_headings_from_text(text: &str) -> Vec<Heading> {
    let mut raw = Vec::new();
    let lines = text.lines().collect::<Vec<_>>();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        let stripped = line.trim();
        if stripped.starts_with('#') {
            let marker = stripped.chars().take_while(|char| *char == '#').count();
            if (1..=6).contains(&marker)
                && stripped.len() > marker
                && stripped.as_bytes().get(marker) == Some(&b' ')
            {
                let title = stripped[marker + 1..].trim().trim_end_matches('#').trim();
                if !title.is_empty() {
                    raw.push((title.to_string(), marker as i64, idx as i64 + 1));
                }
            }
        } else if idx + 1 < lines.len() {
            let underline = lines[idx + 1].trim();
            if !stripped.is_empty()
                && !underline.is_empty()
                && underline.chars().all(|char| char == '=')
            {
                raw.push((stripped.to_string(), 1, idx as i64 + 1));
                idx += 1;
            } else if !stripped.is_empty()
                && !underline.is_empty()
                && underline.chars().all(|char| char == '-')
            {
                raw.push((stripped.to_string(), 2, idx as i64 + 1));
                idx += 1;
            }
        }
        idx += 1;
    }
    assign_heading_slugs(raw)
}

fn assign_heading_slugs(raw: Vec<(String, i64, i64)>) -> Vec<Heading> {
    let mut counts = HashMap::<String, usize>::new();
    let mut assigned = std::collections::HashSet::<String>::new();
    raw.into_iter()
        .map(|(text, level, line)| {
            let base = markdown_slugify(&text);
            let n = counts.get(&base).copied().unwrap_or(0);
            let slug = if n == 0 && !assigned.contains(&base) {
                base.clone()
            } else {
                let mut k = n.max(1);
                loop {
                    let candidate = format!("{base}-{k}");
                    if !assigned.contains(&candidate) {
                        break candidate;
                    }
                    k += 1;
                }
            };
            counts.insert(base, n + 1);
            assigned.insert(slug.clone());
            Heading {
                text,
                slug,
                level,
                line,
            }
        })
        .collect()
}

fn markdown_slugify(text: &str) -> String {
    let mut out = String::new();
    for char in text.chars() {
        if char.is_alphanumeric() {
            out.extend(char.to_lowercase());
        } else if char == ' ' || char == '-' {
            out.push('-');
        } else if char == '_' {
            out.push('_');
        }
    }
    out
}

fn markdown_section_for_line(line: i64, file_path: &str, headings: &[Heading]) -> Option<String> {
    let mut section_slug = None;
    for heading in headings {
        if heading.line > line {
            break;
        }
        section_slug = Some(heading.slug.as_str());
    }
    section_slug.map(|slug| format!("{file_path}::{slug}"))
}

fn extract_markdown_directives(
    file_path: &str,
    text: &str,
    headings: &[Heading],
    edges: &mut Vec<ParsedEdge>,
) {
    let re =
        Regex::new(r"(?i)<!--\s*(constrained-by|blocked-by|supersedes|derived-from)\s+(.+?)\s*-->")
            .expect("valid markdown directive regex");
    for captures in re.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let kind = captures[1].to_ascii_lowercase();
        let raw_target = captures[2].trim();
        let line = line_for_offset(text, matched.start());
        let source = markdown_section_for_line(line, file_path, headings)
            .unwrap_or_else(|| file_path.to_string());
        let Some(target) = markdown_target(raw_target, file_path) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "DEPENDS_ON".to_string(),
            source,
            target: target.clone(),
            file_path: file_path.to_string(),
            line,
            extra: json!({"markdown_directive_kind": kind}),
        });
        let target_file = target
            .split_once("::")
            .map(|(target_file, _)| target_file)
            .unwrap_or(target.as_str());
        if target_file != file_path {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: target_file.to_string(),
                file_path: file_path.to_string(),
                line,
                extra: json!({
                    "markdown_import_kind": "directive",
                    "markdown_directive_kind": kind,
                }),
            });
        }
    }
}

fn extract_markdown_links(
    file_path: &str,
    text: &str,
    headings: &[Heading],
    edges: &mut Vec<ParsedEdge>,
) {
    let inline_re = Regex::new(r"\[[^\]]+\]\(([^)]+)\)").expect("valid markdown link regex");
    let ref_re = Regex::new(r"(?m)^\s*\[[^\]]+\]:\s*(\S+)").expect("valid markdown ref regex");
    for captures in inline_re
        .captures_iter(text)
        .chain(ref_re.captures_iter(text))
    {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let raw_target = normalize_link_target(&captures[1]);
        if raw_target.is_empty() || is_external_target(&raw_target) {
            continue;
        }
        let line = line_for_offset(text, matched.start());
        let source = markdown_section_for_line(line, file_path, headings)
            .unwrap_or_else(|| file_path.to_string());
        let Some(target) = markdown_target(&raw_target, file_path) else {
            continue;
        };
        if let Some((target_file, _target_section)) = target.split_once("::") {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: target_file.to_string(),
                file_path: file_path.to_string(),
                line,
                extra: json!({"markdown_import_kind": "link"}),
            });
            edges.push(ParsedEdge {
                kind: "REFERENCES".to_string(),
                source,
                target,
                file_path: file_path.to_string(),
                line,
                extra: json!({"markdown_reference_kind": "link"}),
            });
        } else if target != file_path {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target,
                file_path: file_path.to_string(),
                line,
                extra: json!({"markdown_import_kind": "link"}),
            });
        }
    }
}

fn extract_markdown_code_spans(
    file_path: &str,
    text: &str,
    headings: &[Heading],
    edges: &mut Vec<ParsedEdge>,
) {
    let code_re = Regex::new(r"`([^`\n]+)`").expect("valid markdown code span regex");
    let symbol_re = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$")
        .expect("valid markdown symbol regex");
    let mut seen = std::collections::HashSet::new();
    for captures in code_re.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let sym = captures[1].trim();
        if sym.len() < 3 || !symbol_re.is_match(sym) {
            continue;
        }
        if !sym.contains('_') && !sym.contains('.') && sym.len() < 10 {
            continue;
        }
        let line = line_for_offset(text, matched.start());
        let source = markdown_section_for_line(line, file_path, headings)
            .unwrap_or_else(|| file_path.to_string());
        if !seen.insert((source.clone(), sym.to_string(), line)) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "CROSS_ARTIFACT".to_string(),
            source,
            target: format!("<unresolved:{sym}>"),
            file_path: file_path.to_string(),
            line,
            extra: json!({
                "relationship_role": "describes_symbol",
                "bridge_kind": "documentation",
                "evidence_kind": "markdown_code_span",
                "evidence_source": "code_span",
                "source_language": "markdown",
                "target_language": "unknown",
                "confidence": 0.2,
                "confidence_tier": "LOW",
                "unresolved_target_name": sym,
            }),
        });
    }
}

fn normalize_link_target(target: &str) -> String {
    let mut target = target.trim().to_string();
    if target.is_empty() {
        return String::new();
    }
    let title_re = Regex::new(r#"\s+(?:"[^"]*"|'[^']*')\s*$"#).expect("valid title regex");
    if let Some(matched) = title_re.find(&target) {
        target = target[..matched.start()].trim_end().to_string();
    }
    if target.starts_with('<') && target.ends_with('>') {
        target = target[1..target.len() - 1].trim().to_string();
    }
    target
}

fn is_external_target(target: &str) -> bool {
    let lowered = target.to_ascii_lowercase();
    lowered.starts_with("http://")
        || lowered.starts_with("https://")
        || lowered.starts_with("mailto:")
        || lowered.starts_with("tel:")
}

fn markdown_target(raw_target: &str, source_file: &str) -> Option<String> {
    let raw_target = raw_target.trim();
    if raw_target.is_empty() || raw_target.starts_with('/') {
        return None;
    }
    if let Some(section) = raw_target.strip_prefix('#') {
        let slug = markdown_slugify(section.trim());
        return (!slug.is_empty()).then(|| format!("{source_file}::{slug}"));
    }

    let (path_part, section_part) = raw_target
        .split_once('#')
        .map(|(path, section)| (path, Some(section.trim())))
        .unwrap_or((raw_target, None));
    let source = Path::new(source_file);
    let target_path = normalize_relative_path(
        &source
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path_part),
    );
    if let Some(section_part) = section_part {
        let slug = markdown_slugify(section_part);
        if !slug.is_empty() {
            return Some(format!("{target_path}::{slug}"));
        }
    }
    Some(target_path)
}

fn line_for_offset(text: &str, offset: usize) -> i64 {
    text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as i64
        + 1
}

fn normalize_relative_path(path: &Path) -> String {
    let mut parts = Vec::<String>::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().to_string());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                parts.push(component.as_os_str().to_string_lossy().to_string());
            }
        }
    }
    parts.join("/")
}

fn dedupe_edges(edges: Vec<ParsedEdge>) -> Vec<ParsedEdge> {
    let mut seen = std::collections::HashSet::new();
    edges
        .into_iter()
        .filter(|edge| {
            seen.insert((
                edge.kind.clone(),
                edge.source.clone(),
                edge.target.clone(),
                edge.line,
            ))
        })
        .collect()
}

fn is_test_file(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.starts_with("test_")
        || lower.ends_with("_test.md")
        || lower.ends_with(".test.md")
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

    #[test]
    fn parses_markdown_sections_and_edges() {
        let source = b"# API Reference

<!-- derived-from ./guide.md#Installation -->

See [Getting Started](./guide.md#Getting-Started).

## Endpoints

Call `build_graph`.
";
        let (nodes, edges) = parse_markdown("api.md", source);
        assert_eq!(nodes.len(), 3);
        assert!(nodes.iter().any(|node| node.name == "api-reference"));
        assert!(nodes.iter().any(|node| node.name == "endpoints"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "DEPENDS_ON"
                && edge.source == "api.md::api-reference"
                && edge.target == "guide.md::installation"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "api.md::api-reference"
                && edge.target == "guide.md::getting-started"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT" && edge.target == "<unresolved:build_graph>"
        }));
    }
}
