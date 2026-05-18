use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use super::documentation_directives::{parse_dagayn_directive, push_documentation_directive_edge};
use super::types::{ParsedEdge, ParsedNode};
use super::util::{dedupe_edges, is_test_file, line_count, node_text, normalize_relative_path};

static MARKDOWN_INLINE_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]+\]\(([^)]+)\)").unwrap());
static MARKDOWN_CODE_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());
static MARKDOWN_SYMBOL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$").unwrap());
static MARKDOWN_TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\s+(?:"[^"]*"|'[^']*')\s*$"#).unwrap());
static MARKDOWN_REFERENCE_DEF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[[^\]]+\]:").unwrap());
const LINE_CURSOR_SCAN_THRESHOLD: usize = 4;

#[derive(Clone, Debug)]
struct Heading {
    text: String,
    slug: String,
    level: i64,
    line: i64,
}

struct MarkdownLineContext<'a> {
    file_path: &'a str,
    headings: &'a [Heading],
}

#[derive(Default)]
struct MarkdownTreeFacts {
    parsed: bool,
    headings: Vec<Heading>,
    directives: Vec<MarkdownDirective>,
    reference_links: Vec<MarkdownLink>,
}

struct MarkdownDirective {
    kind: String,
    target: String,
    line: i64,
}

struct MarkdownLink {
    target: String,
    line: i64,
}

impl<'a> MarkdownLineContext<'a> {
    fn new(file_path: &'a str, headings: &'a [Heading]) -> Self {
        Self {
            file_path,
            headings,
        }
    }

    fn section_for_line(&self, line: i64) -> Option<String> {
        if self.headings.len() <= 8 {
            let mut section_slug = None;
            for heading in self.headings {
                if heading.line > line {
                    break;
                }
                section_slug = Some(heading.slug.as_str());
            }
            return section_slug.map(|slug| format!("{}::{slug}", self.file_path));
        }
        let idx = self
            .headings
            .partition_point(|heading| heading.line <= line);
        idx.checked_sub(1)
            .map(|idx| format!("{}::{}", self.file_path, self.headings[idx].slug))
    }

    fn source_for_line(&self, line: i64) -> String {
        self.section_for_line(line)
            .unwrap_or_else(|| self.file_path.to_string())
    }
}

struct LineCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
    line: i64,
    lookups: usize,
}

impl<'a> LineCursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            offset: 0,
            line: 1,
            lookups: 0,
        }
    }

    fn line_for_offset(&mut self, offset: usize) -> i64 {
        if self.lookups < LINE_CURSOR_SCAN_THRESHOLD {
            self.lookups += 1;
            let end = offset.min(self.bytes.len());
            self.offset = end;
            self.line = self.bytes[..end]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as i64
                + 1;
            return self.line;
        }
        if offset < self.offset {
            self.offset = 0;
            self.line = 1;
        }
        let end = offset.min(self.bytes.len());
        self.line += self.bytes[self.offset..end]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as i64;
        self.offset = end;
        self.line
    }
}

pub(super) fn parse_markdown_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let line_end = line_count(source);
    let tree_facts = collect_markdown_tree_facts(source, &text, parser);
    let headings = tree_facts.headings;
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
            kind: "DocSection".to_string(),
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

    extract_markdown_doc_bodies(file_path, &text, &headings, &mut nodes, &mut edges);

    let line_context = MarkdownLineContext::new(file_path, &headings);
    if tree_facts.parsed {
        extract_markdown_directives(&line_context, &tree_facts.directives, &mut edges);
        extract_markdown_reference_links(&line_context, &tree_facts.reference_links, &mut edges);
    } else {
        extract_markdown_directives_from_text(&line_context, &text, &mut edges);
        extract_markdown_reference_links_from_text(&line_context, &text, &mut edges);
    }
    extract_markdown_inline_links(&line_context, &text, &mut edges);
    extract_markdown_dagayn_directives(&line_context, &text, &mut edges);
    extract_markdown_code_spans(&line_context, &text, &mut edges);
    (nodes, dedupe_edges(edges))
}

fn extract_markdown_doc_bodies(
    file_path: &str,
    text: &str,
    headings: &[Heading],
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let context = MarkdownLineContext::new(file_path, headings);
    let heading_lines: HashMap<i64, ()> =
        headings.iter().map(|heading| (heading.line, ())).collect();
    let mut counters: HashMap<String, i64> = HashMap::new();
    let mut block_start: Option<i64> = None;
    let mut block_lines: Vec<String> = Vec::new();

    for (idx, line) in text.lines().enumerate() {
        let line_no = idx as i64 + 1;
        let trimmed = line.trim();
        let skip = trimmed.is_empty()
            || heading_lines.contains_key(&line_no)
            || is_markdown_non_body_line(trimmed);
        if skip {
            flush_markdown_doc_body(
                file_path,
                &context,
                &mut counters,
                nodes,
                edges,
                &mut block_start,
                &mut block_lines,
                line_no - 1,
            );
            continue;
        }
        if block_start.is_none() {
            block_start = Some(line_no);
            block_lines.clear();
        }
        block_lines.push(line.to_string());
    }

    let final_line = text.lines().count() as i64;
    flush_markdown_doc_body(
        file_path,
        &context,
        &mut counters,
        nodes,
        edges,
        &mut block_start,
        &mut block_lines,
        final_line,
    );
}

