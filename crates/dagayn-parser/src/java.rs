use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::json;

use super::jni::jni_symbol;
use super::member_calls::CallOrigin;
use super::stdlib::java::{
    JAVA_STDLIB_ROOTS, is_java_lang_class, jvm_class_package, jvm_package_of, jvm_path_has_root,
};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, is_test_file, line_count, node_text, normalize_relative_path,
    set_declared_namespaces, strip_matching_quotes, type_name_without_arguments,
};
use super::{qualify, resolve_rust_call_targets};

struct JavaParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    repo_root: Option<&'a Path>,
    /// The `package` declaration, which prefixes JNI symbol names.
    package: Option<String>,
    /// The names that decide whether a call reaches the Java class library.
    stdlib: JavaStdlibScope,
    /// Classes, interfaces, enums, and records this file declares: a
    /// receiver typed by one keeps the same-file binding, one typed by any
    /// other class is left to resolution across files.
    type_names: HashSet<String>,
}

pub(super) fn parse_java_with_parser(
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
        language: "java".to_string(),
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
        let context = JavaParseContext {
            source,
            file_path: file_path.clone(),
            repo_root,
            package: java_package(tree.root_node(), source),
            stdlib: JavaStdlibScope::collect(tree.root_node(), source, &file_path, repo_root),
            type_names: java_collect_type_names_declared(tree.root_node(), source),
        };
        java_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        set_declared_namespaces(
            &mut nodes,
            collect_namespace_paths(
                tree.root_node(),
                source,
                &["package_declaration"],
                None,
                &["scoped_identifier", "identifier"],
            ),
        );
        jvm_bind_local_receivers(&nodes, &mut edges);
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

fn java_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
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
            "import_declaration" => {
                java_emit_import(child, context, edges);
            }
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration" => {
                if let Some(name) = java_type_name(child, context.source) {
                    java_emit_type(
                        child,
                        context.source,
                        &context.file_path,
                        &name,
                        owner(),
                        nodes,
                        edges,
                    );
                    let path = java_scope_join(owner(), &name);
                    java_walk_children(child, context, Some(&path), None, nodes, edges);
                    continue;
                }
            }
            "object_creation_expression"
                if let Some(body) = java_anonymous_body(child)
                    && let Some(base) = child
                        .child_by_field_name("type")
                        .map(|ty| java_simple_type_name(ty, context.source)) =>
            {
                let owner = match (enclosing_class, enclosing_func) {
                    (Some(class), Some(func)) => format!("{class}.{func}"),
                    (Some(class), None) => class.to_string(),
                    (None, Some(func)) => func.to_string(),
                    (None, None) => String::new(),
                };
                let owner = (!owner.is_empty()).then_some(owner);
                java_emit_anonymous_class(
                    child,
                    &context.file_path,
                    &base,
                    owner.as_deref(),
                    nodes,
                    edges,
                );
                let mut cursor = child.walk();
                for part in child.children(&mut cursor) {
                    if part.id() != body.id() {
                        java_walk_children(
                            part,
                            context,
                            enclosing_class,
                            enclosing_func,
                            nodes,
                            edges,
                        );
                    }
                }
                let path = java_scope_join(owner.as_deref(), &base);
                java_walk_children(body, context, Some(&path), None, nodes, edges);
                continue;
            }
            "method_declaration"
            | "constructor_declaration"
            | "compact_constructor_declaration" => {
                if let Some(name) = java_function_name(child, context.source) {
                    java_emit_function(child, context, &name, owner(), nodes, edges);
                    java_walk_children(child, context, owner(), Some(&name), nodes, edges);
                    continue;
                }
            }
            "method_invocation" => {
                java_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        java_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

/// An import of the class library targets its package (`java.util` for
/// `import java.util.List`, `java.lang` for `import static
/// java.lang.Math.max`); one of this repository targets its file.
fn java_emit_import(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    edges: &mut Vec<ParsedEdge>,
) {
    let file_path = &context.file_path;
    let Some(import_target) = java_import_target(node, context.source) else {
        return;
    };
    let resolved = resolve_java_import_target(&import_target, file_path, context.repo_root);
    let mut extra = json!({});
    let target = match resolved {
        Some(resolved) => resolved,
        None => {
            let mut target = import_target;
            if jvm_path_has_root(&target, JAVA_STDLIB_ROOTS)
                && let Some(package) = java_import_package(&target)
            {
                mark_stdlib_edge(&mut target, &mut extra, &package, StdlibEvidence::Certain);
            }
            target
        }
    };
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: file_path.to_string(),
        target,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
}

/// The package an import path names: `java.io` for `java.io.*` and for
/// `java.io.File`.
fn java_import_package(path: &str) -> Option<String> {
    match path.strip_suffix(".*") {
        Some(prefix) => jvm_package_of(prefix, true),
        None => jvm_package_of(path, false),
    }
}

fn java_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let target = text
        .trim()
        .trim_start_matches("import")
        .trim()
        .trim_start_matches("static")
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();
    (!target.is_empty()).then_some(target)
}

