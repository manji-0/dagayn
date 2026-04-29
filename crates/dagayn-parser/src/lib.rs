//! Rust parser crate.
//!
//! The migration target is for this crate to own file discovery, language
//! detection, parser orchestration, Markdown, Terraform, and notebook
//! extraction. During Phase 1 it starts with parseable-file filtering so Python
//! can shrink back toward CLI/MCP interfaces.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

static TERRAFORM_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$"#).unwrap());
static TERRAFORM_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap());
static TERRAFORM_HEADER_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]*)"|'([^']*)'|([A-Za-z_][A-Za-z0-9_-]*)"#).unwrap());
static TERRAFORM_PROVIDER_SOURCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"source\s*=\s*(["'][^"']+["'])"#).unwrap());
static TERRAFORM_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:(data)\.([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)|((?:module|var|local|output|provider|check))\.([A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*))\b",
    )
    .unwrap()
});

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
    let headings = collect_markdown_headings(source, &text);
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
    parsed_compact_json(nodes, edges)
}

pub fn parse_rust_owned_files_compact_json(repo_root: &Path, file_paths: &[String]) -> String {
    let mut batch = Vec::new();
    let mut errors = Vec::new();
    for file_path in file_paths {
        if !rust_parser_owns_path(file_path) {
            errors.push(json!([file_path, "unsupported Rust parser path"]));
            continue;
        }
        let full_path = repo_root.join(file_path);
        let source = match std::fs::read(&full_path) {
            Ok(source) => source,
            Err(err) => {
                errors.push(json!([file_path, err.to_string()]));
                continue;
            }
        };
        let (nodes, edges) = parse_rust_owned_file(file_path, &source);
        let (nodes, edges) = parsed_compact_values(nodes, edges);
        batch.push(json!([file_path, nodes, edges, sha256_hex(&source)]));
    }
    json!({"batch": batch, "errors": errors}).to_string()
}

pub fn parse_terraform(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let line_end = source.iter().filter(|byte| **byte == b'\n').count() as i64 + 1;
    let blocks = collect_terraform_blocks(source, &text);
    let defined_names = blocks
        .iter()
        .flat_map(|block| {
            if block.kind == "locals" {
                collect_terraform_attrs(&block.body, block.body_start_line)
                    .into_iter()
                    .map(|attr| format!("local.{}", attr.name))
                    .collect::<Vec<_>>()
            } else {
                terraform_defined_name(block)
                    .into_iter()
                    .collect::<Vec<_>>()
            }
        })
        .collect::<HashSet<_>>();

    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "terraform".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    for block in &blocks {
        if block.kind == "locals" {
            for attr in collect_terraform_attrs(&block.body, block.body_start_line) {
                let node_name = format!("local.{}", attr.name);
                push_terraform_node(
                    file_path,
                    &mut nodes,
                    &mut edges,
                    TerraformNodeSpec {
                        kind: "Function",
                        name: &node_name,
                        line_start: attr.line_start,
                        line_end: attr.line_end,
                        is_test: false,
                        terraform_kind: "local",
                    },
                );
                scan_terraform_body(
                    &attr.text,
                    &node_name,
                    file_path,
                    attr.line_start,
                    &defined_names,
                    &mut edges,
                );
            }
            continue;
        }

        if matches!(block.kind.as_str(), "import" | "moved" | "removed") {
            handle_terraform_meta_block(file_path, block, &defined_names, &mut edges);
            continue;
        }

        let Some(node_name) = terraform_defined_name(block) else {
            continue;
        };
        let terraform_kind = terraform_kind_for_block(block);
        let (kind, is_test) = match block.kind.as_str() {
            "variable" | "output" => ("Function", false),
            "check" => ("Test", true),
            _ => ("Class", false),
        };
        push_terraform_node(
            file_path,
            &mut nodes,
            &mut edges,
            TerraformNodeSpec {
                kind,
                name: &node_name,
                line_start: block.line_start,
                line_end: block.line_end,
                is_test,
                terraform_kind,
            },
        );
        scan_terraform_body(
            &block.body,
            &node_name,
            file_path,
            block.line_start,
            &defined_names,
            &mut edges,
        );

        if block.kind == "module" {
            if let Some(source_attr) = collect_terraform_attrs(&block.body, block.body_start_line)
                .into_iter()
                .find(|attr| attr.name == "source")
            {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: terraform_qualified(file_path, &node_name),
                    target: strip_tf_string(&source_attr.value),
                    file_path: file_path.to_string(),
                    line: source_attr.line_start,
                    extra: json!({}),
                });
            }
        }

        if block.kind == "terraform" {
            for captures in TERRAFORM_PROVIDER_SOURCE_RE.captures_iter(&block.body) {
                edges.push(ParsedEdge {
                    kind: "DEPENDS_ON".to_string(),
                    source: terraform_qualified(file_path, &node_name),
                    target: strip_tf_string(&captures[1]),
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({}),
                });
            }
        }
    }

    (nodes, dedupe_edges(edges))
}

