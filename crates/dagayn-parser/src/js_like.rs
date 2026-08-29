use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};

use super::js_modules::{
    collect_javascript_defined_names, collect_javascript_import_map, collect_javascript_type_names,
    decode_javascript_string_literal, javascript_child_text, javascript_function_name,
    javascript_import_targets, javascript_named_child, resolve_javascript_call_target,
    resolve_javascript_imported_symbol, resolve_javascript_module, JavaScriptCaches,
    JavaScriptParseContext,
};
use super::member_calls::MemberCallBindings;
use super::parsers::*;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    ends_with_ascii_ignore_case, is_test_file, line_count, node_text, starts_with_ascii_ignore_case,
};
use super::{add_tested_by_edges, qualify, resolve_rust_call_targets};

pub(super) fn parse_javascript_like(
    file_path: &str,
    source: &[u8],
    language: &'static str,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = match language {
        "javascript" => new_javascript_parser(),
        "typescript" => new_typescript_parser(),
        "tsx" => new_tsx_parser(),
        _ => None,
    };
    parse_javascript_like_with_parser(
        file_path,
        source,
        language,
        parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

pub(super) fn parse_javascript_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &'static str,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_javascript_like_interned(
        &FilePath::new(file_path),
        source,
        language,
        parser,
        repo_root,
        caches,
    )
}

