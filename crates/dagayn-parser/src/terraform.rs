use std::collections::HashSet;

use serde_json::{json, Value};

use super::documentation_directives::{
    extract_line_comment_dagayn_directives, nearest_documentation_source,
    push_documentation_directive_edge,
};
use super::terraform_bridges::extract_terraform_code_bridges;
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
    if file_path.ends_with(".tf.json") || file_path.ends_with(".tfvars.json") {
        return parse_terraform_json(file_path, source);
    }
    parse_terraform_hcl(file_path, source, parser)
}

/// Parse Terraform JSON syntax (`.tf.json` / `.tfvars.json`) directly from the
/// JSON document; the tree-sitter grammar only understands HCL.
fn parse_terraform_json(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let Ok(value) = serde_json::from_slice::<Value>(source) else {
        return (Vec::new(), Vec::new());
    };
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File.as_str().to_string(),
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

    let Some(root) = value.as_object() else {
        return (nodes, edges);
    };
    for (kind, value) in root {
        let Some(entries) = value.as_object() else {
            continue;
        };
        match kind.as_str() {
            "resource" | "data" | "ephemeral" => {
                for (block_type, names) in entries {
                    let Some(names) = names.as_object() else {
                        continue;
                    };
                    for name in names.keys() {
                        let node_name = format!("{kind}.{block_type}.{name}");
                        push_terraform_node(
                            file_path,
                            &mut nodes,
                            &mut edges,
                            TerraformNodeSpec {
                                kind: "Class",
                                name: &node_name,
                                line_start: 1,
                                line_end,
                                is_test: false,
                                terraform_kind: kind,
                            },
                        );
                    }
                }
            }
            "variable" | "output" | "check" => {
                for (name, _) in entries {
                    let label = if kind == "variable" {
                        format!("var.{name}")
                    } else {
                        format!("{kind}.{name}")
                    };
                    let node_kind = if kind == "variable" || kind == "output" {
                        "Function"
                    } else {
                        "Class"
                    };
                    push_terraform_node(
                        file_path,
                        &mut nodes,
                        &mut edges,
                        TerraformNodeSpec {
                            kind: node_kind,
                            name: &label,
                            line_start: 1,
                            line_end,
                            // Production `check` blocks run during plan/apply (#136).
                            is_test: false,
                            terraform_kind: kind,
                        },
                    );
                }
            }
            "module" => {
                for (name, body) in entries {
                    let node_name = format!("module.{name}");
                    push_terraform_node(
                        file_path,
                        &mut nodes,
                        &mut edges,
                        TerraformNodeSpec {
                            kind: "Class",
                            name: &node_name,
                            line_start: 1,
                            line_end,
                            is_test: false,
                            terraform_kind: "module",
                        },
                    );
                    if let Some(source) = body
                        .as_object()
                        .and_then(|attrs| attrs.get("source"))
                        .and_then(Value::as_str)
                    {
                        edges.push(ParsedEdge {
                            kind: crate::core::types::EdgeKind::ImportsFrom
                                .as_str()
                                .to_string(),
                            source: terraform_qualified(file_path, &node_name),
                            target: source.to_string(),
                            file_path: file_path.to_string(),
                            line: 1,
                            extra: json!({}),
                        });
                    }
                }
            }
            "provider" => {
                for name in entries.keys() {
                    let node_name = format!("provider.{name}");
                    push_terraform_node(
                        file_path,
                        &mut nodes,
                        &mut edges,
                        TerraformNodeSpec {
                            kind: "Class",
                            name: &node_name,
                            line_start: 1,
                            line_end,
                            is_test: false,
                            terraform_kind: "provider",
                        },
                    );
                }
            }
            "locals" => {
                for name in entries.keys() {
                    let node_name = format!("local.{name}");
                    push_terraform_node(
                        file_path,
                        &mut nodes,
                        &mut edges,
                        TerraformNodeSpec {
                            kind: "Function",
                            name: &node_name,
                            line_start: 1,
                            line_end,
                            is_test: false,
                            terraform_kind: "local",
                        },
                    );
                }
            }
            "terraform" => push_terraform_node(
                file_path,
                &mut nodes,
                &mut edges,
                TerraformNodeSpec {
                    kind: "Class",
                    name: "terraform",
                    line_start: 1,
                    line_end,
                    is_test: false,
                    terraform_kind: "terraform",
                },
            ),
            _ => {}
        }
    }
    (nodes, edges)
}

fn parse_terraform_hcl(
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
        kind: crate::core::types::NodeKind::File.as_str().to_string(),
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
            "variable" | "output" | "publish_output" | "upstream_input" => ("Function", false),
            // `check` is a top-level production block (Terraform 1.5+ health
            // checks executed during plan/apply), not a .tftest.hcl construct.
            "check" => ("Class", false),
            "run" | "mock_provider" | "variables" | "override_resource" | "override_data"
            | "override_module" => ("Test", true),
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
                    kind: crate::core::types::EdgeKind::ImportsFrom
                        .as_str()
                        .to_string(),
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
                    kind: crate::core::types::EdgeKind::DependsOn.as_str().to_string(),
                    source: terraform_qualified(file_path, &node_name),
                    target: provider_source.clone(),
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({}),
                });
            }
        }
    }

    extract_terraform_documentation_directives(file_path, &text, &nodes, &mut edges);
    extract_terraform_code_bridges(file_path, &blocks, &mut edges);

    (nodes, dedupe_edges(edges))
}

fn extract_terraform_documentation_directives(
    file_path: &str,
    text: &str,
    nodes: &[ParsedNode],
    edges: &mut Vec<ParsedEdge>,
) {
    for directive in extract_line_comment_dagayn_directives(text, &["#", "//"]) {
        let source = nearest_documentation_source(file_path, nodes, directive.line);
        push_documentation_directive_edge(
            edges,
            source,
            file_path,
            "terraform",
            &directive,
            "comment_directive",
        );
    }
}

fn terraform_defined_name(block: &TerraformBlock) -> Option<String> {
    let kind = block.kind.as_str();
    match kind {
        "terraform" | "variables" | "required_providers" | "override_resource"
        | "override_data" | "override_module" => Some(kind.to_string()),
        "provider" => block.labels.first().map(|name| format!("provider.{name}")),
        "variable" => block.labels.first().map(|name| format!("var.{name}")),
        "module" => block.labels.first().map(|name| format!("module.{name}")),
        "output" => block.labels.first().map(|name| format!("output.{name}")),
        "check" => block.labels.first().map(|name| format!("check.{name}")),
        "data" | "resource" | "ephemeral" | "action" | "orchestrate" | "store" | "list" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("{kind}.{block_type}.{name}")),
        "run" | "mock_provider" | "component" | "identity_token" | "deployment"
        | "deployment_group" | "publish_output" | "upstream_input" => {
            block.labels.first().map(|name| format!("{kind}.{name}"))
        }
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
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
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
                    kind: crate::core::types::EdgeKind::ImportsFrom
                        .as_str()
                        .to_string(),
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
                    kind: crate::core::types::EdgeKind::References
                        .as_str()
                        .to_string(),
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
                    kind: crate::core::types::EdgeKind::References
                        .as_str()
                        .to_string(),
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
            kind: crate::core::types::EdgeKind::Calls.as_str().to_string(),
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
            kind: crate::core::types::EdgeKind::References
                .as_str()
                .to_string(),
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
