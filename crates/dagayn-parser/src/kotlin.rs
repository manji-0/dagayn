use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::java::{JvmReceiver, jvm_bind_local_receivers, jvm_mark_receiver};
use super::jni::jni_symbol;
use super::member_calls::CallOrigin;
use super::stdlib::java::{
    is_java_lang_class, jvm_class_package, jvm_package_of, jvm_path_has_root,
};
use super::stdlib::kotlin::{
    KOTLIN_STDLIB_ROOTS, is_kotlin_builtin_function, is_kotlin_builtin_type,
};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, direct_child, direct_child_text, first_descendant,
    last_descendant_text, line_count, line_of, node_text, set_declared_namespaces,
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
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "kotlin")];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let scope = KotlinStdlibScope::collect(tree.root_node(), source);
        kotlin_walk_children(
            tree.root_node(),
            source,
            &file_path,
            &scope,
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
        jvm_bind_local_receivers(&nodes, &mut edges);
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

#[allow(clippy::too_many_arguments)]
fn kotlin_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    scope: &KotlinStdlibScope,
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
                if let Some(name) = direct_child_text(child, source, &["type_identifier"]) {
                    kotlin_emit_type(child, source, file_path, &name, owner(), nodes, edges);
                    let path = match owner() {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name.clone(),
                    };
                    kotlin_walk_children(
                        child,
                        source,
                        file_path,
                        scope,
                        Some(&path),
                        None,
                        nodes,
                        edges,
                    );
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
                    scope,
                    enclosing_class,
                    Some("constructor"),
                    nodes,
                    edges,
                );
                continue;
            }
            "function_declaration" => {
                if let Some(name) = direct_child_text(child, source, &["simple_identifier"]) {
                    kotlin_emit_function(child, source, file_path, &name, owner(), nodes, edges);
                    kotlin_walk_children(
                        child,
                        source,
                        file_path,
                        scope,
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
                    scope,
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
            scope,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

/// An import of the standard library targets its package (`java.io` for
/// `import java.io.File`, `kotlin.math` for `import kotlin.math.max`).
fn kotlin_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(mut target) = kotlin_import_target(node, source) else {
        return;
    };
    let mut extra = json!({});
    let wildcard = kotlin_is_wildcard_import(node, source);
    if jvm_path_has_root(&target, KOTLIN_STDLIB_ROOTS)
        && let Some(package) = jvm_package_of(&target, wildcard)
    {
        if wildcard {
            target.push_str(".*");
        }
        mark_stdlib_edge(&mut target, &mut extra, &package, StdlibEvidence::Certain);
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: file_path.to_string(),
        target,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
}

fn kotlin_is_wildcard_import(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    node_text(node, source).trim_end().ends_with('*')
}

/// The imported path, without the `import` keyword or an `as` alias.
///
/// The whole statement used to be the target, so `import java.util.UUID`
/// could never match a package index entry.
fn kotlin_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let target = direct_child_text(node, source, &["identifier"])?;
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
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|parent| qualify(file_path, parent, None))
            .unwrap_or_else(|| file_path.to_string()),
        qualified.clone(),
        file_path.clone(),
        line_of(node),
    ));
    let mut cursor = node.walk();
    for specifier in node.children(&mut cursor) {
        if specifier.kind() != "delegation_specifier" {
            continue;
        }
        // `Base(a)` invokes a superclass constructor; a bare type names an
        // interface (or a delegated one via `by`).
        let invocation = direct_child(specifier, &["constructor_invocation"]);
        let is_class = invocation.is_some();
        let Some(target) = direct_child(invocation.unwrap_or(specifier), &["user_type"])
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
    let Some(modifiers) = direct_child(node, &["modifiers"]) else {
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
        return_type: kotlin_return_type(node, source),
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        qualified,
        file_path.clone(),
        line_of(node),
    ));
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
    let modifiers = direct_child(node, &["modifiers"])?;
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
                    let name = direct_child_text(current, source, &["type_identifier"]);
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
    let package = direct_child(root, &["package_header"])
        .and_then(|header| direct_child_text(header, source, &["identifier"]));
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
    scope: &KotlinStdlibScope,
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
        let mut target = call_name;
        let mut extra = json!({});
        if let Some((package, evidence, symbol)) = kotlin_stdlib_call(node, source, scope, &target)
        {
            target = symbol;
            mark_stdlib_edge(&mut target, &mut extra, &package, evidence);
        } else {
            jvm_mark_receiver(
                kotlin_call_receiver(node, source, scope, &target),
                &mut extra,
            );
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
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
    last_descendant_text(callee, source, &["simple_identifier", "type_identifier"])
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
    let suffix = direct_child(node, &["call_suffix"])?;
    let arguments = first_descendant(suffix, &["value_arguments"])?;
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
    if let Some(content) = first_descendant(node, &["string_content"]) {
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

/// The names of a file that decide whether a call reaches the standard
/// library: what it declares, and what its imports bind.
#[derive(Default)]
struct KotlinStdlibScope {
    /// Classes, objects, functions, and type aliases this file declares: a
    /// `println` of its own is never the standard library's.
    declared: HashSet<String>,
    /// The name each import binds (its alias, or its last segment) to its
    /// path when that is in the standard library (`File` ->
    /// `java.io.File`), or to `None` when it comes from elsewhere.
    imported: HashMap<String, Option<String>>,
    /// Packages of the standard library imported whole (`java.io.*`).
    stdlib_wildcards: Vec<String>,
    /// A wildcard import from elsewhere may bring in a class named like
    /// one of `java.lang` or a whole-imported package.
    foreign_wildcard: bool,
    /// Classes and objects this file declares: a receiver typed by one
    /// keeps the same-file binding, one typed by any other class is left to
    /// resolution across files.
    classes: HashSet<String>,
}

impl KotlinStdlibScope {
    fn collect(root: tree_sitter::Node<'_>, source: &[u8]) -> Self {
        let mut scope = Self::default();
        if let Some(imports) = direct_child(root, &["import_list"]) {
            let mut cursor = imports.walk();
            for header in imports.children(&mut cursor) {
                scope.add_import(header, source);
            }
        }
        let mut cursor = root.walk();
        for header in root.children(&mut cursor) {
            scope.add_import(header, source);
        }
        kotlin_collect_declared_names(root, source, &mut scope.declared);
        kotlin_collect_class_names(root, source, &mut scope.classes);
        scope
    }

    fn add_import(&mut self, header: tree_sitter::Node<'_>, source: &[u8]) {
        if header.kind() != "import_header" {
            return;
        }
        let Some(path) = kotlin_import_target(header, source) else {
            return;
        };
        let stdlib = jvm_path_has_root(&path, KOTLIN_STDLIB_ROOTS);
        if kotlin_is_wildcard_import(header, source) {
            if stdlib {
                self.stdlib_wildcards.push(path);
            } else {
                self.foreign_wildcard = true;
            }
            return;
        }
        let name = direct_child(header, &["import_alias"])
            .and_then(|alias| {
                last_descendant_text(alias, source, &["type_identifier", "simple_identifier"])
            })
            .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(&path).to_string());
        self.imported.insert(name, stdlib.then_some(path));
    }

    /// The package of a class named bare (`File`, `System`), and how sure
    /// that is: an import of it or of its package, a class of `java.lang`
    /// (in scope in every Kotlin/JVM file), or — only likely — one of the
    /// types Kotlin imports into every file (`List`, `String`).
    fn resolve_type(&self, name: &str) -> Option<(String, StdlibEvidence)> {
        if self.declared.contains(name) {
            return None;
        }
        if let Some(imported) = self.imported.get(name) {
            let package = jvm_package_of(imported.as_deref()?, false)?;
            return Some((package, StdlibEvidence::Certain));
        }
        if is_kotlin_builtin_type(name) {
            return Some(("kotlin".to_string(), StdlibEvidence::Likely));
        }
        let package = jvm_class_package(name)?;
        if !is_java_lang_class(name) && !self.stdlib_wildcards.iter().any(|p| p == package) {
            return None;
        }
        let evidence = if self.foreign_wildcard {
            StdlibEvidence::Likely
        } else {
            StdlibEvidence::Certain
        };
        Some((package.to_string(), evidence))
    }
}

fn kotlin_collect_declared_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    let name = match node.kind() {
        "class_declaration" | "object_declaration" | "type_alias" | "type_parameter" => {
            direct_child_text(node, source, &["type_identifier"])
        }
        "function_declaration" => direct_child_text(node, source, &["simple_identifier"]),
        _ => None,
    };
    names.extend(name);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        kotlin_collect_declared_names(child, source, names);
    }
}

