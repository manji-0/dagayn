//! High-confidence Terraform → application-code CROSS_ARTIFACT bridges.
//!
//! Layer-1 / Layer-2 patterns only (no naming-only Layer-3 heuristics):
//! - `provisioner "local-exec"` command paths
//! - Lambda / function source path attributes (`filename`, `source_dir`, …)
//! - Explicit entrypoint attributes (`handler`, `entry_point`)

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use super::terraform_collect::{strip_tf_string, terraform_attrs, TerraformBlock};
use super::types::{EdgeKind, ParsedEdge};
use super::util::normalize_relative_path;

static PROVISIONER_LOCAL_EXEC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"provisioner\s+"local-exec""#).unwrap());
static COMMAND_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)\bcommand\s*=\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')"#).unwrap());
static ENTRYPOINT_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)\b(handler|entry_point)\s*=\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')"#).unwrap()
});
static PATH_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?x)
        (?:\$\{path\.(?:module|root)\}/|\./|\.\./|/)?
        [\w.-]+(?:/[\w.-]+)+
        (?:\.[A-Za-z0-9_+-]+)?
        |
        [\w.-]+\.(?:py|sh|bash|js|mjs|cjs|ts|tsx|jsx|go|rs|rb|php|pl|R|jl|lua|ex|exs|zip)
        "#,
    )
    .unwrap()
});

const PATH_ATTR_NAMES: &[&str] = &[
    "filename",
    "source_dir",
    "source_file",
    "source_directory",
];

const FILENAME_RESOURCE_TYPES: &[&str] = &[
    "aws_lambda_function",
    "aws_lambda_layer_version",
    "google_cloudfunctions_function",
    "google_cloudfunctions2_function",
    "azurerm_linux_function_app",
    "azurerm_windows_function_app",
    "azurerm_function_app",
];

pub(super) fn extract_terraform_code_bridges(
    file_path: &str,
    blocks: &[TerraformBlock],
    edges: &mut Vec<ParsedEdge>,
) {
    for block in blocks {
        if !matches!(block.kind.as_str(), "resource" | "data") {
            continue;
        }
        let Some(resource_type) = block.labels.first().map(String::as_str) else {
            continue;
        };
        let Some(resource_name) = block.labels.get(1).map(String::as_str) else {
            continue;
        };
        let node_name = format!("{}.{}.{}", block.kind, resource_type, resource_name);
        let source = format!("{file_path}::{node_name}");

        extract_path_attr_bridges(
            file_path,
            &source,
            resource_type,
            block,
            edges,
        );
        extract_local_exec_bridges(file_path, &source, block, edges);
        extract_entrypoint_bridges(file_path, &source, block, edges);
    }
}

fn extract_path_attr_bridges(
    file_path: &str,
    source: &str,
    resource_type: &str,
    block: &TerraformBlock,
    edges: &mut Vec<ParsedEdge>,
) {
    for attr in terraform_attrs(block).iter() {
        if !PATH_ATTR_NAMES.contains(&attr.name.as_str()) {
            continue;
        }
        if attr.name == "filename"
            && !FILENAME_RESOURCE_TYPES.contains(&resource_type)
            && !resource_type.contains("lambda")
            && !resource_type.contains("function")
            && !resource_type.contains("cloudfunction")
        {
            continue;
        }
        let Some(path) = concrete_tf_path(&attr.value, file_path) else {
            continue;
        };
        push_bridge_edge(
            edges,
            source,
            &path,
            file_path,
            attr.line_start,
            BridgeSpec {
                relationship_role: "maps_entrypoint",
                bridge_kind: "manifest_link",
                evidence_kind: "config",
                evidence_source: attr.name.as_str(),
                target_language: target_language_for_path(&path),
                original_symbol_name: None,
            },
        );
    }
}

fn extract_local_exec_bridges(
    file_path: &str,
    source: &str,
    block: &TerraformBlock,
    edges: &mut Vec<ParsedEdge>,
) {
    let body = &block.body;
    for provisioner in PROVISIONER_LOCAL_EXEC_RE.find_iter(body) {
        let after = &body[provisioner.end()..];
        let Some(open_rel) = after.find('{') else {
            continue;
        };
        let open_abs = provisioner.end() + open_rel;
        let Some(close_abs) = find_matching_brace(body, open_abs) else {
            continue;
        };
        let provisioner_body = &body[open_abs + 1..close_abs];
        let line = block.body_start_line
            + body[..open_abs]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count() as i64;
        let Some(command_cap) = COMMAND_ATTR_RE.captures(provisioner_body) else {
            continue;
        };
        let command = strip_tf_string(&command_cap[1]);
        for token in PATH_TOKEN_RE.find_iter(&command) {
            let Some(path) = concrete_tf_path(token.as_str(), file_path) else {
                continue;
            };
            push_bridge_edge(
                edges,
                source,
                &path,
                file_path,
                line,
                BridgeSpec {
                    relationship_role: "invokes_binary",
                    bridge_kind: "subprocess",
                    evidence_kind: "syntax",
                    evidence_source: "provisioner.local-exec.command",
                    target_language: target_language_for_path(&path),
                    original_symbol_name: None,
                },
            );
        }
    }
}