fn resolve_java_import_target(
    target: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    if target.ends_with(".*") {
        return None;
    }
    java_resolve_module_to_file(target, file_path, repo_root).or_else(|| {
        target
            .rfind('.')
            .and_then(|dot| java_resolve_module_to_file(&target[..dot], file_path, repo_root))
    })
}

fn java_resolve_module_to_file(
    module: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    let relative = module.replace('.', "/") + ".java";
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    if let Some(repo_root) = repo_root {
        let mut current = repo_root.join(caller_dir);
        loop {
            let candidate = current.join(&relative);
            if candidate.is_file() {
                return candidate
                    .strip_prefix(repo_root)
                    .ok()
                    .map(normalize_relative_path);
            }
            if !current.pop() {
                break;
            }
        }
        return None;
    }

    let mut current = caller_dir.to_path_buf();
    loop {
        let candidate = current.join(&relative);
        if candidate.is_file() {
            return Some(normalize_relative_path(&candidate));
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn java_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = java_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
        if java_is_value_container(type_role) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "java".to_string(),
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
    for (base, role) in java_bases(node, source) {
        edges.push(ParsedEdge {
            kind: if role == "implements" {
                crate::core::types::EdgeKind::Implements
            } else {
                crate::core::types::EdgeKind::Inherits
            },
            source: qualified.clone(),
            target: base,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": role,
                "syntax_source": node.kind(),
            }),
        });
    }
}

fn java_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_direct_child_text(node, source, &["identifier", "type_identifier"])
}

fn java_type_role(node: tree_sitter::Node<'_>, source: &[u8]) -> (&'static str, bool, bool) {
    if node.kind() == "interface_declaration" {
        return ("interface", true, true);
    }
    if node.kind() == "enum_declaration" {
        return ("enum", false, false);
    }
    if node.kind() == "record_declaration" {
        return ("record", false, false);
    }
    let is_abstract = java_direct_child_text(node, source, &["modifiers"])
        .is_some_and(|mods| mods.split_whitespace().any(|part| part == "abstract"));
    if is_abstract {
        ("abstract_class", true, false)
    } else {
        ("class", false, false)
    }
}

fn java_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "record" | "enum")
}

fn java_bases(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<(String, &'static str)> {
    let mut bases = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "superclass" => java_collect_type_names(child, source, "extends", &mut bases),
            "super_interfaces" => {
                java_collect_type_names(child, source, "implements", &mut bases);
            }
            "extends_interfaces" => java_collect_type_names(child, source, "extends", &mut bases),
            _ => {}
        }
    }
    bases
}

fn java_collect_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    role: &'static str,
    bases: &mut Vec<(String, &'static str)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "type_identifier" | "generic_type" | "scoped_type_identifier"
        ) {
            if let Some(base) = type_name_without_arguments(child, source) {
                bases.push((base, role));
            }
        } else {
            java_collect_type_names(child, source, role, bases);
        }
    }
}