/// The standard-library package a call reaches, how sure that is, and the
/// call as written or resolved (`println`, `File.readText`,
/// `kotlin.math.max`):
///
/// - a function or class an import binds (`File("x")` after `import
///   java.io.File`), or a class of `java.lang` (`System.getenv`);
/// - a path through the library (`kotlin.math.max`, `java.nio.file.Path.of`);
/// - a member of a value it just constructed (`File("x").readText()`) or of
///   a string literal;
/// - only likely: a function Kotlin imports into every file (`println`,
///   `listOf`), or a method of a variable typed by its declaration or
///   initializer (`val f = File(p); f.readText()`).
fn kotlin_stdlib_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    scope: &KotlinStdlibScope,
    name: &str,
) -> Option<(String, StdlibEvidence, String)> {
    let callee = kotlin_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => {
            if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                let (package, evidence) = scope.resolve_type(name)?;
                return Some((package, evidence, name.to_string()));
            }
            if scope.declared.contains(name) || kotlin_variable_type(node, name, source).is_some() {
                return None;
            }
            if let Some(imported) = scope.imported.get(name) {
                let package = jvm_package_of(imported.as_deref()?, false)?;
                return Some((package, StdlibEvidence::Certain, name.to_string()));
            }
            is_kotlin_builtin_function(name).then(|| {
                (
                    "kotlin".to_string(),
                    StdlibEvidence::Likely,
                    name.to_string(),
                )
            })
        }
        "navigation_expression" => {
            let receiver = callee.named_child(0)?;
            let written = || format!("{}.{name}", node_text(receiver, source).trim());
            match receiver.kind() {
                "string_literal" => Some((
                    "kotlin".to_string(),
                    StdlibEvidence::Certain,
                    format!("String.{name}"),
                )),
                // `File("x").readText()`: a member of what it constructs.
                "call_expression" => {
                    let class = kotlin_call_callee(receiver)
                        .filter(|callee| callee.kind() == "simple_identifier")
                        .map(|callee| node_text(callee, source))
                        .filter(|class| class.starts_with(|c: char| c.is_ascii_uppercase()))?;
                    let (package, evidence) = scope.resolve_type(&class)?;
                    Some((package, evidence, format!("{class}.{name}")))
                }
                "simple_identifier" | "navigation_expression" => {
                    let segments = kotlin_dotted_name(receiver, source)?;
                    let first = segments.first()?;
                    if first.starts_with(|c: char| c.is_ascii_uppercase()) {
                        let (package, evidence) = scope.resolve_type(first)?;
                        return Some((package, evidence, written()));
                    }
                    let variable = kotlin_variable_type(node, first, source);
                    if segments.len() == 1 {
                        let class = variable??;
                        let (package, _) = scope.resolve_type(&class)?;
                        return Some((package, StdlibEvidence::Likely, format!("{class}.{name}")));
                    }
                    if variable.is_some()
                        || scope.declared.contains(first)
                        || scope.imported.contains_key(first)
                        || !KOTLIN_STDLIB_ROOTS.contains(&first.as_str())
                    {
                        return None;
                    }
                    let path = written();
                    let package = jvm_package_of(&path, false)?;
                    Some((package, StdlibEvidence::Certain, path))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// `a.b.c` as its segments, when it is only names.
fn kotlin_dotted_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "simple_identifier" => Some(vec![node_text(node, source)]),
        "navigation_expression" => {
            let mut segments = kotlin_dotted_name(node.named_child(0)?, source)?;
            let suffix = direct_child(node, &["navigation_suffix"])?;
            segments.push(direct_child_text(suffix, source, &["simple_identifier"])?);
            Some(segments)
        }
        _ => None,
    }
}

/// The class of the variable `name` visible at `node`: a parameter or a
/// property declared before it, typed by its annotation (`xs: List<Int>`)
/// or by the class it constructs (`val f = File(p)`). `Some(None)` when
/// the nearest declaration gives no class to go by (`val n = f()`, a
/// lambda or loop variable), so an outer one of the same name does not
/// count.
fn kotlin_variable_type(
    node: tree_sitter::Node<'_>,
    name: &str,
    source: &[u8],
) -> Option<Option<String>> {
    kotlin_variable_declaration(node, name, source).map(|declared| match declared {
        KotlinDeclared::Type(class) => Some(class),
        KotlinDeclared::Value(value) => {
            value.and_then(|value| kotlin_constructed_class(value, source))
        }
    })
}

/// What the nearest declaration of a variable says about it: the class of
/// its annotation (`repo: Repo`), or else the value it is initialized with,
/// if any (`val conn = store.connect()`).
#[derive(Clone)]
enum KotlinDeclared<'a> {
    Type(String),
    Value(Option<tree_sitter::Node<'a>>),
}

/// The class `Repo(...)` constructs: a call of a capitalized name.
fn kotlin_constructed_class(value: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    Some(value)
        .filter(|value| value.kind() == "call_expression")
        .and_then(kotlin_call_callee)
        .filter(|callee| callee.kind() == "simple_identifier")
        .map(|callee| node_text(callee, source))
        .filter(|class| class.starts_with(|c: char| c.is_ascii_uppercase()))
}

/// The class a type annotation names (`Repo` for `Repo?`, `List` for
/// `List<Int>`).
fn kotlin_annotated_class(declaration: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    direct_child(declaration, &["user_type", "nullable_type"])
        .and_then(|ty| first_descendant(ty, &["type_identifier"]))
        .map(|ty| node_text(ty, source))
}

/// A `val` / `var` naming `name`: its annotation, or else its initializer
/// (the expression after `=`).
fn kotlin_property_named<'a>(
    property: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<KotlinDeclared<'a>> {
    let declaration = direct_child(property, &["variable_declaration"])?;
    if direct_child_text(declaration, source, &["simple_identifier"]).as_deref() != Some(name) {
        return None;
    }
    if let Some(class) = kotlin_annotated_class(declaration, source) {
        return Some(KotlinDeclared::Type(class));
    }
    let mut cursor = property.walk();
    let value = property
        .children(&mut cursor)
        .skip_while(|child| child.kind() != "=")
        .find(|child| child.is_named());
    Some(KotlinDeclared::Value(value))
}