fn extract_entrypoint_bridges(
    file_path: &str,
    source: &str,
    block: &TerraformBlock,
    edges: &mut Vec<ParsedEdge>,
) {
    for captures in ENTRYPOINT_ATTR_RE.captures_iter(&block.body) {
        let attr_name = captures.get(1).map(|m| m.as_str()).unwrap_or("handler");
        let raw = strip_tf_string(captures.get(2).map(|m| m.as_str()).unwrap_or(""));
        if raw.is_empty() || raw.contains("${") {
            continue;
        }
        let line = block.body_start_line
            + block.body[..captures.get(0).map(|m| m.start()).unwrap_or(0)]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count() as i64;
        push_bridge_edge(
            edges,
            source,
            &format!("<unresolved:{raw}>"),
            file_path,
            line,
            BridgeSpec {
                relationship_role: "maps_entrypoint",
                bridge_kind: "manifest_link",
                evidence_kind: "config",
                evidence_source: attr_name,
                target_language: "unknown",
                original_symbol_name: Some(raw),
            },
        );
    }
}

struct BridgeSpec<'a> {
    relationship_role: &'a str,
    bridge_kind: &'a str,
    evidence_kind: &'a str,
    evidence_source: &'a str,
    target_language: &'a str,
    original_symbol_name: Option<String>,
}

fn push_bridge_edge(
    edges: &mut Vec<ParsedEdge>,
    source: &str,
    target: &str,
    file_path: &str,
    line: i64,
    spec: BridgeSpec<'_>,
) {
    let mut extra = json!({
        "relationship_role": spec.relationship_role,
        "bridge_kind": spec.bridge_kind,
        "evidence_kind": spec.evidence_kind,
        "evidence_source": spec.evidence_source,
        "source_language": "terraform",
        "target_language": spec.target_language,
        "confidence": 0.8,
        "confidence_tier": "HIGH",
    });
    if let Some(symbol) = spec.original_symbol_name {
        extra["original_symbol_name"] = json!(symbol);
    }
    edges.push(ParsedEdge {
        kind: EdgeKind::CrossArtifact.as_str().to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line,
        extra,
    });
}

fn concrete_tf_path(raw: &str, file_path: &str) -> Option<String> {
    let value = strip_tf_string(raw).trim().to_string();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("s3://")
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("gs://")
        || value.starts_with("azurerm://")
    {
        return None;
    }

    let mut normalized = value.clone();
    for (pattern, replacement) in [
        ("${path.module}/", ""),
        ("${path.module}", ""),
        ("${path.root}/", ""),
        ("${path.root}", ""),
    ] {
        normalized = normalized.replace(pattern, replacement);
    }
    // Reject remaining interpolations — only concrete paths are high-confidence.
    if normalized.contains("${") {
        return None;
    }
    let normalized = normalized
        .strip_prefix("./")
        .unwrap_or(normalized.as_str())
        .trim()
        .to_string();
    if normalized.is_empty() {
        return None;
    }
    if !(normalized.contains('/')
        || looks_like_code_path(&normalized)
        || normalized.starts_with("../"))
    {
        return None;
    }

    let base = Path::new(file_path).parent().unwrap_or_else(|| Path::new(""));
    let joined = if Path::new(&normalized).is_absolute() {
        Path::new(&normalized).to_path_buf()
    } else if value.contains("${path.module}")
        || value.contains("${path.root}")
        || value.starts_with("./")
        || value.starts_with("../")
    {
        base.join(&normalized)
    } else if normalized.contains('/') {
        // Repo-relative path without path.module — keep as written.
        Path::new(&normalized).to_path_buf()
    } else {
        base.join(&normalized)
    };
    let path = normalize_relative_path(&joined);
    (!path.is_empty()).then_some(path)
}

fn looks_like_code_path(path: &str) -> bool {
    [
        ".py", ".sh", ".bash", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".go", ".rs",
        ".rb", ".php", ".pl", ".R", ".jl", ".lua", ".ex", ".exs", ".zip",
    ]
    .iter()
    .any(|ext| path.ends_with(ext))
}

fn target_language_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".py") {
        "python"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        "javascript"
    } else if lower.ends_with(".ts") || lower.ends_with(".tsx") {
        "typescript"
    } else if lower.ends_with(".go") {
        "go"
    } else if lower.ends_with(".rs") {
        "rust"
    } else if lower.ends_with(".sh") || lower.ends_with(".bash") {
        "bash"
    } else {
        "unknown"
    }
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