fn is_markdown_non_body_line(trimmed: &str) -> bool {
    (trimmed.starts_with("<!--") && trimmed.ends_with("-->"))
        || MARKDOWN_REFERENCE_DEF_RE.is_match(trimmed)
}

fn flush_markdown_doc_body(
    file_path: &str,
    context: &MarkdownLineContext<'_>,
    counters: &mut HashMap<String, i64>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
    block_start: &mut Option<i64>,
    block_lines: &mut Vec<String>,
    block_end: i64,
) {
    let Some(start_line) = block_start.take() else {
        return;
    };
    if block_lines.is_empty() {
        return;
    }
    let Some(parent_section) = context.section_for_line(start_line) else {
        block_lines.clear();
        return;
    };
    let parent_slug = parent_section.rsplit("::").next().unwrap_or("document");
    let counter = counters.entry(parent_section.clone()).or_insert(0);
    *counter += 1;
    let name = format!("{parent_slug}--body-{counter}");
    let qualified_name = format!("{file_path}::{name}");
    let display_name = block_lines
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(96)
        .collect::<String>();

    nodes.push(ParsedNode {
        kind: "DocBody".to_string(),
        name: name.clone(),
        file_path: file_path.to_string(),
        line_start: start_line,
        line_end: block_end.max(start_line),
        language: "markdown".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({
            "markdown_kind": "body",
            "display_name": display_name,
            "parent_section": parent_section,
        }),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.source_for_line(start_line),
        target: qualified_name,
        file_path: file_path.to_string(),
        line: start_line,
        extra: json!({}),
    });
    block_lines.clear();
}

fn collect_markdown_tree_facts(
    source: &[u8],
    text: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> MarkdownTreeFacts {
    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let mut facts = MarkdownTreeFacts {
                parsed: true,
                headings: collect_markdown_headings_from_tree(root, source),
                ..MarkdownTreeFacts::default()
            };
            collect_markdown_link_and_directive_nodes(root, source, &mut facts);
            if facts.headings.is_empty() {
                facts.headings = collect_markdown_headings_from_text(text);
            }
            return facts;
        }
    }
    MarkdownTreeFacts {
        parsed: false,
        headings: collect_markdown_headings_from_text(text),
        ..MarkdownTreeFacts::default()
    }
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

fn collect_markdown_link_and_directive_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    facts: &mut MarkdownTreeFacts,
) {
    match node.kind() {
        "directive_comment" => {
            if let Some(directive) = parse_markdown_directive_node(node, source) {
                facts.directives.push(directive);
            }
        }
        "link_reference_definition" => {
            if let Some(link) = parse_markdown_reference_link_node(node, source) {
                facts.reference_links.push(link);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_markdown_link_and_directive_nodes(child, source, facts);
    }
}

fn parse_markdown_directive_node(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<MarkdownDirective> {
    let text = node_text(node, source);
    let inner = text
        .trim()
        .strip_prefix("<!--")?
        .trim()
        .strip_suffix("-->")?
        .trim();
    let (kind, target) = inner
        .split_once(char::is_whitespace)
        .map(|(kind, target)| (kind, target.trim()))
        .unwrap_or((inner, ""));
    if !matches_markdown_directive_kind(kind) || target.is_empty() {
        return None;
    }
    Some(MarkdownDirective {
        kind: kind.to_ascii_lowercase(),
        target: target.to_string(),
        line: node.start_position().row as i64 + 1,
    })
}

fn parse_markdown_reference_link_node(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<MarkdownLink> {
    let mut cursor = node.walk();
    let destination = node
        .children(&mut cursor)
        .find(|child| child.kind() == "link_destination")?;
    let target = normalize_link_target(&node_text(destination, source));
    (!target.is_empty()).then(|| MarkdownLink {
        target,
        line: node.start_position().row as i64 + 1,
    })
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

fn extract_markdown_directives(
    line_context: &MarkdownLineContext<'_>,
    directives: &[MarkdownDirective],
    edges: &mut Vec<ParsedEdge>,
) {
    for directive in directives {
        let raw_target = directive.target.trim();
        let line = directive.line;
        let source = line_context.source_for_line(line);
        let file_path = line_context.file_path;
        let Some(target) = markdown_target(raw_target, file_path) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "DEPENDS_ON".to_string(),
            source,
            target: target.clone(),
            file_path: file_path.to_string(),
            line,
            extra: json!({"markdown_directive_kind": directive.kind}),
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
                    "markdown_directive_kind": directive.kind,
                }),
            });
        }
    }
}