/// The nearest declaration of the variable `name` visible at `node` (see
/// [`kotlin_variable_type`]). A property of a class counts wherever it is
/// declared in the class body.
fn kotlin_variable_declaration<'a>(
    node: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<KotlinDeclared<'a>> {
    let names_it = |declaration: tree_sitter::Node<'_>| {
        direct_child_text(declaration, source, &["simple_identifier"])
            .is_some_and(|var| var == name)
    };
    let mut current = node;
    while let Some(scope) = current.parent() {
        let mut cursor = scope.walk();
        for child in scope.children(&mut cursor) {
            let found = match child.kind() {
                "property_declaration"
                    if child.start_byte() < node.start_byte() || scope.kind() == "class_body" =>
                {
                    kotlin_property_named(child, name, source)
                }
                "function_value_parameters" | "primary_constructor" | "class_parameters" => {
                    let mut inner = child.walk();
                    child
                        .children(&mut inner)
                        .filter(|param| matches!(param.kind(), "parameter" | "class_parameter"))
                        .find(|param| names_it(*param))
                        .map(|param| match kotlin_annotated_class(param, source) {
                            Some(class) => KotlinDeclared::Type(class),
                            None => KotlinDeclared::Value(None),
                        })
                }
                "lambda_parameters" | "variable_declaration"
                    if matches!(scope.kind(), "lambda_literal" | "for_statement") =>
                {
                    node_text(child, source)
                        .split([',', ':', '(', ')'])
                        .any(|part| part.trim() == name)
                        .then_some(KotlinDeclared::Value(None))
                }
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        current = scope;
    }
    None
}

/// The property `name` of the class enclosing `at` (`this.repo`): one of
/// its body or of its primary constructor.
fn kotlin_property_declaration<'a>(
    at: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<KotlinDeclared<'a>> {
    let mut current = at;
    while let Some(scope) = current.parent() {
        if scope.kind() == "class_body" {
            let mut cursor = scope.walk();
            let first = scope.children(&mut cursor).next()?;
            return kotlin_variable_declaration(first, name, source);
        }
        current = scope;
    }
    None
}

