use std::collections::HashMap;

use serde_json::{json, Value};

use super::types::{FilePath, ParsedEdge, ParsedNode};
use std::path::Path;

use super::util::{
    is_test_file, line_count, node_text, resolve_import_path, set_namespaces_from_type_names,
    strip_matching_quotes,
};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_julia_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: "julia".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = JuliaParseContext {
        source,
        file_path: file_path.clone(),
        repo_root,
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            julia_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            set_namespaces_from_type_names(&mut nodes);
            let mut edges = resolve_julia_targets(&nodes, edges, &file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct JuliaParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    repo_root: Option<&'a Path>,
}

fn julia_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "module_definition" => {
                if let Some(name) = julia_direct_child_text(child, context.source, &["identifier"])
                {
                    julia_emit_class(
                        child,
                        context,
                        JuliaClassSpec {
                            name: &name,
                            parent_name: None,
                            extra: json!({"type_role": "class"}),
                            contains_from_parent: true,
                        },
                        nodes,
                        edges,
                    );
                    if let Some(block) = julia_direct_child(child, &["block"]) {
                        julia_walk_children(block, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "using_statement" | "import_statement" => {
                for target in julia_import_targets(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
                continue;
            }
            "export_statement" | "public_statement" => {
                julia_emit_symbol_references(child, context, enclosing_class, edges);
                continue;
            }
            "macrocall_expression"
                if julia_handle_macrocall(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) =>
            {
                continue;
            }
            "abstract_definition" | "struct_definition" => {
                if let Some(name) = julia_type_name(child, context.source) {
                    let extra = if child.kind() == "abstract_definition" {
                        json!({"type_role": "abstract_type", "is_abstract": true})
                    } else {
                        json!({"type_role": "struct"})
                    };
                    julia_emit_class(
                        child,
                        context,
                        JuliaClassSpec {
                            name: &name,
                            parent_name: enclosing_class,
                            extra,
                            contains_from_parent: false,
                        },
                        nodes,
                        edges,
                    );
                    if child.kind() == "struct_definition" {
                        julia_emit_inheritance(child, context, &name, enclosing_class, edges);
                    }
                    continue;
                }
            }
            "function_definition" | "macro_definition" => {
                if let Some(name) = julia_function_name(child, context.source) {
                    let parent = julia_function_parent(enclosing_class, enclosing_func);
                    julia_emit_function(child, context, &name, parent.as_deref(), nodes, edges);
                    julia_emit_owner_reference(child, context, &name, parent.as_deref(), edges);
                    if let Some(block) = julia_direct_child(child, &["block"]) {
                        julia_walk_children(
                            block,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "assignment"
                if julia_handle_short_function(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) =>
            {
                continue;
            }
            "call_expression" => {
                if julia_is_signature_call(child) || julia_is_assignment_lhs_call(child) {
                    continue;
                }
                if let Some(call_name) = julia_call_name(child, context.source) {
                    if call_name == "include" {
                        if let Some(target) = julia_first_string_arg(child, context.source) {
                            // `include` is relative to the including file.
                            let target = resolve_import_path(
                                &target,
                                &context.file_path,
                                context.repo_root,
                                &[],
                                false,
                            )
                            .unwrap_or(target);
                            edges.push(ParsedEdge {
                                kind: crate::core::types::EdgeKind::ImportsFrom,
                                source: context.file_path.to_string(),
                                target,
                                file_path: context.file_path.clone(),
                                line: child.start_position().row as i64 + 1,
                                extra: json!({}),
                            });
                        }
                    }
                    julia_emit_call(
                        child,
                        context,
                        &call_name,
                        enclosing_class,
                        enclosing_func,
                        edges,
                    );
                }
            }
            _ => {}
        }
        julia_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn julia_handle_short_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(lhs) = julia_assignment_lhs_call(node) else {
        return false;
    };
    let Some(name) = julia_call_name(lhs, context.source) else {
        return false;
    };
    let parent = julia_function_parent(enclosing_class, enclosing_func);
    julia_emit_function(node, context, &name, parent.as_deref(), nodes, edges);
    julia_emit_owner_reference(node, context, &name, parent.as_deref(), edges);
    let mut seen_operator = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !seen_operator {
            if child.kind() == "operator" {
                seen_operator = true;
            }
            continue;
        }
        julia_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
    }
    true
}

fn julia_handle_macrocall(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(macro_name) = julia_macro_name(node, context.source) else {
        return false;
    };
    match macro_name.as_str() {
        "enum" => {
            julia_emit_enum(node, context, enclosing_class, nodes, edges);
            true
        }
        "testset" => {
            julia_emit_testset(node, context, enclosing_class, enclosing_func, nodes, edges);
            true
        }
        _ => {
            julia_emit_call(
                node,
                context,
                &format!("@{macro_name}"),
                enclosing_class,
                enclosing_func,
                edges,
            );
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "macro_argument_list" {
                    julia_walk_children(
                        child,
                        context,
                        enclosing_class,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            true
        }
    }
}

struct JuliaClassSpec<'a> {
    name: &'a str,
    parent_name: Option<&'a str>,
    extra: Value,
    contains_from_parent: bool,
}

fn julia_emit_class(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    spec: JuliaClassSpec<'_>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, spec.name, spec.parent_name);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: spec.name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: spec.parent_name.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: spec.extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: if spec.contains_from_parent {
            spec.parent_name
                .map(|parent| qualify(&context.file_path, parent, None))
                .unwrap_or_else(|| context.file_path.to_string())
        } else {
            context.file_path.to_string()
        },
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn julia_emit_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    parent_name: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, parent_name);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: parent_name.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: parent_name
            .map(|parent| qualify(&context.file_path, parent, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn julia_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    call_name: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = julia_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn julia_emit_enum(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(args) = julia_direct_child(node, &["macro_argument_list"]) else {
        return;
    };
    let identifiers = julia_direct_child_texts(args, context.source, &["identifier"]);
    let Some(type_name) = identifiers.first() else {
        return;
    };
    let qualified_type = qualify(&context.file_path, type_name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: type_name.clone(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"julia_kind": "enum"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified_type.clone(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for variant in identifiers.iter().skip(1) {
        nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Function,
            name: variant.clone(),
            file_path: context.file_path.clone(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: "julia".to_string(),
            parent_name: Some(type_name.clone()),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"julia_kind": "enum_variant"}),
        });
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: qualified_type.clone(),
            target: qualify(&context.file_path, variant, Some(type_name)),
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn julia_emit_testset(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let desc = julia_direct_child(node, &["macro_argument_list"])
        .and_then(|args| julia_first_descendant_text(args, context.source, &["content"]));
    let line = node.start_position().row as i64 + 1;
    let name = desc
        .map(|desc| format!("testset:{desc}@L{line}"))
        .unwrap_or_else(|| format!("testset@L{line}"));
    let qualified = qualify(&context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Test,
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: line,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_func
            .map(|func| qualify(&context.file_path, func, enclosing_class))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line,
        extra: json!({}),
    });
    if let Some(args) = julia_direct_child(node, &["macro_argument_list"]) {
        julia_walk_children(args, context, enclosing_class, Some(&name), nodes, edges);
    }
}

fn julia_emit_symbol_references(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let marker = if node.kind() == "export_statement" {
        "julia_export"
    } else {
        "julia_public"
    };
    let source = enclosing_class
        .map(|class| qualify(&context.file_path, class, None))
        .unwrap_or_else(|| context.file_path.to_string());
    for target in julia_direct_child_texts(node, context.source, &["identifier"]) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::References,
            source: source.clone(),
            target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({marker: true}),
        });
    }
}

fn julia_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(type_head) = julia_direct_child(node, &["type_head"]) else {
        return;
    };
    let Some(binary) = julia_direct_child(type_head, &["binary_expression"]) else {
        return;
    };
    let identifiers = julia_direct_child_texts(binary, context.source, &["identifier"]);
    if identifiers.len() < 2 {
        return;
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Inherits,
        source: qualify(&context.file_path, name, enclosing_class),
        target: identifiers[1].clone(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({"relationship_role": "extends", "syntax_source": "struct_definition"}),
    });
}

fn julia_emit_owner_reference(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    parent_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(owner) = julia_qualified_function_owner(node, context.source) else {
        return;
    };
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::References,
        source: qualify(&context.file_path, name, parent_name),
        target: owner,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn julia_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = julia_call_signature(node, context.source).unwrap_or_else(|| call_name.into());
    let (relationship_role, bridge_kind) = match signature.as_str() {
        "run" | "readchomp" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "read" | "readlines" => ("reads_file", "file_io"),
        "write" => ("writes_file", "file_io"),
        "Libdl.dlopen" | "dlopen" | "ccall" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match julia_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "julia",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn julia_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            targets.push(node_text(child, source));
        } else if child.kind() == "selected_import" {
            let names = julia_direct_child_texts(child, source, &["identifier"]);
            if let Some(module) = names.first() {
                targets.extend(names.iter().skip(1).map(|name| format!("{module}.{name}")));
            }
        }
    }
    targets
}

fn julia_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let type_head = julia_direct_child(node, &["type_head"])?;
    julia_first_descendant_text(type_head, source, &["identifier"])
}

fn julia_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let signature = julia_direct_child(node, &["signature"])?;
    let call = julia_first_descendant(signature, &["call_expression"])?;
    julia_call_name(call, source)
}

fn julia_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "field_expression" => julia_last_descendant_text(first, source, &["identifier"]),
        _ => None,
    }
}

fn julia_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "field_expression" => Some(node_text(first, source).replace(' ', "")),
        _ => None,
    }
}