pub fn parse_terraform_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_terraform(file_path, source);
    parsed_compact_json(nodes, edges)
}

fn parsed_compact_json(nodes: Vec<ParsedNode>, edges: Vec<ParsedEdge>) -> String {
    let (compact_nodes, compact_edges) = parsed_compact_values(nodes, edges);
    json!([compact_nodes, compact_edges]).to_string()
}

fn parsed_compact_values(
    nodes: Vec<ParsedNode>,
    edges: Vec<ParsedEdge>,
) -> (Vec<Value>, Vec<Value>) {
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
    (compact_nodes, compact_edges)
}

fn parse_rust_owned_file(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let lowered = file_path.to_ascii_lowercase();
    if lowered.ends_with(".md") || lowered.ends_with(".markdown") {
        parse_markdown(file_path, source)
    } else if lowered.ends_with(".tf") || lowered.ends_with(".tfvars") {
        parse_terraform(file_path, source)
    } else {
        (Vec::new(), Vec::new())
    }
}

pub fn rust_parser_owns_path(file_path: &str) -> bool {
    let lowered = file_path.to_ascii_lowercase();
    lowered.ends_with(".md")
        || lowered.ends_with(".markdown")
        || lowered.ends_with(".tf")
        || lowered.ends_with(".tfvars")
}

fn sha256_hex(source: &[u8]) -> String {
    let digest = Sha256::digest(source);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Clone, Debug)]
struct TerraformBlock {
    kind: String,
    labels: Vec<String>,
    body: String,
    line_start: i64,
    line_end: i64,
    body_start_line: i64,
}

#[derive(Clone, Debug)]
struct TerraformAttr {
    name: String,
    value: String,
    text: String,
    line_start: i64,
    line_end: i64,
}

struct TerraformNodeSpec<'a> {
    kind: &'a str,
    name: &'a str,
    line_start: i64,
    line_end: i64,
    is_test: bool,
    terraform_kind: &'a str,
}

fn collect_terraform_blocks(source: &[u8], text: &str) -> Vec<TerraformBlock> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::terraform_language())
        .is_ok()
    {
        if let Some(tree) = parser.parse(source, None) {
            let mut blocks = Vec::new();
            collect_terraform_block_nodes(tree.root_node(), source, &mut blocks);
            if !blocks.is_empty() {
                return blocks;
            }
        }
    }
    collect_terraform_blocks_from_text(text)
}

fn collect_terraform_block_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    blocks: &mut Vec<TerraformBlock>,
) {
    if let Some(block) = terraform_block_from_node(node, source) {
        blocks.push(block);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_block_nodes(child, source, blocks);
    }
}

fn terraform_block_from_node(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<TerraformBlock> {
    let kind = terraform_kind_from_node_kind(node.kind())?.to_string();
    let labels = terraform_block_labels(node, source);
    let body_node = terraform_block_body_node(node);
    let body = body_node
        .map(|body| node_text(body, source))
        .unwrap_or_default();
    let body_start_line = body_node
        .map(|body| body.start_position().row as i64 + 1)
        .unwrap_or_else(|| node.start_position().row as i64 + 1);

    Some(TerraformBlock {
        kind,
        labels,
        body,
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        body_start_line,
    })
}

fn terraform_kind_from_node_kind(node_kind: &str) -> Option<&'static str> {
    match node_kind {
        "terraform_block" => Some("terraform"),
        "provider_block" => Some("provider"),
        "variable_block" => Some("variable"),
        "locals_block" => Some("locals"),
        "module_block" => Some("module"),
        "data_block" => Some("data"),
        "resource_block" => Some("resource"),
        "check_block" => Some("check"),
        "output_block" => Some("output"),
        "import_block" => Some("import"),
        "moved_block" => Some("moved"),
        "removed_block" => Some("removed"),
        "ephemeral_block" => Some("ephemeral"),
        _ => None,
    }
}

fn terraform_block_labels(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut labels = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_lit" {
            labels.push(strip_tf_string(&node_text(child, source)));
        }
    }
    labels
}