/// The classes and objects a file declares, at any depth.
fn kotlin_collect_class_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    if matches!(node.kind(), "class_declaration" | "object_declaration") {
        names.extend(direct_child_text(node, source, &["type_identifier"]));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        kotlin_collect_class_names(child, source, names);
    }
}

/// The declared return type as written (`User?`, `List<User>`): the type
/// after the parameters' `:`. `fun String.trimmed()` has its receiver type
/// before the name, which is not it.
fn kotlin_return_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "function_declaration" {
        return None;
    }
    let mut cursor = node.walk();
    let mut children = node
        .children(&mut cursor)
        .skip_while(|child| child.kind() != "function_value_parameters")
        .skip(1);
    if children.next()?.kind() != ":" {
        return None;
    }
    let ty = children.next()?;
    Some(node_text(ty, source).trim().to_string())
}

/// A member call's receiver, past calls of the same method: one line
/// holds a single edge per target, so in `b.with(1).with(2)` the edge of
/// `with` stands for both and its receiver is `b`.
fn kotlin_call_receiver(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    scope: &KotlinStdlibScope,
    method: &str,
) -> JvmReceiver {
    let mut receiver = kotlin_navigation_receiver(node);
    while let Some(inner) = receiver.filter(|inner| {
        inner.kind() == "call_expression"
            && kotlin_navigation_receiver(*inner).is_some()
            && kotlin_call_name(*inner, source).as_deref() == Some(method)
    }) {
        receiver = kotlin_navigation_receiver(inner);
    }
    match receiver {
        Some(receiver) => kotlin_expression_receiver(receiver, node, source, scope),
        None => JvmReceiver::Known,
    }
}

