use std::collections::HashSet;

use serde_json::json;

use super::terraform_collect::{
    collect_terraform_blocks, collect_terraform_reference_targets, strip_tf_string,
    terraform_attrs, terraform_provider_sources, TerraformAttr, TerraformBlock, TERRAFORM_CALL_RE,
};
use super::types::{ParsedEdge, ParsedNode};
use super::util::{dedupe_edges, is_test_file, line_count};

pub(super) fn parse_terraform_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let line_end = line_count(source);
    let blocks = collect_terraform_blocks(source, &text, parser);
    let mut defined_names = HashSet::new();
    for block in &blocks {
        if block.kind == "locals" {
            for attr in terraform_attrs(block).iter() {
                defined_names.insert(format!("local.{}", attr.name));
            }
        } else if let Some(name) = terraform_defined_name(block) {
            defined_names.insert(name);
        }
    }

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
            for attr in terraform_attrs(block).iter() {
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
                scan_terraform_attr(
                    attr,
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
        scan_terraform_block(
            block,
            &node_name,
            file_path,
            block.line_start,
            &defined_names,
            &mut edges,
        );

        if block.kind == "module" {
            if let Some(source_attr) = terraform_attrs(block)
                .iter()
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
            for provider_source in terraform_provider_sources(block).iter() {
                edges.push(ParsedEdge {
                    kind: "DEPENDS_ON".to_string(),
                    source: terraform_qualified(file_path, &node_name),
                    target: provider_source.clone(),
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({}),
                });
            }
        }
    }

    (nodes, dedupe_edges(edges))
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

struct TerraformNodeSpec<'a> {
    kind: &'a str,
    name: &'a str,
    line_start: i64,
    line_end: i64,
    is_test: bool,
    terraform_kind: &'a str,
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
    let attrs = terraform_attrs(block);
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
    scan_terraform_block(
        block,
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

fn scan_terraform_block(
    block: &TerraformBlock,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let (Some(calls), Some(references)) = (&block.calls, &block.references) {
        push_terraform_calls(calls, caller, file_path, line, edges);
        push_terraform_references(references, caller, file_path, line, defined_names, edges);
    } else {
        scan_terraform_body(&block.body, caller, file_path, line, defined_names, edges);
    }
}

fn scan_terraform_attr(
    attr: &TerraformAttr,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let (Some(calls), Some(references)) = (&attr.calls, &attr.references) {
        push_terraform_calls(calls, caller, file_path, line, edges);
        push_terraform_references(references, caller, file_path, line, defined_names, edges);
    } else {
        scan_terraform_body(&attr.text, caller, file_path, line, defined_names, edges);
    }
}

fn collect_terraform_calls(
    text: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    edges: &mut Vec<ParsedEdge>,
) {
    let calls = TERRAFORM_CALL_RE
        .captures_iter(text)
        .map(|captures| captures[1].to_string())
        .filter(|name| !matches!(name.as_str(), "for" | "if"))
        .collect::<Vec<_>>();
    push_terraform_calls(&calls, caller, file_path, line, edges);
}

fn push_terraform_calls(
    calls: &[String],
    caller: &str,
    file_path: &str,
    line: i64,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = HashSet::new();
    for name in calls {
        if !seen.insert(name.clone()) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.to_string(),
            target: name.clone(),
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
    let references = collect_terraform_reference_targets(text);
    push_terraform_references(&references, caller, file_path, line, defined_names, edges);
}

fn push_terraform_references(
    references: &[String],
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = HashSet::new();
    for target in references {
        if target == caller || !seen.insert(target.clone()) {
            continue;
        }
        let resolved = if defined_names.contains(target) {
            terraform_qualified(file_path, target)
        } else {
            target.clone()
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

fn terraform_qualified(file_path: &str, name: &str) -> String {
    format!("{file_path}::{name}")
}