fn java_emit_function(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, &context.file_path);
    let qualified = qualify(file_path, name, enclosing_class);
    // A `native` method is implemented by the C symbol its JNI name spells.
    let extra = match enclosing_class {
        Some(class) if node.kind() == "method_declaration" && java_is_native(node) => json!({
            "ffi_import": {
                "abi": "jni",
                "symbol": jni_symbol(context.package.as_deref(), class, name),
            }
        }),
        _ => json!({}),
    };
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "java".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: java_direct_child_text(node, source, &["formal_parameters"]),
        // As written (`List<User>`, `Optional<Repo>`): resolution across
        // files types what a call of this method returns by it.
        return_type: (node.kind() == "method_declaration")
            .then(|| java_field_text(node, source, "type"))
            .flatten(),
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

fn java_is_native(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.kind() == "modifiers")
        .any(|modifiers| {
            let mut inner = modifiers.walk();
            modifiers
                .children(&mut inner)
                .any(|modifier| modifier.kind() == "native")
        })
}

fn java_package(root: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = root.walk();
    let declaration = root
        .children(&mut cursor)
        .find(|child| child.kind() == "package_declaration")?;
    java_direct_child_text(declaration, source, &["scoped_identifier", "identifier"])
}

fn java_scope_join(enclosing: Option<&str>, name: &str) -> String {
    match enclosing {
        Some(parent) => format!("{parent}.{name}"),
        None => name.to_string(),
    }
}

fn java_anonymous_body(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == "class_body")
}

fn java_simple_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = node_text(node, source);
    let base = text.split('<').next().unwrap_or(&text).trim();
    base.rsplit('.').next().unwrap_or(base).to_string()
}

/// Anonymous classes are named after their base type and scoped under the
/// member that creates them, so `new Runnable() { run() }` inside `Outer.run`
/// becomes `Outer.run.Runnable.run` instead of colliding with `Outer.run`.
fn java_emit_anonymous_class(
    node: tree_sitter::Node<'_>,
    file_path: &FilePath,
    base: &str,
    owner: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, base, owner);
    let line = node.start_position().row as i64 + 1;
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: base.to_string(),
        file_path: file_path.clone(),
        line_start: line,
        line_end: node.end_position().row as i64 + 1,
        language: "java".to_string(),
        parent_name: owner.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class", "is_anonymous": true}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: owner
            .map(|owner| qualify(file_path, owner, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified.clone(),
        file_path: file_path.clone(),
        line,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Inherits,
        source: qualified,
        target: base.to_string(),
        file_path: file_path.clone(),
        line,
        extra: json!({}),
    });
}

fn java_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_field_text(node, source, "name")
        .or_else(|| java_direct_child_text(node, source, &["identifier"]))
}

fn java_field_text(node: tree_sitter::Node<'_>, source: &[u8], field: &str) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    let text = node_text(child, source).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn java_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, &context.file_path);
    let caller = match (enclosing_func, enclosing_class) {
        (Some(func), _) => qualify(file_path, func, enclosing_class),
        (None, Some(class)) => qualify(file_path, class, None),
        (None, None) => file_path.to_string(),
    };

    if let Some(call_name) = java_call_name(node, source) {
        let mut target = call_name;
        let mut extra = json!({});
        if let Some((package, evidence, symbol)) = java_stdlib_call(node, context, &target) {
            target = symbol;
            mark_stdlib_edge(&mut target, &mut extra, &package, evidence);
        } else {
            jvm_mark_receiver(java_call_receiver(node, context, &target), &mut extra);
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

    if let Some(signature) = java_call_signature(node, source)
        && let Some(edge) = java_bridge_edge(node, source, file_path, &caller, &signature)
    {
        edges.push(edge);
    }
}

/// Reads the invoked method from the `name` field.
///
/// `method_invocation` puts the receiver first, so taking the first
/// non-`argument_list` child made `Broker.build(t)` point at `Broker` — the
/// class — instead of `build`, losing every qualified call.
fn java_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_field_text(node, source, "name")
}

fn java_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            break;
        }
        parts.push(node_text(child, source));
    }
    let signature = parts.join("").trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn java_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "Runtime.exec" => ("invokes_binary", "subprocess"),
        "System.loadLibrary"
        | "System.load"
        | "Runtime.getRuntime().loadLibrary"
        | "Runtime.getRuntime().load" => ("loads_shared_library", "ffi"),
        "Files.readString" | "Files.readAllBytes" => ("reads_file", "file_io"),
        "Files.writeString" | "Files.write" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match java_first_string_arg(node, source) {
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
            "source_language": "java",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn java_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string_literal" {
            return Some(java_string_text(child, source));
        }
        return None;
    }
    None
}