fn terraform_block_body_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if let Some(body) = node.child_by_field_name("body") {
        return Some(body);
    }
    let mut cursor = node.walk();
    let body = node
        .children(&mut cursor)
        .find(|child| child.kind() == "block_body");
    body
}

fn collect_terraform_blocks_from_text(text: &str) -> Vec<TerraformBlock> {
    let mut blocks = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let Some(open_rel) = text[offset..].find('{') else {
            break;
        };
        let open = offset + open_rel;
        let header_start = text[..open]
            .rfind(['\n', '}'])
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let header = strip_terraform_line_comment(&text[header_start..open]).trim();
        let Some((kind, labels)) = parse_terraform_header(header) else {
            offset = open + 1;
            continue;
        };
        let Some(close) = find_matching_brace(text, open) else {
            break;
        };
        let body = text[open + 1..close].to_string();
        blocks.push(TerraformBlock {
            kind,
            labels,
            body,
            line_start: line_for_offset(text, header_start),
            line_end: line_for_offset(text, close),
            body_start_line: line_for_offset(text, open),
        });
        offset = close + 1;
    }
    blocks
}

fn parse_terraform_header(header: &str) -> Option<(String, Vec<String>)> {
    if header.is_empty() || header.contains('=') {
        return None;
    }
    let tokens = TERRAFORM_HEADER_TOKEN_RE
        .captures_iter(header)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|value| value.as_str().to_string())
        })
        .collect::<Vec<_>>();
    let (kind, labels) = tokens.split_first()?;
    let supported = matches!(
        kind.as_str(),
        "terraform"
            | "provider"
            | "variable"
            | "locals"
            | "module"
            | "data"
            | "resource"
            | "check"
            | "output"
            | "import"
            | "moved"
            | "removed"
            | "ephemeral"
    );
    supported.then(|| (kind.clone(), labels.to_vec()))
}

fn find_matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_i64;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut in_line_comment = false;
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if idx < open {
            continue;
        }
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
            continue;
        }
        if ch == '#' {
            in_line_comment = true;
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '/') {
            in_line_comment = true;
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn collect_terraform_attrs(body: &str, body_start_line: i64) -> Vec<TerraformAttr> {
    let lines = body.lines().collect::<Vec<_>>();
    let mut attrs = Vec::new();
    let mut idx = 0_usize;
    while idx < lines.len() {
        let Some(captures) = TERRAFORM_ATTR_RE.captures(lines[idx]) else {
            idx += 1;
            continue;
        };
        let name = captures[1].to_string();
        let mut attr_lines = vec![lines[idx]];
        let mut depth = terraform_expr_depth(captures.get(2).map(|m| m.as_str()).unwrap_or(""));
        let start_idx = idx;
        idx += 1;
        while idx < lines.len() {
            let starts_next_attr = depth <= 0 && TERRAFORM_ATTR_RE.is_match(lines[idx]);
            if starts_next_attr {
                break;
            }
            if depth <= 0 && lines[idx].trim() == "}" {
                break;
            }
            attr_lines.push(lines[idx]);
            depth += terraform_expr_depth(lines[idx]);
            idx += 1;
            if depth <= 0
                && attr_lines
                    .last()
                    .is_some_and(|line| line.trim_end().ends_with('}'))
            {
                break;
            }
        }
        let text = attr_lines.join("\n");
        let value = TERRAFORM_ATTR_RE
            .captures(attr_lines[0])
            .and_then(|captures| captures.get(2))
            .map(|value| value.as_str().trim().to_string())
            .unwrap_or_default();
        attrs.push(TerraformAttr {
            name,
            value,
            text,
            line_start: body_start_line + start_idx as i64,
            line_end: body_start_line
                + start_idx as i64
                + attr_lines.len() as i64
                + i64::from(attr_lines.len() > 1)
                - 1,
        });
    }
    attrs
}

fn terraform_expr_depth(text: &str) -> i64 {
    let mut depth = 0_i64;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for ch in strip_terraform_line_comment(text).chars() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
            continue;
        }
        if matches!(ch, '{' | '[' | '(') {
            depth += 1;
        } else if matches!(ch, '}' | ']' | ')') {
            depth -= 1;
        }
    }
    depth
}