/// `repo` of `repo.save()` / `repo?.save()`; none for a bare call.
fn kotlin_navigation_receiver(call: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    kotlin_call_callee(call)
        .filter(|callee| callee.kind() == "navigation_expression")
        .and_then(|callee| callee.named_child(0))
}

/// What the expression a method is called on is (see [`JvmReceiver`]);
/// variables are looked up from `at`.
fn kotlin_expression_receiver(
    expression: tree_sitter::Node<'_>,
    at: tree_sitter::Node<'_>,
    source: &[u8],
    scope: &KotlinStdlibScope,
) -> JvmReceiver {
    match expression.kind() {
        "simple_identifier" => {
            let name = node_text(expression, source);
            match kotlin_variable_declaration(at, &name, source) {
                Some(declared) => kotlin_declared_receiver(declared, source, scope),
                // A class or object (`Repo.create()`), or a name an import
                // binds.
                None if name.starts_with(|c: char| c.is_ascii_uppercase())
                    || scope.imported.contains_key(&name) =>
                {
                    JvmReceiver::Known
                }
                // `it`, or a property the class inherits.
                None => JvmReceiver::Unknown(None),
            }
        }
        "navigation_expression" => {
            let Some(segments) = kotlin_dotted_name(expression, source) else {
                return match expression.named_child(0).map(|inner| inner.kind()) {
                    Some("this_expression") => direct_child(expression, &["navigation_suffix"])
                        .and_then(|suffix| {
                            direct_child_text(suffix, source, &["simple_identifier"])
                        })
                        .and_then(|field| kotlin_property_declaration(at, &field, source))
                        .map_or(JvmReceiver::Unknown(None), |declared| {
                            kotlin_declared_receiver(declared, source, scope)
                        }),
                    _ => JvmReceiver::Unknown(None),
                };
            };
            // `a.b.run()` on a variable is of a type unknown; a path to a
            // class or through a package (`com.acme.Util.run()`) is not.
            if kotlin_variable_declaration(at, &segments[0], source).is_some()
                && segments
                    .last()
                    .is_some_and(|last| !last.starts_with(|c: char| c.is_ascii_uppercase()))
            {
                JvmReceiver::Unknown(None)
            } else {
                JvmReceiver::Known
            }
        }
        "call_expression" => match kotlin_constructed_class(expression, source) {
            Some(class) => kotlin_class_receiver(&class, scope),
            None => JvmReceiver::Unknown(kotlin_call_origin(expression, source)),
        },
        "as_expression" => direct_child(expression, &["user_type", "nullable_type"])
            .and_then(|ty| first_descendant(ty, &["type_identifier"]))
            .map_or(JvmReceiver::Known, |ty| {
                kotlin_class_receiver(&node_text(ty, source), scope)
            }),
        "postfix_expression" | "parenthesized_expression" => expression
            .named_child(0)
            .map_or(JvmReceiver::Known, |inner| {
                kotlin_expression_receiver(inner, at, source, scope)
            }),
        "indexing_expression" | "if_expression" | "when_expression" | "elvis_expression" => {
            JvmReceiver::Unknown(None)
        }
        _ => JvmReceiver::Known,
    }
}

