use serde_json::json;

use super::jni::jni_symbol;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, is_test_file, line_count, node_text, set_declared_namespaces,
    strip_matching_quotes, type_name_without_arguments,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_kotlin_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: "kotlin".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        kotlin_walk_children(
            tree.root_node(),
            source,
            &file_path,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        kotlin_name_jni_symbols(tree.root_node(), source, &file_path, &mut nodes);
        set_declared_namespaces(
            &mut nodes,
            collect_namespace_paths(
                tree.root_node(),
                source,
                &["package_header"],
                None,
                &["identifier"],
            ),
        );
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

fn kotlin_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    // Declarations inside a function body are local to it: `a`'s
    // `helper` is `a.helper`, apart from `b.helper`.
    let local_owner = std::cell::OnceCell::new();
    // Built on first use: this runs for every node of a function body.
    let owner = || {
        local_owner
            .get_or_init(|| {
                enclosing_func.map(|func| match enclosing_class {
                    Some(class) => format!("{class}.{func}"),
                    None => func.to_string(),
                })
            })
            .as_deref()
            .or(enclosing_class)
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_header" => {
                kotlin_emit_import(child, source, file_path, edges);
            }
            "class_declaration" | "object_declaration" => {
                if let Some(name) = kotlin_direct_child_text(child, source, &["type_identifier"]) {
                    kotlin_emit_type(child, source, file_path, &name, owner(), nodes, edges);
                    let path = match owner() {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name.clone(),
                    };
                    kotlin_walk_children(child, source, file_path, Some(&path), None, nodes, edges);
                    continue;
                }
            }
            "secondary_constructor" if enclosing_class.is_some() => {
                kotlin_emit_function(
                    child,
                    source,
                    file_path,
                    "constructor",
                    enclosing_class,
                    nodes,
                    edges,
                );
                kotlin_walk_children(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    Some("constructor"),
                    nodes,
                    edges,
                );
                continue;
            }
            "function_declaration" => {
                if let Some(name) = kotlin_direct_child_text(child, source, &["simple_identifier"])
                {
                    kotlin_emit_function(child, source, file_path, &name, owner(), nodes, edges);
                    kotlin_walk_children(
                        child,
                        source,
                        file_path,
                        owner(),
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "call_expression" => {
                kotlin_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        kotlin_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn kotlin_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = kotlin_import_target(node, source) else {
        return;
    };
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: file_path.to_string(),
        target,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

/// The imported path, without the `import` keyword or an `as` alias.
///
/// The whole statement used to be the target, so `import java.util.UUID`
/// could never match a package index entry.
fn kotlin_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let target = kotlin_direct_child_text(node, source, &["identifier"])?;
    let target = target.trim();
    (!target.is_empty()).then(|| target.to_string())
}

fn kotlin_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    let extra = kotlin_type_extra(node, source);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "kotlin".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|parent| qualify(file_path, parent, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified.clone(),
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    let mut cursor = node.walk();
    for specifier in node.children(&mut cursor) {
        if specifier.kind() != "delegation_specifier" {
            continue;
        }
        // `Base(a)` invokes a superclass constructor; a bare type names an
        // interface (or a delegated one via `by`).
        let invocation = kotlin_direct_child(specifier, &["constructor_invocation"]);
        let is_class = invocation.is_some();
        let Some(target) = kotlin_direct_child(invocation.unwrap_or(specifier), &["user_type"])
            .and_then(|user_type| type_name_without_arguments(user_type, source))
        else {
            continue;
        };
        let (kind, role) = if is_class {
            (crate::core::types::EdgeKind::Inherits, "extends")
        } else {
            (crate::core::types::EdgeKind::Implements, "implements")
        };
        edges.push(ParsedEdge {
            kind,
            source: qualified.clone(),
            target,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": role,
                "syntax_source": node.kind(),
            }),
        });
    }
}

fn kotlin_class_modifiers(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(modifiers) = kotlin_direct_child(node, &["modifiers"]) else {
        return Vec::new();
    };
    let mut cursor = modifiers.walk();
    modifiers
        .children(&mut cursor)
        .filter(|child| child.kind() == "class_modifier")
        .map(|child| node_text(child, source).trim().to_string())
        .collect()
}

fn kotlin_type_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> serde_json::Value {
    let modifiers = kotlin_class_modifiers(node, source);
    let has_keyword = |keyword: &str| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .any(|child| !child.is_named() && child.kind() == keyword)
    };
    let type_role = if node.kind() == "object_declaration" {
        "object"
    } else if modifiers.iter().any(|modifier| modifier == "data") {
        "record"
    } else if modifiers.iter().any(|modifier| modifier == "enum") {
        "enum"
    } else if has_keyword("interface") {
        "interface"
    } else {
        "class"
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "record" {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
        if type_role == "interface" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    extra
}

fn kotlin_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    // `external fun` is implemented through JNI; the JVM class that owns it,
    // and so its symbol, is named once the whole file is walked.
    let extra = match kotlin_jni_owner(node, source, enclosing_class) {
        Some(owner) => json!({"ffi_import": {"abi": "jni", "owner": owner}}),
        None => json!({}),
    };
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "kotlin".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

/// The class path an `external fun` belongs to on the JVM: the enclosing
/// class (`Outer.Inner`), its `Companion` unless the function is
/// `@JvmStatic`, or `""` for a top-level function (the file facade).
fn kotlin_jni_owner(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    enclosing_class: Option<&str>,
) -> Option<String> {
    if node.kind() != "function_declaration" {
        return None;
    }
    let modifiers = kotlin_direct_child(node, &["modifiers"])?;
    let mut external = false;
    let mut jvm_static = false;
    let mut cursor = modifiers.walk();
    for modifier in modifiers.children(&mut cursor) {
        let text = node_text(modifier, source);
        match modifier.kind() {
            "function_modifier" => external |= text.trim() == "external",
            "annotation" => {
                jvm_static |= text
                    .trim_start_matches('@')
                    .split(['(', ' '])
                    .next()
                    .is_some_and(|name| name.rsplit('.').next() == Some("JvmStatic"));
            }
            _ => {}
        }
    }
    if !external {
        return None;
    }
    let mut owner = enclosing_class.unwrap_or_default().to_string();
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        match current.kind() {
            "companion_object" => {
                if !jvm_static {
                    let name = kotlin_direct_child_text(current, source, &["type_identifier"]);
                    owner.push('.');
                    owner.push_str(name.as_deref().unwrap_or("Companion"));
                }
                break;
            }
            "class_declaration" | "object_declaration" => break,
            _ => ancestor = current.parent(),
        }
    }
    Some(owner)
}

/// Replaces each `ffi_import.owner` recorded by [`kotlin_jni_owner`] with the
/// JNI `symbol`: top-level functions live in the file facade class
/// (`Sum.kt` -> `SumKt`, or `@file:JvmName("Name")`).
fn kotlin_name_jni_symbols(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    nodes: &mut [ParsedNode],
) {
    let package = kotlin_direct_child(root, &["package_header"])
        .and_then(|header| kotlin_direct_child_text(header, source, &["identifier"]));
    let facade = kotlin_file_facade(root, source, file_path);
    for node in nodes.iter_mut() {
        let Some(import) = node.extra.get_mut("ffi_import") else {
            continue;
        };
        let Some(owner) = import
            .get("owner")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let class = if owner.is_empty() {
            facade.clone()
        } else {
            owner
        };
        import["symbol"] = json!(jni_symbol(package.as_deref(), &class, &node.name));
        if let Some(map) = import.as_object_mut() {
            map.remove("owner");
        }
    }
}

fn kotlin_file_facade(root: tree_sitter::Node<'_>, source: &[u8], file_path: &FilePath) -> String {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "file_annotation" {
            continue;
        }
        let text = node_text(child, source);
        if let Some(rest) = text.split_once("JvmName(").map(|(_, rest)| rest)
            && let Some(name) = rest.split('"').nth(1)
        {
            return name.to_string();
        }
    }
    let stem = file_path
        .to_string()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches(".kts")
        .trim_end_matches(".kt")
        .to_string();
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => format!("{}{}Kt", first.to_uppercase(), chars.as_str()),
        None => "Kt".to_string(),
    }
}