fn java_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn java_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

/// The names of a file that decide whether a call reaches the Java class
/// library: what it declares, and what its imports bind.
#[derive(Default)]
struct JavaStdlibScope {
    /// Types, methods, and type parameters this file declares: a `Math`
    /// or `max` of its own is never the class library's.
    declared: HashSet<String>,
    /// The simple name each single import binds to its path when that is
    /// in the class library (`List` -> `java.util.List`, `max` ->
    /// `java.lang.Math.max`), or to `None` when it comes from elsewhere.
    imported: HashMap<String, Option<String>>,
    /// Packages of the class library imported whole (`java.util.*`).
    stdlib_wildcards: Vec<String>,
    /// A wildcard import from elsewhere (`com.acme.*`) may bring in a class
    /// named like one of `java.lang` or a whole-imported package.
    foreign_wildcard: bool,
}

impl JavaStdlibScope {
    fn collect(
        root: tree_sitter::Node<'_>,
        source: &[u8],
        file_path: &FilePath,
        repo_root: Option<&Path>,
    ) -> Self {
        let mut scope = Self::default();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() != "import_declaration" {
                continue;
            }
            let Some(path) = java_import_target(child, source) else {
                continue;
            };
            let stdlib = jvm_path_has_root(&path, JAVA_STDLIB_ROOTS)
                && resolve_java_import_target(&path, file_path, repo_root).is_none();
            let is_static = node_text(child, source).contains("static ");
            match path.strip_suffix(".*") {
                // `import static java.lang.Math.*` binds members this table
                // does not list.
                Some(package) if stdlib && !is_static => {
                    scope.stdlib_wildcards.push(package.to_string());
                }
                Some(_) => scope.foreign_wildcard |= !stdlib,
                None => {
                    let name = path.rsplit('.').next().unwrap_or(&path).to_string();
                    scope.imported.insert(name, stdlib.then_some(path));
                }
            }
        }
        java_collect_declared_names(root, source, &mut scope.declared);
        scope
    }

    /// The package of a type named without a package (`List`, `Math`), and
    /// how sure that is: an import of it, or a class of `java.lang` or of a
    /// whole-imported package, unless a wildcard import from elsewhere may
    /// bring in its own.
    fn resolve_type(&self, name: &str) -> Option<(String, StdlibEvidence)> {
        if self.declared.contains(name) {
            return None;
        }
        if let Some(imported) = self.imported.get(name) {
            let package = jvm_package_of(imported.as_deref()?, false)?;
            return Some((package, StdlibEvidence::Certain));
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

    /// [`Self::resolve_type`] for a type as written: simple (`List`), nested
    /// (`Map.Entry`), or fully qualified (`java.util.List`).
    fn resolve_type_path(&self, path: &[String]) -> Option<(String, StdlibEvidence)> {
        let first = path.first()?;
        if first.starts_with(|c: char| c.is_ascii_uppercase()) {
            return self.resolve_type(first);
        }
        if path.len() < 2
            || !JAVA_STDLIB_ROOTS.contains(&first.as_str())
            || self.declared.contains(first)
            || self.imported.contains_key(first)
        {
            return None;
        }
        jvm_package_of(&path.join("."), false).map(|package| (package, StdlibEvidence::Certain))
    }
}

fn java_collect_declared_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    if matches!(
        node.kind(),
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration"
            | "method_declaration"
    ) && let Some(name) = java_field_text(node, source, "name")
    {
        names.insert(name);
    }
    if node.kind() == "type_parameter"
        && let Some(name) = java_direct_child_text(node, source, &["type_identifier", "identifier"])
    {
        names.insert(name);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        java_collect_declared_names(child, source, names);
    }
}

