use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::util::{line_for_offset, node_text, node_text_is, node_text_tf_string_is};

pub(super) static TERRAFORM_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$"#).unwrap());
pub(super) static TERRAFORM_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap());
pub(super) static TERRAFORM_HEADER_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]*)"|'([^']*)'|([A-Za-z_][A-Za-z0-9_-]*)"#).unwrap());
pub(super) static TERRAFORM_PROVIDER_SOURCE_FALLBACK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"source\s*=\s*(["'][^"']+["'])"#).unwrap());
pub(super) static TERRAFORM_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:(data)\.([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)|((?:module|var|local|output|provider|check))\.([A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*))\b",
    )
    .unwrap()
});

pub(super) struct TerraformBlock {
    pub(super) kind: String,
    pub(super) labels: Vec<String>,
    pub(super) body: String,
    pub(super) line_start: i64,
    pub(super) line_end: i64,
    pub(super) body_start_line: i64,
    pub(super) attrs: Option<Vec<TerraformAttr>>,
    pub(super) calls: Option<Vec<String>>,
    pub(super) references: Option<Vec<String>>,
    pub(super) provider_sources: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub(super) struct TerraformAttr {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) text: String,
    pub(super) line_start: i64,
    pub(super) line_end: i64,
    pub(super) calls: Option<Vec<String>>,
    pub(super) references: Option<Vec<String>>,
}

pub(super) fn collect_terraform_blocks(
    source: &[u8],
    text: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> Vec<TerraformBlock> {
    if let Some(parser) = parser {
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
    let body_calls = body_node.map(|body| collect_terraform_calls_from_tree(body, source));
    let body_references =
        body_node.map(|body| collect_terraform_references_from_tree(body, source));
    let provider_sources = body_node.map(|body| collect_terraform_provider_sources(body, source));

    Some(TerraformBlock {
        kind,
        labels,
        body,
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        body_start_line,
        attrs: body_node.map(|body| collect_terraform_attrs_from_tree(body, source)),
        calls: body_calls,
        references: body_references,
        provider_sources,
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
        "action_block" => Some("action"),
        "run_block" => Some("run"),
        "mock_provider_block" => Some("mock_provider"),
        "variables_block" => Some("variables"),
        "override_resource_block" => Some("override_resource"),
        "override_data_block" => Some("override_data"),
        "override_module_block" => Some("override_module"),
        "component_block" => Some("component"),
        "required_providers_block" => Some("required_providers"),
        "identity_token_block" => Some("identity_token"),
        "store_block" => Some("store"),
        "deployment_block" => Some("deployment"),
        "deployment_group_block" => Some("deployment_group"),
        "orchestrate_block" => Some("orchestrate"),
        "publish_output_block" => Some("publish_output"),
        "upstream_input_block" => Some("upstream_input"),
        "list_block" => Some("list"),
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
            attrs: None,
            calls: None,
            references: None,
            provider_sources: None,
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
            | "action"
            | "run"
            | "mock_provider"
            | "variables"
            | "override_resource"
            | "override_data"
            | "override_module"
            | "component"
            | "required_providers"
            | "identity_token"
            | "store"
            | "deployment"
            | "deployment_group"
            | "orchestrate"
            | "publish_output"
            | "upstream_input"
            | "list"
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
            calls: None,
            references: None,
        });
    }
    attrs
}

pub(super) fn terraform_attrs(block: &TerraformBlock) -> Cow<'_, [TerraformAttr]> {
    block.attrs.as_deref().map_or_else(
        || Cow::Owned(collect_terraform_attrs(&block.body, block.body_start_line)),
        Cow::Borrowed,
    )
}

pub(super) fn terraform_provider_sources(block: &TerraformBlock) -> Cow<'_, [String]> {
    block.provider_sources.as_deref().map_or_else(
        || {
            Cow::Owned(
                TERRAFORM_PROVIDER_SOURCE_FALLBACK_RE
                    .captures_iter(&block.body)
                    .map(|captures| strip_tf_string(&captures[1]))
                    .collect(),
            )
        },
        Cow::Borrowed,
    )
}

