//! Where a Terraform block hands a value to something that reads it as code.
//!
//! The parser sees one file, but a Terraform bridge to code usually spans
//! several: a module call passes `source_path = "${path.module}/src"`, and a
//! resource in the module's directory reads `var.source_path`. So the parser
//! records facts ([`TerraformValue`]) on the block nodes, and the
//! `resolve_terraform_artifact_refs` post-processing step joins them across
//! files, checks the targets against the tree, and writes the
//! `CROSS_ARTIFACT` edges.
//!
//! Layer-1 / Layer-2 evidence only (no naming-only heuristics):
//! - `provisioner "local-exec"` command paths
//! - Lambda / function source path attributes (`filename`, `source_dir`, ...)
//! - Explicit entrypoint attributes (`handler`, `entry_point`)

use std::path::{Component, Path};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::terraform_collect::{
    TerraformAttr, TerraformBlock, find_matching_brace, strip_tf_string, terraform_attrs,
};
use super::types::FilePath;

static PROVISIONER_LOCAL_EXEC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"provisioner\s+"local-exec""#).unwrap());
static COMMAND_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)\bcommand\s*=\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')"#).unwrap()
});
static ENTRYPOINT_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)\b(handler|entry_point)\s*=\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')"#)
        .unwrap()
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

const PATH_ATTR_NAMES: &[&str] = &["filename", "source_dir", "source_file", "source_directory"];
const ENTRYPOINT_ATTR_NAMES: &[&str] = &["handler", "entry_point"];

const FILENAME_RESOURCE_TYPES: &[&str] = &[
    "aws_lambda_function",
    "aws_lambda_layer_version",
    "google_cloudfunctions_function",
    "google_cloudfunctions2_function",
    "azurerm_linux_function_app",
    "azurerm_windows_function_app",
    "azurerm_function_app",
];

/// The node `extra` key holding a block's [`TerraformValue`]s.
pub(super) const TERRAFORM_VALUES_KEY: &str = "terraform_values";

/// How the consumer of a value reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SinkRole {
    /// A path to code or a code package (`filename`, `source_dir`).
    CodePath,
    /// The name of the function the platform calls (`handler`).
    Entrypoint,
    /// A shell command run on the machine applying the plan.
    Command,
}

impl SinkRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::CodePath => "code_path",
            Self::Entrypoint => "entrypoint",
            Self::Command => "command",
        }
    }
}

/// What the value is handed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    /// An attribute of a resource or data block that reads it as code.
    Sink(SinkRole),
}

/// One attribute value and where it goes: the literal paths and symbol it
/// carries, and the `var.` / `local.` values it passes on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerraformValue {
    pub(super) slot: Slot,
    /// The attribute name (`filename`, `build_config.entry_point`), or
    /// `provisioner.local-exec.command`.
    pub(super) attr: String,
    pub(super) line_start: i64,
    pub(super) line_end: i64,
    /// Concrete repository-relative paths the value names.
    pub(super) paths: Vec<String>,
    /// The value itself when it is one string literal without interpolation.
    pub(super) symbol: Option<String>,
    /// `var.` / `local.` references whose value it takes.
    pub(super) refs: Vec<String>,
}

impl TerraformValue {
    pub(super) fn to_json(&self) -> Value {
        let mut value = json!({
            "attr": self.attr,
            "lines": [self.line_start, self.line_end],
            "paths": self.paths,
            "refs": self.refs,
        });
        match self.slot {
            Slot::Sink(role) => {
                value["slot"] = json!("sink");
                value["role"] = json!(role.as_str());
            }
        }
        if let Some(symbol) = &self.symbol {
            value["symbol"] = json!(symbol);
        }
        value
    }
}

/// The values a `resource` or `data` block hands to code.
pub(super) fn terraform_block_values(
    file_path: &FilePath,
    block: &TerraformBlock,
) -> Vec<TerraformValue> {
    if !matches!(block.kind.as_str(), "resource" | "data") {
        return Vec::new();
    }
    let Some(resource_type) = block.labels.first().map(String::as_str) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let attrs = terraform_attrs(block);
    for attr in attrs.iter() {
        if let Some(role) = sink_role(resource_type, &attr.name) {
            values.push(attr_value(file_path, Slot::Sink(role), attr));
        }
    }
    match &block.nested_attrs {
        Some(nested) => {
            for attr in nested {
                let leaf = attr.name.rsplit('.').next().unwrap_or(&attr.name);
                if ENTRYPOINT_ATTR_NAMES.contains(&leaf) {
                    values.push(attr_value(
                        file_path,
                        Slot::Sink(SinkRole::Entrypoint),
                        attr,
                    ));
                }
            }
        }
        None => values.extend(nested_entrypoints_from_text(block, &attrs)),
    }
    values.extend(local_exec_values(file_path, block));
    values
}