fn terraform_defined_name(block: &TerraformBlock) -> Option<String> {
    match block.kind.as_str() {
        "terraform" => Some("terraform".to_string()),
        "provider" => block.labels.first().map(|name| format!("provider.{name}")),
        "variable" => block.labels.first().map(|name| format!("var.{name}")),
        "module" => block.labels.first().map(|name| format!("module.{name}")),
        "data" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("data.{block_type}.{name}")),
        "resource" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("resource.{block_type}.{name}")),
        "ephemeral" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("ephemeral.{block_type}.{name}")),
        "output" => block.labels.first().map(|name| format!("output.{name}")),
        "check" => block.labels.first().map(|name| format!("check.{name}")),
        _ => None,
    }
}

fn terraform_kind_for_block(block: &TerraformBlock) -> &str {
    match block.kind.as_str() {
        "terraform" => "terraform",
        "provider" => "provider",
        "variable" => "variable",
        "module" => "module",
        "data" => "data",
        "resource" => "resource",
        "ephemeral" => "ephemeral",
        "output" => "output",
        "check" => "check",
        other => other,
    }
}

fn push_terraform_node(
    file_path: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
    spec: TerraformNodeSpec<'_>,
) {
    let qualified = terraform_qualified(file_path, spec.name);
    nodes.push(ParsedNode {
        kind: spec.kind.to_string(),
        name: spec.name.to_string(),
        file_path: file_path.to_string(),
        line_start: spec.line_start,
        line_end: spec.line_end,
        language: "terraform".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: spec.is_test,
        extra: json!({"terraform_kind": spec.terraform_kind}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified,
        file_path: file_path.to_string(),
        line: spec.line_start,
        extra: json!({}),
    });
}

fn handle_terraform_meta_block(
    file_path: &str,
    block: &TerraformBlock,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let attrs = collect_terraform_attrs(&block.body, block.body_start_line);
    let attr_value = |name: &str| {
        attrs
            .iter()
            .find(|attr| attr.name == name)
            .map(|attr| strip_tf_string(&attr.value))
    };
    match block.kind.as_str() {
        "import" => {
            if let Some(target) = attr_value("id").or_else(|| attr_value("to")) {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({}),
                });
            }
        }
        "moved" => {
            if let (Some(source), Some(target)) = (attr_value("from"), attr_value("to")) {
                edges.push(ParsedEdge {
                    kind: "REFERENCES".to_string(),
                    source,
                    target,
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({"terraform_kind": "moved"}),
                });
            }
        }
        "removed" => {
            if let Some(target) = attr_value("from") {
                edges.push(ParsedEdge {
                    kind: "REFERENCES".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({"terraform_kind": "removed"}),
                });
            }
        }
        _ => {}
    }
    scan_terraform_body(
        &block.body,
        file_path,
        file_path,
        block.line_start,
        defined_names,
        edges,
    );
}

fn scan_terraform_body(
    body: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    collect_terraform_calls(body, caller, file_path, line, edges);
    collect_terraform_references(body, caller, file_path, line, defined_names, edges);
}

fn collect_terraform_calls(
    text: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = HashSet::new();
    for captures in TERRAFORM_CALL_RE.captures_iter(text) {
        let name = captures[1].to_string();
        if matches!(name.as_str(), "for" | "if") || !seen.insert(name.clone()) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.to_string(),
            target: name,
            file_path: file_path.to_string(),
            line,
            extra: json!({}),
        });
    }
}

fn collect_terraform_references(
    text: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = HashSet::new();
    for captures in TERRAFORM_REFERENCE_RE.captures_iter(text) {
        let target = if captures.get(1).is_some() {
            format!("data.{}.{}", &captures[2], &captures[3])
        } else if captures.get(4).is_some() {
            format!("{}.{}", &captures[4], &captures[5])
        } else {
            let root = &captures[6];
            if matches!(
                root,
                "count" | "each" | "ingress" | "egress" | "path" | "self" | "terraform"
            ) {
                continue;
            }
            format!("resource.{}.{}", root, &captures[7])
        };
        if target == caller || !seen.insert(target.clone()) {
            continue;
        }
        let resolved = if defined_names.contains(&target) {
            terraform_qualified(file_path, &target)
        } else {
            target
        };
        edges.push(ParsedEdge {
            kind: "REFERENCES".to_string(),
            source: caller.to_string(),
            target: resolved,
            file_path: file_path.to_string(),
            line,
            extra: json!({}),
        });
    }
}

fn strip_tf_string(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn strip_terraform_line_comment(line: &str) -> &str {
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut prev = '\0';
    for (idx, ch) in line.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            prev = ch;
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
        } else if ch == '#' || (prev == '/' && ch == '/') {
            let start = if ch == '/' {
                idx.saturating_sub(1)
            } else {
                idx
            };
            return &line[..start];
        }
        prev = ch;
    }
    line
}

