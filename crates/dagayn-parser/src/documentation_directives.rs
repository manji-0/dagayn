use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::qualify;
use super::types::{EdgeKind, FilePath, ParsedEdge, ParsedNode};
use super::util::normalize_relative_path;

static DAGAYN_DIRECTIVE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bdagayn:\s*([A-Za-z][A-Za-z-]*)\s+(.+?)\s*$").unwrap());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DocumentationDirectiveKind {
    ImplementedBy,
    Implements,
    ExplainedBy,
    HasRunbook,
    ProblemDescribedBy,
    DiscussedBy,
    DiscussesArtifact,
    RaisesIssueFor,
    DescribesSymbol,
}

impl DocumentationDirectiveKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "implemented-by" => Some(Self::ImplementedBy),
            "implements" => Some(Self::Implements),
            "explained-by" => Some(Self::ExplainedBy),
            "has-runbook" => Some(Self::HasRunbook),
            "problem-described-by" => Some(Self::ProblemDescribedBy),
            "discussed-by" => Some(Self::DiscussedBy),
            "discusses" | "discusses-artifact" => Some(Self::DiscussesArtifact),
            "raises-issue-for" => Some(Self::RaisesIssueFor),
            "describes" | "describes-symbol" => Some(Self::DescribesSymbol),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ImplementedBy => "implemented-by",
            Self::Implements => "implements",
            Self::ExplainedBy => "explained-by",
            Self::HasRunbook => "has-runbook",
            Self::ProblemDescribedBy => "problem-described-by",
            Self::DiscussedBy => "discussed-by",
            Self::DiscussesArtifact => "discusses-artifact",
            Self::RaisesIssueFor => "raises-issue-for",
            Self::DescribesSymbol => "describes-symbol",
        }
    }

    fn relationship_role(self) -> &'static str {
        match self {
            Self::ImplementedBy => "implemented_by",
            Self::Implements => "implements_contract",
            Self::ExplainedBy => "explained_by",
            Self::HasRunbook => "has_runbook",
            Self::ProblemDescribedBy => "problem_described_by",
            Self::DiscussedBy => "discussed_by",
            Self::DiscussesArtifact => "discusses_artifact",
            Self::RaisesIssueFor => "raises_issue_for",
            Self::DescribesSymbol => "describes_symbol",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct DocumentationDirective {
    pub directive_kind: DocumentationDirectiveKind,
    pub target: String,
    pub line: i64,
}

pub(super) fn parse_dagayn_directive(
    line: &str,
    line_number: i64,
) -> Option<DocumentationDirective> {
    let captures = DAGAYN_DIRECTIVE_RE.captures(line)?;
    let directive_kind = DocumentationDirectiveKind::parse(captures.get(1)?.as_str())?;
    let target = captures
        .get(2)?
        .as_str()
        .trim()
        .trim_end_matches("-->")
        .trim()
        .to_string();
    (!target.is_empty()).then_some(DocumentationDirective {
        directive_kind,
        target,
        line: line_number,
    })
}

pub(super) fn extract_line_comment_dagayn_directives(
    text: &str,
    comment_prefixes: &[&str],
) -> Vec<DocumentationDirective> {
    let mut directives = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(comment) = comment_prefixes
            .iter()
            .find_map(|prefix| trimmed.strip_prefix(prefix))
        else {
            continue;
        };
        if let Some(directive) = parse_dagayn_directive(comment.trim_start(), index as i64 + 1) {
            directives.push(directive);
        }
    }
    directives
}

pub(super) fn push_documentation_directive_edge(
    edges: &mut Vec<ParsedEdge>,
    source: String,
    source_file: &FilePath,
    source_language: &str,
    directive: &DocumentationDirective,
    evidence_kind: &str,
) {
    let resolved = directive_target(&directive.target, source_file);
    let mut extra = documentation_directive_extra(
        directive,
        source_language,
        target_language_hint(resolved.target()),
        resolved.unresolved_symbol(),
        evidence_kind,
        resolved.confidence(),
        resolved.confidence_tier(),
    );
    if resolved.unresolved_symbol().is_some() {
        extra["target_language"] = json!("unknown");
    }
    edges.push(ParsedEdge {
        kind: EdgeKind::CrossArtifact,
        source,
        target: resolved.target().to_string(),
        file_path: source_file.clone(),
        line: directive.line,
        extra,
    });
}

pub(super) fn nearest_documentation_source(
    file_path: &FilePath,
    nodes: &[ParsedNode],
    line: i64,
) -> String {
    if let Some(node) = nodes
        .iter()
        .filter(|node| node.kind != "File" && node.line_start <= line && line <= node.line_end)
        .min_by_key(|node| node.line_end - node.line_start)
    {
        return qualified_node_name(file_path, node);
    }

    if let Some(node) = nodes
        .iter()
        .filter(|node| node.kind != "File" && node.line_start > line && node.line_start - line <= 3)
        .min_by_key(|node| node.line_start)
    {
        return qualified_node_name(file_path, node);
    }

    file_path.to_string()
}