fn sink_role(resource_type: &str, attr: &str) -> Option<SinkRole> {
    if ENTRYPOINT_ATTR_NAMES.contains(&attr) {
        return Some(SinkRole::Entrypoint);
    }
    if !PATH_ATTR_NAMES.contains(&attr) {
        return None;
    }
    let filename_owner = FILENAME_RESOURCE_TYPES.contains(&resource_type)
        || resource_type.contains("lambda")
        || resource_type.contains("function")
        || resource_type.contains("cloudfunction");
    (attr != "filename" || filename_owner).then_some(SinkRole::CodePath)
}

fn attr_value(file_path: &FilePath, slot: Slot, attr: &TerraformAttr) -> TerraformValue {
    let literal = strip_tf_string(&attr.value);
    let is_literal = attr.value.trim_start().starts_with(['"', '\'']) && !literal.contains("${");
    let paths = match slot {
        Slot::Sink(SinkRole::CodePath) => concrete_tf_path(&attr.value, file_path)
            .into_iter()
            .collect(),
        Slot::Sink(_) => Vec::new(),
    };
    TerraformValue {
        slot,
        attr: attr.name.clone(),
        line_start: attr.line_start,
        line_end: attr.line_end,
        paths,
        symbol: (is_literal && !literal.trim().is_empty()).then_some(literal),
        refs: attr.value_refs.clone().unwrap_or_default(),
    }
}

/// Without a syntax tree there are no nested attributes; find entrypoints
/// that sit in nested blocks by their text.
fn nested_entrypoints_from_text(
    block: &TerraformBlock,
    top_level: &[TerraformAttr],
) -> Vec<TerraformValue> {
    let mut values = Vec::new();
    for captures in ENTRYPOINT_ATTR_RE.captures_iter(&block.body) {
        let line = block.body_start_line
            + newlines(&block.body[..captures.get(0).map_or(0, |m| m.start())]);
        if top_level.iter().any(|attr| attr.line_start == line) {
            continue;
        }
        let raw = strip_tf_string(captures.get(2).map_or("", |m| m.as_str()));
        if raw.is_empty() || raw.contains("${") {
            continue;
        }
        values.push(TerraformValue {
            slot: Slot::Sink(SinkRole::Entrypoint),
            attr: captures
                .get(1)
                .map_or("handler", |m| m.as_str())
                .to_string(),
            line_start: line,
            line_end: line,
            paths: Vec::new(),
            symbol: Some(raw),
            refs: Vec::new(),
        });
    }
    values
}

fn local_exec_values(file_path: &FilePath, block: &TerraformBlock) -> Vec<TerraformValue> {
    let body = &block.body;
    let mut values = Vec::new();
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
        let Some(command_cap) = COMMAND_ATTR_RE.captures(provisioner_body) else {
            continue;
        };
        let command = strip_tf_string(&command_cap[1]);
        let paths: Vec<String> = PATH_TOKEN_RE
            .find_iter(&command)
            .filter_map(|token| concrete_tf_path(token.as_str(), file_path))
            .collect();
        if paths.is_empty() {
            continue;
        }
        values.push(TerraformValue {
            slot: Slot::Sink(SinkRole::Command),
            attr: "provisioner.local-exec.command".to_string(),
            line_start: block.body_start_line + newlines(&body[..open_abs]),
            line_end: block.body_start_line + newlines(&body[..close_abs]),
            paths,
            symbol: None,
            refs: Vec::new(),
        });
    }
    values
}

fn newlines(text: &str) -> i64 {
    text.bytes().filter(|byte| *byte == b'\n').count() as i64
}

fn concrete_tf_path(raw: &str, file_path: &FilePath) -> Option<String> {
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
    // `${path.module}/function` or `./function` is a path by construction;
    // only a bare word needs a slash or a code extension to count as one.
    let module_relative = value.contains("${path.module}")
        || value.contains("${path.root}")
        || value.starts_with("./")
        || value.starts_with("../");
    if !(module_relative || normalized.contains('/') || looks_like_code_path(&normalized)) {
        return None;
    }

    let base = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let joined = if Path::new(&normalized).is_absolute() {
        Path::new(&normalized).to_path_buf()
    } else if module_relative {
        base.join(&normalized)
    } else if normalized.contains('/') {
        // Repo-relative path without path.module — keep as written.
        Path::new(&normalized).to_path_buf()
    } else {
        base.join(&normalized)
    };
    let path = normalize_within_repo(&joined)?;
    (!path.is_empty()).then_some(path)
}

/// `a/b/../c` as `a/c`; `None` when `..` climbs above the repository root.
fn normalize_within_repo(path: &Path) -> Option<String> {
    let mut parts = Vec::<String>::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::RootDir | Component::Prefix(_) => {
                parts.push(component.as_os_str().to_string_lossy().to_string());
            }
        }
    }
    Some(parts.join("/"))
}

fn looks_like_code_path(path: &str) -> bool {
    [
        ".py", ".sh", ".bash", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".go", ".rs", ".rb",
        ".php", ".pl", ".R", ".jl", ".lua", ".ex", ".exs", ".zip",
    ]
    .iter()
    .any(|ext| path.ends_with(ext))
}