fn julia_macro_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let macro_identifier = julia_direct_child(node, &["macro_identifier"])?;
    julia_direct_child_text(macro_identifier, source, &["identifier"])
}

fn julia_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let args = julia_direct_child(node, &["argument_list"])?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return julia_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn julia_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    julia_first_descendant_text(node, source, &["content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn julia_qualified_function_owner(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let signature = if node.kind() == "assignment" {
        julia_assignment_lhs_call(node)
    } else {
        julia_direct_child(node, &["signature"])
            .and_then(|signature| julia_first_descendant(signature, &["call_expression"]))
    }?;
    let field = julia_first_descendant(signature, &["field_expression"])?;
    let names = julia_direct_child_texts(field, source, &["identifier"]);
    names.first().cloned()
}

fn julia_assignment_lhs_call<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let lhs = julia_first_named_child(node)?;
    if lhs.kind() == "call_expression" {
        Some(lhs)
    } else if lhs.kind() == "typed_expression" {
        julia_first_descendant(lhs, &["call_expression"])
    } else {
        None
    }
}

fn julia_is_signature_call(node: tree_sitter::Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "signature")
}

fn julia_is_assignment_lhs_call(node: tree_sitter::Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "assignment" {
        return julia_first_named_child(parent) == Some(node);
    }
    if parent.kind() == "typed_expression" {
        return parent
            .parent()
            .is_some_and(|grandparent| grandparent.kind() == "assignment");
    }
    false
}

fn julia_function_parent(
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
) -> Option<String> {
    match (enclosing_class, enclosing_func) {
        (Some(class), Some(func)) => Some(format!("{class}.{func}")),
        (Some(class), None) => Some(class.to_string()),
        (None, Some(func)) => Some(func.to_string()),
        (None, None) => None,
    }
}

fn julia_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn julia_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    julia_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn julia_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}

fn julia_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|child| child.is_named());
    found
}

fn julia_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = julia_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn julia_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    julia_first_descendant(node, kinds).map(|child| node_text(child, source))
}

fn julia_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    julia_collect_descendant_texts(node, source, kinds, &mut found);
    found
}

fn julia_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    found: &mut Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            *found = Some(node_text(child, source));
        }
        julia_collect_descendant_texts(child, source, kinds, found);
    }
}

fn resolve_julia_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Class" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if matches!(edge.kind.as_str(), "CALLS" | "REFERENCES") && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}