enum DirectiveTarget {
    Direct { target: String },
    Unresolved { symbol: String, target: String },
}

impl DirectiveTarget {
    fn target(&self) -> &str {
        match self {
            Self::Direct { target } | Self::Unresolved { target, .. } => target,
        }
    }

    fn unresolved_symbol(&self) -> Option<&str> {
        match self {
            Self::Direct { .. } => None,
            Self::Unresolved { symbol, .. } => Some(symbol),
        }
    }

    fn confidence(&self) -> f64 {
        match self {
            Self::Direct { .. } => 0.8,
            Self::Unresolved { .. } => 0.2,
        }
    }

    fn confidence_tier(&self) -> &'static str {
        match self {
            Self::Direct { .. } => "HIGH",
            Self::Unresolved { .. } => "LOW",
        }
    }
}

fn directive_target(raw_target: &str, source_file: &str) -> DirectiveTarget {
    let target = raw_target.trim();
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("tel:")
    {
        return direct_target(target.to_string());
    }

    if let Some(section) = target.strip_prefix('#') {
        let slug = documentation_slugify(section);
        return direct_target(format!("{source_file}::{slug}"));
    }

    if let Some((path, symbol)) = target.split_once("::") {
        let normalized_path = normalize_directive_path(path, source_file);
        return direct_target(format!("{normalized_path}::{symbol}"));
    }

    if let Some((path, section)) = target.split_once('#') {
        let normalized_path = normalize_directive_path(path, source_file);
        let slug = documentation_slugify(section);
        return direct_target(format!("{normalized_path}::{slug}"));
    }

    if target.starts_with("./") || target.starts_with("../") || looks_like_file_target(target) {
        return direct_target(normalize_directive_path(target, source_file));
    }

    DirectiveTarget::Unresolved {
        symbol: target.to_string(),
        target: format!("<unresolved:{target}>"),
    }
}

fn direct_target(target: String) -> DirectiveTarget {
    DirectiveTarget::Direct { target }
}

fn normalize_directive_path(path: &str, source_file: &str) -> String {
    if path.starts_with("./") || path.starts_with("../") {
        return normalize_relative_path(
            &Path::new(source_file)
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(path),
        );
    }
    normalize_relative_path(Path::new(path))
}

fn looks_like_file_target(target: &str) -> bool {
    target.contains('/')
        || target.ends_with(".md")
        || target.ends_with(".markdown")
        || target.ends_with(".py")
        || target.ends_with(".tf")
        || target.ends_with(".tfvars")
        || target.ends_with(".rs")
        || target.ends_with(".js")
        || target.ends_with(".ts")
        || target.ends_with(".tsx")
        || target.ends_with(".jsx")
}

fn target_language_hint(target: &str) -> &'static str {
    let path = target
        .split_once("::")
        .map(|(path, _)| path)
        .unwrap_or(target);
    if path.ends_with(".md") || path.ends_with(".markdown") {
        "markdown"
    } else if path.ends_with(".tf") || path.ends_with(".tfvars") {
        "terraform"
    } else if path.ends_with(".py") {
        "python"
    } else if path.ends_with(".rs") {
        "rust"
    } else if path.ends_with(".js") || path.ends_with(".jsx") {
        "javascript"
    } else if path.ends_with(".ts") || path.ends_with(".tsx") {
        "typescript"
    } else {
        "unknown"
    }
}

fn documentation_directive_extra(
    directive: &DocumentationDirective,
    source_language: &str,
    target_language: &str,
    unresolved_symbol: Option<&str>,
    evidence_kind: &str,
    confidence: f64,
    confidence_tier: &str,
) -> Value {
    let mut extra = json!({
        "relationship_role": directive.directive_kind.relationship_role(),
        "bridge_kind": "documentation",
        "evidence_kind": evidence_kind,
        "evidence_source": "dagayn_directive",
        "source_language": source_language,
        "target_language": target_language,
        "confidence": confidence,
        "confidence_tier": confidence_tier,
        "dagayn_directive_kind": directive.directive_kind.as_str(),
    });
    if let Some(symbol) = unresolved_symbol {
        extra["original_symbol_name"] = json!(symbol);
    }
    extra
}

fn documentation_slugify(text: &str) -> String {
    let mut out = String::new();
    for char in text.trim().chars() {
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

fn qualified_node_name(file_path: &FilePath, node: &ParsedNode) -> String {
    if node.kind == "File" {
        file_path.to_string()
    } else {
        qualify(&file_path, &node.name, node.parent_name.as_deref())
    }
}