pub(super) fn parse_javascript_like_interned(
    file_path: &FilePath,
    source: &[u8],
    language: &'static str,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let test_file = is_javascript_test_file(file_path);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: test_file,
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let mut defined_names = HashSet::new();
            collect_javascript_defined_names(root, source, &mut defined_names);
            let mut type_names = HashSet::new();
            collect_javascript_type_names(root, source, &mut type_names);
            let mut import_map = HashMap::new();
            collect_javascript_import_map(root, source, &mut import_map);
            let context = JavaScriptParseContext {
                source,
                file_path: file_path.clone(),
                language,
                test_file,
                defined_names: &defined_names,
                import_map: &import_map,
                repo_root,
                caches,
                bindings: RefCell::new(MemberCallBindings::with_types(type_names)),
            };
            javascript_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            let mut edges = resolve_rust_call_targets(&nodes, edges, file_path);
            if test_file {
                add_tested_by_edges(&nodes, &mut edges);
            }
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn javascript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration"
            | "class"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration" => {
                if let Some(name) = javascript_named_child(
                    child,
                    context.source,
                    &["identifier", "type_identifier"],
                ) {
                    let qualified = qualify(&context.file_path, &name, enclosing_class);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Class,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: context.language.to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: javascript_class_extra(child, context.source),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: context.file_path.to_string(),
                        target: qualified.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    emit_javascript_inheritance_edges(child, context, &qualified, edges);
                    javascript_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_declaration"
            | "method_definition"
            | "method_signature"
            | "function_signature"
            | "arrow_function" => {
                if javascript_emit_function_node(child, context, enclosing_class, nodes, edges) {
                    if let Some(name) = javascript_function_name(child, context.source) {
                        let snapshot = context.bindings.borrow().snapshot();
                        if let Some(class_name) = enclosing_class {
                            context
                                .bindings
                                .borrow_mut()
                                .bind_implicit_receivers(class_name);
                        }
                        javascript_walk_children(
                            child,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                        context.bindings.borrow_mut().restore(snapshot);
                    }
                    continue;
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                if javascript_emit_variable_functions(child, context, enclosing_class, nodes, edges)
                {
                    continue;
                }
            }
            "public_field_definition" => {
                if javascript_emit_field_function(child, context, enclosing_class, nodes, edges) {
                    continue;
                }
            }
            "import_statement" | "export_statement" => {
                for target in javascript_import_targets(child, context.source) {
                    let resolved = resolve_javascript_module(
                        &target,
                        &context.file_path,
                        context.repo_root,
                        context.caches,
                    )
                    .unwrap_or(target);
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target: resolved,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
                if child.kind() == "import_statement" {
                    continue;
                }
            }
            "call_expression" | "new_expression" => {
                if javascript_emit_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                javascript_emit_jsx_component_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "pair"
            | "assignment_expression"
            | "array"
            | "arguments"
            | "shorthand_property_identifier" => {
                javascript_emit_value_references(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        javascript_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        javascript_bind_declarator(child, context);
        javascript_bind_assignment(child, context);
    }
}

fn javascript_emit_function_node(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = javascript_function_name(node, context.source) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, &context.file_path);
    let qualified = qualify(&context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: if node.kind() == "arrow_function" {
            None
        } else {
            javascript_child_text(node, context.source, "formal_parameters")
        },
        return_type: javascript_child_text(node, context.source, "type_annotation"),
        modifiers: None,
        is_test,
        extra: if matches!(node.kind(), "method_signature" | "function_signature") {
            json!({"is_abstract": true})
        } else {
            json!({})
        },
    });
    let container = enclosing_class
        .map(|name| qualify(&context.file_path, name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

fn javascript_emit_variable_functions(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut handled = false;
    let mut cursor = node.walk();
    for declarator in node.children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let mut name = None;
        let mut function_node = None;
        let mut declarator_cursor = declarator.walk();
        for child in declarator.children(&mut declarator_cursor) {
            if child.kind() == "identifier" && name.is_none() {
                name = Some(node_text(child, context.source));
            } else if is_javascript_function_value(child.kind()) {
                function_node = Some(child);
            }
        }
        let (Some(name), Some(function_node)) = (name, function_node) else {
            continue;
        };
        let is_test = is_javascript_test_function(&name, &context.file_path);
        let qualified = qualify(&context.file_path, &name, enclosing_class);
        nodes.push(ParsedNode {
            kind: if is_test {
                crate::core::types::NodeKind::Test
            } else {
                crate::core::types::NodeKind::Function
            },
            name: name.clone(),
            file_path: context.file_path.clone(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: enclosing_class.map(str::to_string),
            params: javascript_child_text(function_node, context.source, "formal_parameters"),
            return_type: javascript_child_text(function_node, context.source, "type_annotation"),
            modifiers: None,
            is_test,
            extra: json!({}),
        });
        let container = enclosing_class
            .map(|class_name| qualify(&context.file_path, class_name, None))
            .unwrap_or_else(|| context.file_path.to_string());
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: container,
            target: qualified,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
        let snapshot = context.bindings.borrow().snapshot();
        if let Some(class_name) = enclosing_class {
            context
                .bindings
                .borrow_mut()
                .bind_implicit_receivers(class_name);
        }
        javascript_walk_children(
            function_node,
            context,
            enclosing_class,
            Some(&name),
            nodes,
            edges,
        );
        context.bindings.borrow_mut().restore(snapshot);
        handled = true;
    }
    handled
}

fn javascript_emit_field_function(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut name = None;
    let mut function_node = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "property_identifier" && name.is_none() {
            name = Some(node_text(child, context.source));
        } else if is_javascript_function_value(child.kind()) {
            function_node = Some(child);
        }
    }
    let (Some(name), Some(function_node)) = (name, function_node) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, &context.file_path);
    let qualified = qualify(&context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: javascript_child_text(function_node, context.source, "formal_parameters"),
        return_type: javascript_child_text(function_node, context.source, "type_annotation"),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    let container = enclosing_class
        .map(|class_name| qualify(&context.file_path, class_name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    let snapshot = context.bindings.borrow().snapshot();
    if let Some(class_name) = enclosing_class {
        context
            .bindings
            .borrow_mut()
            .bind_implicit_receivers(class_name);
    }
    javascript_walk_children(
        function_node,
        context,
        enclosing_class,
        Some(&name),
        nodes,
        edges,
    );
    context.bindings.borrow_mut().restore(snapshot);
    true
}

fn javascript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(call_name) = javascript_call_name(node, context.source) else {
        return false;
    };
    let effective_call_name = if context.test_file && !is_test_runner_name(&call_name) {
        javascript_base_test_runner_name(node, context.source).unwrap_or_else(|| call_name.clone())
    } else {
        call_name.clone()
    };
    if context.test_file && is_test_runner_name(&effective_call_name) {
        let line = node.start_position().row as i64 + 1;
        let synthetic_name = match javascript_first_string_arg(node, context.source) {
            Some(description) if !description.is_empty() => {
                format!("{effective_call_name}:{description}@L{line}")
            }
            _ => format!("{effective_call_name}@L{line}"),
        };
        let qualified = qualify(&context.file_path, &synthetic_name, enclosing_class);
        nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Test,
            name: synthetic_name.clone(),
            file_path: context.file_path.clone(),
            line_start: line,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: enclosing_class.map(str::to_string),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: true,
            extra: json!({}),
        });
        let container = enclosing_func
            .map(|func| qualify(&context.file_path, func, enclosing_class))
            .unwrap_or_else(|| context.file_path.to_string());
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: container,
            target: qualified,
            file_path: context.file_path.clone(),
            line,
            extra: json!({}),
        });
        javascript_walk_children(
            node,
            context,
            enclosing_class,
            Some(&synthetic_name),
            nodes,
            edges,
        );
        return true;
    }

    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    let target = javascript_bound_member_target(node, context)
        .unwrap_or_else(|| resolve_javascript_call_target(&call_name, context));
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = javascript_bridge_edge(node, context, &caller) {
        edges.push(edge);
    }
    false
}

fn javascript_emit_value_references(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    match node.kind() {
        "pair" => {
            if let Some(value) = javascript_pair_value_identifier(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "shorthand_property_identifier" => {
            let value = node_text(node, context.source);
            javascript_emit_reference_if_known(node, context, &caller, &value, edges);
        }
        "assignment_expression" => {
            if let Some(value) = javascript_last_identifier_child(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "array" | "arguments" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    let value = node_text(child, context.source);
                    javascript_emit_reference_if_known(child, context, &caller, &value, edges);
                }
            }
        }
        _ => {}
    }
}

fn javascript_emit_jsx_component_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = javascript_jsx_component_target(node, context) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn javascript_jsx_component_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let (base_name, component_name) = javascript_jsx_component_reference(node, context.source)?;
    if let Some(base_name) = base_name {
        if let Some(module) = context.import_map.get(&base_name) {
            return resolve_javascript_imported_symbol(&component_name, module, context)
                .or(Some(component_name));
        }
        return Some(component_name);
    }
    Some(resolve_javascript_call_target(&component_name, context))
}

fn javascript_jsx_component_reference(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Option<String>, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(child, source);
                return looks_like_jsx_component_name(&name).then_some((None, name));
            }
            "member_expression" => {
                let component_name = javascript_rightmost_identifier(child, source)?;
                if !looks_like_jsx_component_name(&component_name) {
                    return None;
                }
                let base_name = javascript_leftmost_identifier(child, source);
                return Some((base_name, component_name));
            }
            _ => {}
        }
    }
    None
}