fn kotlin_declared_receiver(
    declared: KotlinDeclared<'_>,
    source: &[u8],
    scope: &KotlinStdlibScope,
) -> JvmReceiver {
    match declared {
        KotlinDeclared::Type(class) => kotlin_class_receiver(&class, scope),
        KotlinDeclared::Value(Some(value)) if value.kind() == "call_expression" => {
            match kotlin_constructed_class(value, source) {
                Some(class) => kotlin_class_receiver(&class, scope),
                // `val conn = store.connect()`: what `connect` returns.
                None => JvmReceiver::Unknown(kotlin_call_origin(value, source)),
            }
        }
        KotlinDeclared::Value(_) => JvmReceiver::Unknown(None),
    }
}

/// A receiver of class `class`: nothing for the standard library; the
/// class of this file or of another; a type parameter says nothing.
fn kotlin_class_receiver(class: &str, scope: &KotlinStdlibScope) -> JvmReceiver {
    if scope.resolve_type(class).is_some() {
        JvmReceiver::Known
    } else if scope.classes.contains(class) {
        JvmReceiver::Local(class.to_string())
    } else if scope.declared.contains(class) {
        JvmReceiver::Unknown(None)
    } else if class.starts_with(|c: char| c.is_ascii_uppercase()) {
        JvmReceiver::Foreign(class.to_string())
    } else {
        JvmReceiver::Known
    }
}

/// The call a receiver is the result of (`store.connect()` -> `connect`
/// on its line).
fn kotlin_call_origin(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<CallOrigin> {
    Some(CallOrigin {
        name: kotlin_call_name(call, source)?,
        line: call.start_position().row as i64 + 1,
        unwrap: false,
    })
}