fn kotlin_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = match (enclosing_func, enclosing_class) {
        (Some(func), _) => qualify(file_path, func, enclosing_class),
        (None, Some(class)) => qualify(file_path, class, None),
        (None, None) => file_path.to_string(),
    };
    if let Some(call_name) = kotlin_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target: call_name,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = kotlin_call_signature(node, source)
        && let Some(edge) = kotlin_bridge_edge(node, source, file_path, &caller, &signature)
    {
        edges.push(edge);
    }
}

fn kotlin_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = kotlin_call_callee(node)?;
    if callee.kind() == "simple_identifier" {
        return Some(node_text(callee, source));
    }
    kotlin_last_descendant_text(callee, source, &["simple_identifier", "type_identifier"])
}

fn kotlin_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = kotlin_call_callee(node)?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn kotlin_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "call_suffix")
}

fn kotlin_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "ProcessBuilder.start" => ("invokes_binary", "subprocess"),
        "System.loadLibrary" | "System.load" => ("loads_shared_library", "ffi"),
        "Files.readString"
        | "Files.readAllBytes"
        | "File.readText"
        | "File.readLines"
        | "File.bufferedReader" => ("reads_file", "file_io"),
        "Files.writeString" | "Files.write" | "File.writeText" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match kotlin_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "kotlin",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn kotlin_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let suffix = kotlin_direct_child(node, &["call_suffix"])?;
    let arguments = kotlin_first_descendant(suffix, &["value_arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "value_argument" {
            kotlin_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if arg.kind() == "string_literal" {
            return Some(kotlin_string_text(arg, source));
        }
        return None;
    }
    None
}

fn kotlin_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if let Some(content) = kotlin_first_descendant(node, &["string_content"]) {
        return node_text(content, source);
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn kotlin_first_non_punctuation_child<'a>(
    node: tree_sitter::Node<'a>,
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"))
}

fn kotlin_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn kotlin_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    kotlin_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn kotlin_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = kotlin_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn kotlin_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(value) = kotlin_last_descendant_text(child, source, kinds) {
            found = Some(value);
        }
    }
    found
}