/// The class-library package a method invocation reaches, how sure that
/// is, and the call as written or resolved (`System.out.println`,
/// `Math.max`, `List.add`):
///
/// - a static import of it (`max(a, b)` after `import static
///   java.lang.Math.max`);
/// - a member of a class it names (`Math.max`, `System.out.println`,
///   `java.util.List.of`), of a value it just constructed (`new
///   ArrayList<>().add(x)`), or of a string literal;
/// - a method of a variable whose declared type is one (`List<String> xs;
///   xs.add(x)`), only likely: the variable may hold a subclass of this
///   repository.
fn java_stdlib_call(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    name: &str,
) -> Option<(String, StdlibEvidence, String)> {
    let (source, scope) = (context.source, &context.stdlib);
    let Some(object) = node.child_by_field_name("object") else {
        if scope.declared.contains(name) {
            return None;
        }
        let path = scope.imported.get(name)?.as_deref()?;
        let package = jvm_package_of(path, false)?;
        let symbol = match path.rsplit('.').nth(1) {
            Some(owner) => format!("{owner}.{name}"),
            None => name.to_string(),
        };
        return Some((package, StdlibEvidence::Certain, symbol));
    };
    match object.kind() {
        "string_literal" => Some((
            "java.lang".to_string(),
            StdlibEvidence::Certain,
            format!("String.{name}"),
        )),
        "object_creation_expression" => {
            let path = java_type_path(object.child_by_field_name("type")?, source)?;
            let (package, evidence) = scope.resolve_type_path(&path)?;
            let class = path.last()?;
            Some((package, evidence, format!("{class}.{name}")))
        }
        "identifier" | "field_access" | "scoped_identifier" => {
            let segments = java_dotted_name(object, source)?;
            let first = segments.first()?;
            let written = format!("{}.{name}", segments.join("."));
            if first.starts_with(|c: char| c.is_ascii_uppercase()) {
                let (package, evidence) = scope.resolve_type(first)?;
                return Some((package, evidence, written));
            }
            if segments.len() == 1 {
                let declared = java_variable_type(node, first, source)??;
                let path = java_type_path(declared, source)?;
                let (package, _) = scope.resolve_type_path(&path)?;
                let class = path.last()?;
                return Some((package, StdlibEvidence::Likely, format!("{class}.{name}")));
            }
            if java_variable_type(node, first, source).is_some() {
                return None;
            }
            let (package, evidence) = scope.resolve_type_path(&segments)?;
            Some((package, evidence, written))
        }
        _ => None,
    }
}

/// `a.b.c` as its segments, when it is only names.
fn java_dotted_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" => Some(vec![node_text(node, source)]),
        "field_access" | "scoped_identifier" | "scoped_type_identifier" => {
            let (head, tail) = match node.kind() {
                "field_access" => ("object", "field"),
                _ => ("scope", "name"),
            };
            let (head, tail) = match (
                node.child_by_field_name(head),
                node.child_by_field_name(tail),
            ) {
                (Some(head), Some(tail)) => (head, tail),
                // `scoped_type_identifier` has no fields: its first and
                // last named children.
                _ => (
                    node.named_child(0)?,
                    node.named_child(node.named_child_count().checked_sub(1)? as u32)?,
                ),
            };
            let mut segments = java_dotted_name(head, source)?;
            segments.push(node_text(tail, source));
            Some(segments)
        }
        _ => None,
    }
}

/// A type as written, without its type arguments: `java.util.List` for
/// `java.util.List<String>`. Arrays and primitives have none.
fn java_type_path(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "generic_type" => java_type_path(node.named_child(0)?, source),
        _ => java_dotted_name(node, source),
    }
}

/// The declared type of the variable `name` visible at `node`: a local
/// declared before it, a parameter, a loop variable, or a field. `Some(None)`
/// when the nearest declaration gives no type to go by (`var x = f()`, a
/// lambda parameter), so an outer one of the same name does not count.
fn java_variable_type<'a>(
    node: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<Option<tree_sitter::Node<'a>>> {
    java_variable_declaration(node, name, source).map(JavaDeclared::constructed_type)
}