fn collect_terraform_attrs_from_tree(
    body: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<TerraformAttr> {
    let mut attrs = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "attribute" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(value_node) = child.child_by_field_name("value") else {
            continue;
        };
        attrs.push(TerraformAttr {
            name: node_text(name_node, source),
            value: node_text(value_node, source).trim().to_string(),
            text: node_text(child, source),
            line_start: child.start_position().row as i64 + 1,
            line_end: child.end_position().row as i64 + 1,
            calls: Some(collect_terraform_calls_from_tree(child, source)),
            references: Some(collect_terraform_references_from_tree(child, source)),
        });
    }
    attrs
}

fn collect_terraform_provider_sources(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut sources = Vec::new();
    collect_terraform_provider_source_nodes(node, source, &mut sources);
    dedupe_strings(sources)
}

fn collect_terraform_provider_source_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    sources: &mut Vec<String>,
) {
    if node.kind() == "attribute" {
        if let (Some(name), Some(value)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ) {
            if node_text_is(name, source, "source") {
                sources.push(strip_tf_string(&node_text(value, source)));
            }
        }
    } else if node.kind() == "object_elem" {
        if let (Some(key), Some(value)) = (
            node.child_by_field_name("key"),
            node.child_by_field_name("value"),
        ) {
            if node_text_tf_string_is(key, source, "source") {
                sources.push(strip_tf_string(&node_text(value, source)));
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_provider_source_nodes(child, source, sources);
    }
}

fn collect_terraform_calls_from_tree(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut calls = Vec::new();
    collect_terraform_call_nodes(node, source, &mut calls);
    dedupe_strings(calls)
}

fn collect_terraform_call_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    calls: &mut Vec<String>,
) {
    if node.kind() == "function_call" {
        if let Some(name) = node.child_by_field_name("name") {
            let name = node_text(name, source);
            if !matches!(name.as_str(), "for" | "if") {
                calls.push(name);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_call_nodes(child, source, calls);
    }
}

fn collect_terraform_references_from_tree(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<String> {
    let mut references = Vec::new();
    collect_terraform_reference_nodes(node, source, &mut references);
    dedupe_strings(references)
}

fn collect_terraform_reference_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    references: &mut Vec<String>,
) {
    if matches!(node.kind(), "template_expr" | "quoted_template") {
        references.extend(collect_terraform_reference_targets(&node_text(
            node, source,
        )));
    }
    if node.kind() == "expression" {
        if let Some(segments) = terraform_traversal_segments(node, source) {
            if let Some(target) = terraform_reference_from_segments(&segments) {
                references.push(target);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_reference_nodes(child, source, references);
    }
}

fn terraform_traversal_segments(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    if node.kind() == "variable_expr" {
        return node
            .child(0)
            .filter(|child| child.kind() == "identifier")
            .map(|identifier| vec![node_text(identifier, source)]);
    }

    if node.kind() != "expression" {
        return None;
    }

    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    if children.len() == 1 {
        return terraform_traversal_segments(children[0], source);
    }

    let mut segments = terraform_traversal_segments(*children.first()?, source)?;
    for child in children.iter().skip(1) {
        if child.kind() != "get_attr" {
            return None;
        }
        let name = child.child_by_field_name("name")?;
        segments.push(node_text(name, source));
    }
    Some(segments)
}

fn terraform_reference_from_segments(segments: &[String]) -> Option<String> {
    let root = segments.first()?.as_str();
    if root == "data" {
        return segments
            .get(1)
            .zip(segments.get(2))
            .map(|(block_type, name)| format!("data.{block_type}.{name}"));
    }
    if matches!(
        root,
        "module" | "var" | "local" | "output" | "provider" | "check"
    ) {
        return segments.get(1).map(|name| format!("{root}.{name}"));
    }
    if matches!(
        root,
        "count" | "each" | "ingress" | "egress" | "path" | "self" | "terraform"
    ) {
        return None;
    }
    segments
        .get(1)
        .map(|name| format!("resource.{root}.{name}"))
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
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

pub(super) fn collect_terraform_reference_targets(text: &str) -> Vec<String> {
    TERRAFORM_REFERENCE_RE
        .captures_iter(text)
        .filter_map(|captures| {
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
                    return None;
                }
                format!("resource.{}.{}", root, &captures[7])
            };
            Some(target)
        })
        .collect::<Vec<_>>()
}

pub(super) fn strip_tf_string(value: &str) -> String {
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