fn extract_markdown_directives_from_text(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut lines = LineCursor::new(text);
    let directives = text.match_indices("<!--").filter_map(|(offset, _)| {
        let rest = &text[offset..];
        let end = rest.find("-->")?;
        let full = &rest[..end + 3];
        let node_line = lines.line_for_offset(offset);
        let inner = full
            .trim()
            .strip_prefix("<!--")?
            .trim()
            .strip_suffix("-->")?
            .trim();
        let (kind, target) = inner
            .split_once(char::is_whitespace)
            .map(|(kind, target)| (kind, target.trim()))
            .unwrap_or((inner, ""));
        if !matches_markdown_directive_kind(kind) || target.is_empty() {
            return None;
        }
        Some(MarkdownDirective {
            kind: kind.to_ascii_lowercase(),
            target: target.to_string(),
            line: node_line,
        })
    });
    extract_markdown_directives(line_context, &directives.collect::<Vec<_>>(), edges);
}

fn extract_markdown_reference_links(
    line_context: &MarkdownLineContext<'_>,
    links: &[MarkdownLink],
    edges: &mut Vec<ParsedEdge>,
) {
    for link in links {
        emit_markdown_link_edges(line_context, &link.target, link.line, edges);
    }
}

fn extract_markdown_reference_links_from_text(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    for (idx, line_text) in text.lines().enumerate() {
        let line = line_text.trim_start();
        let Some(label_end) = line.find("]:") else {
            continue;
        };
        if !line.starts_with('[') || label_end <= 1 {
            continue;
        }
        let target = normalize_link_target(line[label_end + 2..].trim());
        emit_markdown_link_edges(line_context, &target, idx as i64 + 1, edges);
    }
}

fn extract_markdown_inline_links(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut lines = LineCursor::new(text);
    for captures in MARKDOWN_INLINE_LINK_RE.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let raw_target = normalize_link_target(&captures[1]);
        let line = lines.line_for_offset(matched.start());
        emit_markdown_link_edges(line_context, &raw_target, line, edges);
    }
}

fn emit_markdown_link_edges(
    line_context: &MarkdownLineContext<'_>,
    raw_target: &str,
    line: i64,
    edges: &mut Vec<ParsedEdge>,
) {
    if raw_target.is_empty() || is_external_target(raw_target) {
        return;
    }
    let source = line_context.source_for_line(line);
    let file_path = line_context.file_path;
    let Some(target) = markdown_target(raw_target, file_path) else {
        return;
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

fn matches_markdown_directive_kind(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "constrained-by" | "blocked-by" | "supersedes" | "derived-from"
    )
}

fn extract_markdown_code_spans(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = std::collections::HashSet::new();
    let mut lines = LineCursor::new(text);
    for captures in MARKDOWN_CODE_SPAN_RE.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let sym = captures[1].trim();
        if sym.len() < 3 || !MARKDOWN_SYMBOL_RE.is_match(sym) {
            continue;
        }
        if !sym.contains('_') && !sym.contains('.') && sym.len() < 10 {
            continue;
        }
        let line = lines.line_for_offset(matched.start());
        let source = line_context.source_for_line(line);
        let file_path = line_context.file_path;
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
                "original_symbol_name": sym,
            }),
        });
    }
}

fn extract_markdown_dagayn_directives(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut lines = LineCursor::new(text);
    for (offset, _) in text.match_indices("<!--") {
        let rest = &text[offset..];
        let Some(end) = rest.find("-->") else {
            continue;
        };
        let full = &rest[..end + 3];
        let line = lines.line_for_offset(offset);
        let Some(inner) = full
            .trim()
            .strip_prefix("<!--")
            .and_then(|value| value.trim().strip_suffix("-->"))
            .map(str::trim)
        else {
            continue;
        };
        let Some(directive) = parse_dagayn_directive(inner, line) else {
            continue;
        };
        push_documentation_directive_edge(
            edges,
            line_context.source_for_line(line),
            line_context.file_path,
            "markdown",
            &directive,
            "markdown_directive",
        );
    }
}

fn normalize_link_target(target: &str) -> String {
    let mut target = target.trim().to_string();
    if target.is_empty() {
        return String::new();
    }
    if let Some(matched) = MARKDOWN_TITLE_RE.find(&target) {
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