fn terraform_qualified(file_path: &str, name: &str) -> String {
    format!("{file_path}::{name}")
}

fn collect_markdown_headings(source: &[u8], text: &str) -> Vec<Heading> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::markdown_language())
        .is_ok()
    {
        if let Some(tree) = parser.parse(source, None) {
            let headings = collect_markdown_headings_from_tree(tree.root_node(), source);
            if !headings.is_empty() {
                return headings;
            }
        }
    }
    collect_markdown_headings_from_text(text)
}

fn collect_markdown_headings_from_tree(root: tree_sitter::Node<'_>, source: &[u8]) -> Vec<Heading> {
    let mut raw = Vec::new();
    collect_markdown_heading_nodes(root, source, &mut raw);
    assign_heading_slugs(raw)
}

fn collect_markdown_heading_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    raw: &mut Vec<(String, i64, i64)>,
) {
    if matches!(node.kind(), "atx_heading" | "setext_heading") {
        let text = markdown_heading_text(node, source);
        if !text.is_empty() {
            raw.push((
                text,
                markdown_heading_level(node, source),
                node.start_position().row as i64 + 1,
            ));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_markdown_heading_nodes(child, source, raw);
    }
}

fn markdown_heading_level(node: tree_sitter::Node<'_>, source: &[u8]) -> i64 {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind.starts_with("atx_h") && kind.ends_with("_marker") {
            return node_text(child, source).chars().count() as i64;
        }
        if kind == "setext_h1_underline" {
            return 1;
        }
        if kind == "setext_h2_underline" {
            return 2;
        }
    }
    1
}

fn markdown_heading_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "atx_h1_marker"
                | "atx_h2_marker"
                | "atx_h3_marker"
                | "atx_h4_marker"
                | "atx_h5_marker"
                | "atx_h6_marker"
                | "setext_h1_underline"
                | "setext_h2_underline"
        ) {
            continue;
        }
        let text = node_text(child, source).trim().to_string();
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts.join(" ").trim().to_string()
}

fn node_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]).to_string()
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

    #[test]
    fn parses_terraform_blocks_calls_and_refs() {
        let source = br#"terraform {
  required_providers {
    aws = {
      source = "hashicorp/aws"
    }
  }
}

variable "tags" {
  type = map(string)
}

locals {
  common_tags = merge(var.tags, {
    ManagedBy = "dagayn"
  })
}

module "network" {
  source = "./modules/network"
}

data "aws_caller_identity" "current" {}

resource "aws_vpc" "main" {
  cidr_block = module.network.cidr_block
  tags = merge(local.common_tags, {
    Account = data.aws_caller_identity.current.account_id
  })
}

check "vpc_ready" {
  assert {
    condition = length(module.network.public_subnet_ids) > 0
  }
}

output "vpc_id" {
  value = aws_vpc.main.id
}
"#;
        let (nodes, edges) = parse_terraform("main.tf", source);
        let names = nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"terraform"));
        assert!(names.contains(&"var.tags"));
        assert!(names.contains(&"local.common_tags"));
        assert!(names.contains(&"module.network"));
        assert!(names.contains(&"data.aws_caller_identity.current"));
        assert!(names.contains(&"resource.aws_vpc.main"));
        assert!(names.contains(&"check.vpc_ready"));
        assert!(names.contains(&"output.vpc_id"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "DEPENDS_ON"
                && edge.source == "main.tf::terraform"
                && edge.target == "hashicorp/aws"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "main.tf::module.network"
                && edge.target == "./modules/network"
        }));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "merge"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "resource.aws_vpc.main"
                && edge.target == "main.tf::data.aws_caller_identity.current"
        }));
    }

    #[test]
    fn parses_rust_owned_files_as_one_compact_batch() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-batch-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("docs")).unwrap();
        std::fs::write(
            repo_root.join("docs/README.md"),
            b"# Guide\n\nSee `build_graph`.\n",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("main.tf"),
            br#"variable "region" {
  default = "us-east-1"
}
"#,
        )
        .unwrap();

        let payload = parse_rust_owned_files_compact_json(
            &repo_root,
            &["docs/README.md".to_string(), "main.tf".to_string()],
        );
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["errors"].as_array().unwrap().len(), 0);
        let batch = parsed["batch"].as_array().unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().any(|item| item[0] == "docs/README.md"));
        assert!(batch.iter().any(|item| item[0] == "main.tf"));

        let _ = std::fs::remove_dir_all(&repo_root);
    }
}