/// What the nearest declaration of a variable says about it: its declared
/// type (`Repo repo`), or, for `var` and a lambda parameter, the value it is
/// initialized with, if any (`var repo = openRepo()`).
#[derive(Clone, Copy)]
enum JavaDeclared<'a> {
    Type(tree_sitter::Node<'a>),
    Value(Option<tree_sitter::Node<'a>>),
}

impl<'a> JavaDeclared<'a> {
    /// The declared type, or with `var` the class it constructs (`var xs =
    /// new ArrayList<String>()`).
    fn constructed_type(self) -> Option<tree_sitter::Node<'a>> {
        match self {
            Self::Type(declared) => Some(declared),
            Self::Value(value) => value
                .filter(|value| value.kind() == "object_creation_expression")
                .and_then(|value| value.child_by_field_name("type")),
        }
    }
}

/// The nearest declaration of the variable `name` visible at `node` (see
/// [`java_variable_type`]).
fn java_variable_declaration<'a>(
    node: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<JavaDeclared<'a>> {
    let mut current = node;
    while let Some(scope) = current.parent() {
        match scope.kind() {
            "enhanced_for_statement"
                if scope
                    .child_by_field_name("name")
                    .is_some_and(|var| node_text(var, source) == name) =>
            {
                return Some(match scope.child_by_field_name("type") {
                    Some(declared) if node_text(declared, source) != "var" => {
                        JavaDeclared::Type(declared)
                    }
                    _ => JavaDeclared::Value(None),
                });
            }
            "lambda_expression"
                if scope
                    .child_by_field_name("parameters")
                    .is_some_and(|params| {
                        params.kind() != "formal_parameters"
                            && node_text(params, source)
                                .trim_matches(|c| c == '(' || c == ')')
                                .split(',')
                                .any(|param| param.trim() == name)
                    }) =>
            {
                return Some(JavaDeclared::Value(None));
            }
            _ => {}
        }
        let mut cursor = scope.walk();
        for child in scope.children(&mut cursor) {
            let found = match child.kind() {
                "local_variable_declaration" if child.start_byte() < node.start_byte() => {
                    java_declarator_type(child, name, source)
                }
                "field_declaration" => java_declarator_type(child, name, source),
                "formal_parameters" | "resource_specification" => {
                    let mut inner = child.walk();
                    child
                        .children(&mut inner)
                        .find(|param| {
                            param
                                .child_by_field_name("name")
                                .is_some_and(|var| node_text(var, source) == name)
                        })
                        .map(|param| match param.child_by_field_name("type") {
                            Some(declared) if node_text(declared, source) != "var" => {
                                JavaDeclared::Type(declared)
                            }
                            _ => JavaDeclared::Value(param.child_by_field_name("value")),
                        })
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

/// The type a declaration gives `name`: its declared type, or with `var`
/// the value it is initialized with.
fn java_declarator_type<'a>(
    declaration: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<JavaDeclared<'a>> {
    let mut cursor = declaration.walk();
    let declarator = declaration
        .children_by_field_name("declarator", &mut cursor)
        .find(|declarator| {
            declarator
                .child_by_field_name("name")
                .is_some_and(|var| node_text(var, source) == name)
        })?;
    match declaration.child_by_field_name("type") {
        Some(declared) if node_text(declared, source) != "var" => {
            Some(JavaDeclared::Type(declared))
        }
        _ => Some(JavaDeclared::Value(declarator.child_by_field_name("value"))),
    }
}

/// The classes, interfaces, enums, and records a file declares, at any
/// depth.
fn java_collect_type_names_declared(root: tree_sitter::Node<'_>, source: &[u8]) -> HashSet<String> {
    fn collect(node: tree_sitter::Node<'_>, source: &[u8], names: &mut HashSet<String>) {
        if matches!(
            node.kind(),
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
        ) && let Some(name) = java_type_name(node, source)
        {
            names.insert(name);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect(child, source, names);
        }
    }
    let mut names = HashSet::new();
    collect(root, source, &mut names);
    names
}

/// What a member call's receiver says about the type its method is looked
/// up on, in Java, Kotlin, and Scala.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JvmReceiver {
    /// Nothing to add: no receiver, `this` / `super`, a class or object (a
    /// static call, `Repo.create()`), a package, or the standard library.
    Known,
    /// A class of this file (`Repo repo = new Repo(); repo.save()`), bound
    /// by [`jvm_bind_local_receivers`].
    Local(String),
    /// A class of another file (`Repo repo; repo.save()`), which resolution
    /// across files matches by `receiver_type`.
    Foreign(String),
    /// A receiver of unknown type (`plugin.run()`, `make().save()`), and the
    /// call it is the result of when it is one.
    Unknown(Option<CallOrigin>),
}

/// The mark [`jvm_mark_receiver`] leaves for [`jvm_bind_local_receivers`].
const JVM_LOCAL_RECEIVER: &str = "local_receiver";

/// Records what a member call's receiver says (see [`JvmReceiver`]):
/// `receiver_type` for a class of another file, `receiver_unknown` (and
/// `receiver_from`) when its type is unknown, so no same-named function of
/// the file is taken for it.
pub(super) fn jvm_mark_receiver(receiver: JvmReceiver, extra: &mut serde_json::Value) {
    match receiver {
        JvmReceiver::Known => {}
        JvmReceiver::Local(type_name) => extra[JVM_LOCAL_RECEIVER] = json!(type_name),
        JvmReceiver::Foreign(type_name) => extra["receiver_type"] = json!(type_name),
        JvmReceiver::Unknown(origin) => {
            extra["receiver_unknown"] = json!(true);
            if let Some(origin) = origin {
                extra["receiver_from"] = origin.to_json();
            }
        }
    }
}

/// Rewrites a call on a receiver typed by a class of this file to
/// `Type::method` when that class declares the method, so `repo.save()`
/// binds `Repo.save` rather than the first `save` of the file; an inherited
/// method keeps the bare name.
pub(super) fn jvm_bind_local_receivers(nodes: &[ParsedNode], edges: &mut [ParsedEdge]) {
    for edge in edges {
        let Some(owner) = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove(JVM_LOCAL_RECEIVER))
        else {
            continue;
        };
        let Some(owner) = owner.as_str() else {
            continue;
        };
        let nested = format!(".{owner}");
        let owners = nodes
            .iter()
            .filter(|node| {
                matches!(node.kind, crate::core::types::NodeKind::Function)
                    && node.name == edge.target
            })
            .filter_map(|node| node.parent_name.as_deref())
            .filter(|parent| *parent == owner || parent.ends_with(&nested))
            .collect::<Vec<_>>();
        // Same-file resolution takes the class itself, or a unique nested
        // class of that name.
        if owners.contains(&owner) || owners.len() == 1 {
            edge.target = format!("{owner}::{}", edge.target);
        }
    }
}

/// A method call's receiver, past calls of the same method: one line
/// holds a single edge per target, so in `b.with(1).with(2)` the edge of
/// `with` stands for both and its receiver is `b`.
fn java_call_receiver(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    method: &str,
) -> JvmReceiver {
    let mut object = node.child_by_field_name("object");
    while let Some(inner) = object.filter(|inner| {
        inner.kind() == "method_invocation"
            && java_call_name(*inner, context.source).as_deref() == Some(method)
    }) {
        object = inner.child_by_field_name("object");
    }
    match object {
        Some(object) => java_expression_receiver(object, node, context),
        None => JvmReceiver::Known,
    }
}

/// What the expression a method is called on is (see [`JvmReceiver`]);
/// variables are looked up from `at`.
fn java_expression_receiver(
    expression: tree_sitter::Node<'_>,
    at: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
) -> JvmReceiver {
    let source = context.source;
    match expression.kind() {
        "identifier" => {
            let name = node_text(expression, source);
            match java_variable_declaration(at, &name, source) {
                Some(declared) => java_declared_receiver(declared, context),
                // A class (`Repo.create()`, `INSTANCE.run()`) or a name a
                // static import binds.
                None if name.starts_with(|c: char| c.is_ascii_uppercase())
                    || context.stdlib.imported.contains_key(&name) =>
                {
                    JvmReceiver::Known
                }
                // A field the class inherits.
                None => JvmReceiver::Unknown(None),
            }
        }
        "field_access" => {
            let (Some(object), Some(field)) = (
                expression.child_by_field_name("object"),
                expression.child_by_field_name("field"),
            ) else {
                return JvmReceiver::Known;
            };
            let field = node_text(field, source);
            if object.kind() == "this" {
                return match java_field_declaration(at, &field, source) {
                    Some(declared) => java_declared_receiver(declared, context),
                    None => JvmReceiver::Unknown(None),
                };
            }
            // `com.acme.Util.run()` names a class; `a.b.run()` a field of a
            // variable.
            if field.starts_with(|c: char| c.is_ascii_uppercase())
                || java_dotted_name(expression, source).is_some_and(|segments| {
                    segments[0].starts_with(|c: char| c.is_ascii_uppercase())
                        || context.stdlib.imported.contains_key(&segments[0])
                })
            {
                return JvmReceiver::Known;
            }
            JvmReceiver::Unknown(None)
        }
        "method_invocation" => JvmReceiver::Unknown(java_call_origin(expression, source)),
        "object_creation_expression" | "cast_expression" => expression
            .child_by_field_name("type")
            .map_or(JvmReceiver::Known, |ty| java_type_receiver(ty, context)),
        "parenthesized_expression" => expression
            .named_child(0)
            .map_or(JvmReceiver::Known, |inner| {
                java_expression_receiver(inner, at, context)
            }),
        "array_access" | "ternary_expression" => JvmReceiver::Unknown(None),
        _ => JvmReceiver::Known,
    }
}

/// A field `name` of the class enclosing `at` (`this.repo`).
fn java_field_declaration<'a>(
    at: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<JavaDeclared<'a>> {
    let mut current = at;
    while let Some(scope) = current.parent() {
        if scope.kind() == "class_body" {
            let mut cursor = scope.walk();
            return scope
                .children(&mut cursor)
                .filter(|child| child.kind() == "field_declaration")
                .find_map(|child| java_declarator_type(child, name, source));
        }
        current = scope;
    }
    None
}