fn looks_like_jsx_component_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_uppercase())
}

fn javascript_emit_reference_if_known(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if javascript_should_skip_value_reference(name)
        || (!context.defined_names.contains(name) && !context.import_map.contains_key(name))
    {
        return;
    }
    let target = resolve_javascript_call_target(name, context);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::References,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn javascript_class_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> Value {
    let type_role = match node.kind() {
        "interface_declaration" => "interface",
        "type_alias_declaration" => "type_alias",
        "enum_declaration" => "enum",
        _ => "class",
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "interface" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
        if javascript_is_type_only_container(type_role)
            || javascript_is_data_model_class(node, source)
        {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    extra
}

fn javascript_is_type_only_container(type_role: &str) -> bool {
    matches!(type_role, "type_alias" | "enum")
}

fn javascript_is_data_model_class(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    if !matches!(node.kind(), "class_declaration" | "class") {
        return false;
    }
    if javascript_has_data_model_decorator(node, source) {
        return true;
    }
    let Some(name) = javascript_named_child(node, source, &["identifier", "type_identifier"])
    else {
        return false;
    };
    if javascript_is_data_model_name(&name) {
        return true;
    }
    javascript_is_property_only_class(node)
}

fn javascript_has_data_model_decorator(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let text = node_text(node, source);
    [
        "@Entity",
        "@ObjectType",
        "@InputType",
        "@ArgsType",
        "@Schema",
        "@model",
        "@Table",
    ]
    .iter()
    .any(|decorator| text.contains(decorator))
}

fn javascript_is_data_model_name(name: &str) -> bool {
    [
        "Dto", "DTO", "Data", "Payload", "Props", "State", "Model", "Entity", "Record", "Schema",
        "Input", "Output",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn javascript_is_property_only_class(node: tree_sitter::Node<'_>) -> bool {
    let mut has_data_field = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "method_definition" => return false,
            "public_field_definition" | "field_definition" | "property_signature" => {
                has_data_field = true;
            }
            _ => {
                if !javascript_class_child_is_property_only(child, &mut has_data_field) {
                    return false;
                }
            }
        }
    }
    has_data_field
}

fn javascript_class_child_is_property_only(
    node: tree_sitter::Node<'_>,
    has_data_field: &mut bool,
) -> bool {
    match node.kind() {
        "method_definition" => return false,
        "public_field_definition" | "field_definition" | "property_signature" => {
            *has_data_field = true;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !javascript_class_child_is_property_only(child, has_data_field) {
            return false;
        }
    }
    true
}

fn emit_javascript_inheritance_edges(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    qualified: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut bases = Vec::new();
    collect_javascript_bases(node, context.source, &mut bases);
    for (base, role) in bases {
        edges.push(ParsedEdge {
            kind: if role == "implements" {
                crate::core::types::EdgeKind::Implements
            } else {
                crate::core::types::EdgeKind::Inherits
            },
            source: qualified.to_string(),
            target: base,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({"relationship_role": role, "syntax_source": node.kind()}),
        });
    }
}

fn collect_javascript_bases(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    bases: &mut Vec<(String, &'static str)>,
) {
    let role = match node.kind() {
        "extends_clause" => Some("extends"),
        "implements_clause" => Some("implements"),
        _ => None,
    };
    if let Some(role) = role {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if matches!(
                child.kind(),
                "identifier" | "type_identifier" | "nested_identifier"
            ) {
                bases.push((node_text(child, source), role));
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_bases(child, source, bases);
    }
}

fn javascript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    match callee.kind() {
        "identifier" | "property_identifier" | "type_identifier" => Some(node_text(callee, source)),
        "member_expression" => javascript_rightmost_identifier(callee, source),
        _ => None,
    }
}

fn javascript_bound_member_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    if callee.kind() != "member_expression" {
        return None;
    }
    let method = javascript_rightmost_identifier(callee, context.source)?;
    let receiver = javascript_leftmost_identifier(callee, context.source)?;
    context.bindings.borrow().resolve_member(&receiver, &method)
}

fn javascript_bind_declarator(node: tree_sitter::Node<'_>, context: &JavaScriptParseContext<'_>) {
    if node.kind() != "variable_declarator" {
        return;
    }
    let mut ident = None;
    let mut annotated = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "type_annotation" => {
                annotated = javascript_named_child(
                    child,
                    context.source,
                    &["identifier", "type_identifier"],
                );
            }
            "new_expression" | "call_expression" => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let Some(ident) = ident else {
        return;
    };
    if let Some(value) = value {
        if let Some(type_name) = javascript_inferred_constructor(value, context) {
            context.bindings.borrow_mut().bind(ident, type_name);
            return;
        }
    }
    if let Some(type_name) = annotated {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn javascript_bind_assignment(node: tree_sitter::Node<'_>, context: &JavaScriptParseContext<'_>) {
    if node.kind() != "assignment_expression" {
        return;
    }
    let mut ident = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "new_expression" | "call_expression" => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let (Some(ident), Some(value)) = (ident, value) else {
        return;
    };
    if let Some(type_name) = javascript_inferred_constructor(value, context) {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn javascript_inferred_constructor(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let call_name = javascript_call_name(node, context.source)?;
    context
        .bindings
        .borrow()
        .constructor_type(&call_name)
        .map(str::to_string)
}

fn javascript_callee_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if node.kind() == "new_expression" {
        if let Some(constructor) = node.child_by_field_name("constructor") {
            return Some(constructor);
        }
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        return children
            .into_iter()
            .find(|child| !matches!(child.kind(), "new" | "arguments" | "type_arguments"));
    }
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .find(|child| child.kind() != "arguments")
}

fn javascript_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_leftmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_leftmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    javascript_callee_node(node)
        .map(|callee| node_text(callee, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn javascript_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
) -> Option<ParsedEdge> {
    let signature = javascript_call_signature(node, context.source)?;
    let (relationship_role, bridge_kind) = javascript_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) =
        match javascript_first_string_arg(node, context.source) {
            Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
            _ => (
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
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn javascript_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "child_process.exec"
        | "child_process.execFile"
        | "child_process.execSync"
        | "child_process.execFileSync"
        | "child_process.spawn"
        | "child_process.spawnSync"
        | "child_process.fork" => Some(("invokes_binary", "subprocess")),
        "fs.readFile" | "fs.readFileSync" | "fs.promises.readFile" => {
            Some(("reads_file", "file_io"))
        }
        "fs.writeFile" | "fs.writeFileSync" | "fs.promises.writeFile" => {
            Some(("writes_file", "file_io"))
        }
        _ => None,
    }
}

fn javascript_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string" | "template_string") {
            return Some(decode_javascript_string_literal(child, source));
        }
        return None;
    }
    None
}

fn javascript_pair_value_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut seen_colon = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == ":" {
            seen_colon = true;
            continue;
        }
        if seen_colon && child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn javascript_last_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .rev()
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(child, source))
}

fn javascript_base_test_runner_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    if callee.kind() != "member_expression" {
        return None;
    }
    let rightmost = javascript_rightmost_identifier(callee, source)?;
    if !matches!(
        rightmost.as_str(),
        "only" | "skip" | "each" | "todo" | "concurrent"
    ) {
        return None;
    }
    let mut cursor = callee.walk();
    for child in callee.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
        if child.kind() == "member_expression" {
            let mut inner = child.walk();
            for sub in child.children(&mut inner) {
                if sub.kind() == "identifier" {
                    return Some(node_text(sub, source));
                }
            }
        }
    }
    None
}

fn is_test_runner_name(name: &str) -> bool {
    matches!(name, "describe" | "it" | "test")
}

fn is_javascript_function_value(kind: &str) -> bool {
    matches!(kind, "arrow_function" | "function_expression" | "function")
}

fn is_javascript_test_function(name: &str, file_path: &FilePath) -> bool {
    starts_with_ascii_ignore_case(name, "test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.ends_with("_spec")
        || (is_javascript_test_file(file_path) && is_test_runner_name(name))
}

fn is_javascript_test_file(file_path: &FilePath) -> bool {
    is_test_file(file_path)
        || ends_with_ascii_ignore_case(file_path, ".test.ts")
        || ends_with_ascii_ignore_case(file_path, ".spec.ts")
        || ends_with_ascii_ignore_case(file_path, ".test.js")
        || ends_with_ascii_ignore_case(file_path, ".spec.js")
}

fn javascript_should_skip_value_reference(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "null"
            | "undefined"
            | "None"
            | "True"
            | "False"
            | "self"
            | "this"
            | "cls"
            | "super"
    ) || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
}