fn java_declared_receiver(
    declared: JavaDeclared<'_>,
    context: &JavaParseContext<'_>,
) -> JvmReceiver {
    match declared {
        JavaDeclared::Type(declared) => java_type_receiver(declared, context),
        JavaDeclared::Value(Some(value)) => match value.kind() {
            "object_creation_expression" | "cast_expression" => value
                .child_by_field_name("type")
                .map_or(JvmReceiver::Unknown(None), |ty| {
                    java_type_receiver(ty, context)
                }),
            // `var conn = store.connect()`: what `connect` returns.
            "method_invocation" => JvmReceiver::Unknown(java_call_origin(value, context.source)),
            _ => JvmReceiver::Unknown(None),
        },
        JavaDeclared::Value(None) => JvmReceiver::Unknown(None),
    }
}

/// A receiver of a declared type: nothing for the class library, a
/// primitive or an array; the class of this file or of another; a type
/// parameter (`T item`) says nothing.
fn java_type_receiver(
    declared: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
) -> JvmReceiver {
    let Some(path) = java_type_path(declared, context.source) else {
        return JvmReceiver::Known;
    };
    if context.stdlib.resolve_type_path(&path).is_some() {
        return JvmReceiver::Known;
    }
    let Some(class) = path.last() else {
        return JvmReceiver::Known;
    };
    if context.type_names.contains(class) {
        return JvmReceiver::Local(class.clone());
    }
    if context.stdlib.declared.contains(class) {
        return JvmReceiver::Unknown(None);
    }
    if class.starts_with(|c: char| c.is_ascii_uppercase()) {
        JvmReceiver::Foreign(class.clone())
    } else {
        JvmReceiver::Known
    }
}

/// The call a receiver is the result of (`store.connect()` ->
/// `connect` on its line).
fn java_call_origin(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<CallOrigin> {
    Some(CallOrigin {
        name: java_call_name(call, source)?,
        line: call.start_position().row as i64 + 1,
        unwrap: false,
    })
}
