use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::java::{JvmReceiver, jvm_bind_local_receivers, jvm_mark_receiver};
use super::member_calls::CallOrigin;
use super::stdlib::java::{
    is_java_lang_class, jvm_class_package, jvm_package_of, jvm_path_has_root,
};
use super::stdlib::scala::{SCALA_STDLIB_ROOTS, is_scala_predef_name, is_scala_subpackage};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, direct_child, direct_child_text, first_descendant_text,
    last_descendant_text, line_count, line_of, node_text, set_declared_namespaces,
    strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_scala_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "scala")];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let scope = ScalaStdlibScope::collect(tree.root_node(), source);
        scala_walk_children(
            tree.root_node(),
            source,
            &file_path,
            &scope,
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
                &["package_clause"],
                Some("name"),
                &["package_identifier"],
            ),
        );
        jvm_bind_local_receivers(&nodes, &mut edges);
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

#[allow(clippy::too_many_arguments)]
fn scala_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    scope: &ScalaStdlibScope,
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
                scala_emit_imports(child, source, file_path, edges);
            }
            "trait_definition" | "class_definition" | "object_definition" | "enum_definition"
            | "given_definition" => {
                if let Some(name) = direct_child_text(child, source, &["identifier"]) {
                    scala_emit_type(child, source, file_path, &name, owner(), nodes, edges);
                    let path = match owner() {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name.clone(),
                    };
                    scala_walk_children(
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
            "function_definition" | "function_declaration" => {
                if let Some(name) = direct_child_text(child, source, &["identifier"]) {
                    scala_emit_function(child, source, file_path, &name, owner(), nodes, edges);
                    scala_walk_children(
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
                scala_emit_call(
                    child,
                    source,
                    file_path,
                    scope,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "instance_expression" => {
                scala_emit_instance_call(
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
        scala_walk_children(
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

fn scala_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    for mut target in scala_import_targets(node, source) {
        // An import of the standard library targets its package:
        // `scala.collection.mutable` for `scala.collection.mutable.HashMap`,
        // `java.io` for `java.io._`.
        let mut extra = json!({});
        if jvm_path_has_root(&target, SCALA_STDLIB_ROOTS) {
            let package = match target.strip_suffix(".*") {
                Some(prefix) => jvm_package_of(prefix, true),
                None => jvm_package_of(&target, false),
            };
            if let Some(package) = package {
                mark_stdlib_edge(&mut target, &mut extra, &package, StdlibEvidence::Certain);
            }
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
}

fn scala_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let text = node_text(node, source);
    let import = text.trim().trim_start_matches("import").trim();
    if let (Some(open), Some(close)) = (import.find('{'), import.rfind('}')) {
        let prefix = import[..open].trim_end_matches('.').trim();
        return import[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .filter_map(|item| match item.split_once("=>") {
                // `W => _` hides a name; `W => V` renames it but imports W.
                Some((_, alias)) if alias.trim() == "_" => None,
                Some((original, _)) => Some(original.trim()),
                None => Some(item),
            })
            .map(|item| format!("{prefix}.{}", scala_normalize_import_selector(item)))
            .collect();
    }
    vec![scala_normalize_import_selector(import)]
}

fn scala_normalize_import_selector(value: &str) -> String {
    value
        .strip_suffix("._")
        .map(|prefix| format!("{prefix}.*"))
        .unwrap_or_else(|| value.to_string())
}

fn scala_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = match node.kind() {
        "trait_definition" => ("trait", true, true),
        "enum_definition" => ("enum", false, false),
        "object_definition" => ("object", false, false),
        "given_definition" => ("given", false, false),
        _ if scala_is_case_class(node, source) => ("record", false, false),
        _ => ("class", false, false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
        if scala_is_value_container(type_role) {
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
        language: "scala".to_string(),
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
    // A trait's supertypes are all contracts; for classes and objects the
    // first `extends` target is the superclass and `with` targets are mixins.
    let first_is_superclass = matches!(node.kind(), "class_definition" | "object_definition");
    {
        for (idx, target) in scala_inheritance_targets(node, source)
            .into_iter()
            .enumerate()
        {
            let is_superclass = idx == 0 && first_is_superclass;
            edges.push(ParsedEdge {
                kind: if is_superclass {
                    crate::core::types::EdgeKind::Inherits
                } else {
                    crate::core::types::EdgeKind::Implements
                },
                source: qualified.clone(),
                target,
                file_path: file_path.clone(),
                line: node.start_position().row as i64 + 1,
                extra: json!({
                    "relationship_role": if is_superclass { "extends" } else { "implements" },
                    "syntax_source": node.kind(),
                }),
            });
        }
    }
}

fn scala_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "record" | "enum")
}

fn scala_is_case_class(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .any(|child| node_text(child, source).trim() == "case")
}

fn scala_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "scala".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: direct_child_text(node, source, &["parameters"]),
        // As written (`Future[User]`): resolution across files types what a
        // call of this method returns by it.
        return_type: node
            .child_by_field_name("return_type")
            .map(|ty| node_text(ty, source).trim().to_string()),
        modifiers: None,
        is_test: false,
        extra: json!({}),
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

fn scala_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    scope: &ScalaStdlibScope,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = scala_caller(file_path, enclosing_class, enclosing_func);
    // `this(...)` in an auxiliary constructor delegates to the primary
    // constructor, not to the enclosing `this` definition.
    if let Some(call_name) = scala_call_name(node, source).filter(|name| name != "this") {
        let mut target = call_name;
        let mut extra = json!({});
        if let Some((package, evidence, symbol)) = scala_stdlib_call(node, source, scope, &target) {
            target = symbol;
            mark_stdlib_edge(&mut target, &mut extra, &package, evidence);
        } else {
            jvm_mark_receiver(
                scala_call_receiver(node, source, scope, &target),
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
    if let Some(signature) = scala_call_signature(node, source)
        && let Some(edge) = scala_bridge_edge(node, source, file_path, &caller, &signature)
    {
        edges.push(edge);
    }
}

fn scala_caller(
    file_path: &FilePath,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
) -> String {
    match (enclosing_func, enclosing_class) {
        (Some(func), _) => qualify(file_path, func, enclosing_class),
        (None, Some(class)) => qualify(file_path, class, None),
        (None, None) => file_path.to_string(),
    }
}

fn scala_emit_instance_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    scope: &ScalaStdlibScope,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(mut target) = first_descendant_text(node, source, &["type_identifier"]) else {
        return;
    };
    let caller = scala_caller(file_path, enclosing_class, enclosing_func);
    let mut extra = json!({});
    // `new ListBuffer[Int]()` constructs a class of the standard library.
    if let Some((package, evidence)) = scope.resolve_type(&target) {
        mark_stdlib_edge(&mut target, &mut extra, &package, evidence);
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
}

fn scala_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = scala_call_callee(node)?;
    if callee.kind() == "identifier" {
        return Some(node_text(callee, source));
    }
    if callee.kind() == "generic_function"
        && let Some(function) = direct_child(callee, &["field_expression", "identifier"])
    {
        return last_descendant_text(function, source, &["identifier", "type_identifier"])
            .or_else(|| Some(node_text(function, source)));
    }
    last_descendant_text(callee, source, &["identifier", "type_identifier"])
}

fn scala_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = scala_call_callee(node)?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn scala_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "arguments")
}

fn scala_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "scala.sys.process.Process" => {
            ("invokes_binary", "subprocess")
        }
        "System.loadLibrary" | "System.load" => ("loads_shared_library", "ffi"),
        "Files.readString" | "Files.readAllBytes" | "scala.io.Source.fromFile" => {
            ("reads_file", "file_io")
        }
        "Files.writeString" | "Files.write" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match scala_first_string_arg(node, source) {
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
            "source_language": "scala",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn scala_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = direct_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(strip_matching_quotes(node_text(child, source).trim()).to_string());
        }
        return None;
    }
    None
}

fn scala_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let extends = if node.kind() == "given_definition" {
        Some(node)
    } else {
        direct_child(node, &["extends_clause"])
    };
    let Some(extends) = extends else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = extends.walk();
    for child in extends.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => out.push(node_text(child, source)),
            "generic_type" => {
                if let Some(target) = first_descendant_text(child, source, &["type_identifier"]) {
                    out.push(target);
                }
            }
            _ => {}
        }
    }
    out
}

/// The names of a file that decide whether a call reaches the standard
/// library: what it declares, and what its imports bind.
#[derive(Default)]
struct ScalaStdlibScope {
    /// Types, objects, and functions this file declares: a `println` of its
    /// own is never `Predef`'s.
    declared: HashSet<String>,
    /// The name each import binds (its rename, or its last segment) to its
    /// path when that is in the standard library (`ListBuffer` ->
    /// `scala.collection.mutable.ListBuffer`, `mutable` ->
    /// `scala.collection.mutable`), or to `None` when it comes from
    /// elsewhere.
    imported: HashMap<String, Option<String>>,
    /// Packages of the standard library imported whole (`java.io._`).
    stdlib_wildcards: Vec<String>,
    /// A wildcard import from elsewhere may bring in a class named like
    /// one of `java.lang` or a whole-imported package.
    foreign_wildcard: bool,
    /// Classes, traits, objects, and enums this file declares: a receiver
    /// typed by one keeps the same-file binding, one typed by any other
    /// class is left to resolution across files.
    classes: HashSet<String>,
}

impl ScalaStdlibScope {
    fn collect(root: tree_sitter::Node<'_>, source: &[u8]) -> Self {
        let mut scope = Self::default();
        scope.collect_into(root, source);
        scope
    }

    fn collect_into(&mut self, node: tree_sitter::Node<'_>, source: &[u8]) {
        match node.kind() {
            "import_declaration" => {
                for (name, path) in scala_import_bindings(node, source) {
                    let stdlib = jvm_path_has_root(&path, SCALA_STDLIB_ROOTS);
                    match name {
                        Some(name) => {
                            self.imported.insert(name, stdlib.then_some(path));
                        }
                        None if stdlib => self.stdlib_wildcards.push(path),
                        None => self.foreign_wildcard = true,
                    }
                }
                return;
            }
            "trait_definition"
            | "class_definition"
            | "object_definition"
            | "enum_definition"
            | "given_definition"
            | "function_definition"
            | "function_declaration"
            | "type_definition" => {
                let name = direct_child_text(node, source, &["identifier", "type_identifier"]);
                if matches!(
                    node.kind(),
                    "trait_definition"
                        | "class_definition"
                        | "object_definition"
                        | "enum_definition"
                ) {
                    self.classes.extend(name.clone());
                }
                self.declared.extend(name);
            }
            // `T` of `def f[T](x: T)`: no class of the library.
            "type_parameters" => {
                let mut cursor = node.walk();
                self.declared.extend(
                    node.children_by_field_name("name", &mut cursor)
                        .map(|name| node_text(name, source)),
                );
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_into(child, source);
        }
    }

    /// The package of a class named bare (`ListBuffer`, `System`, `List`),
    /// and how sure that is: an import of it or of its package, or a class
    /// of `java.lang` (in scope in every Scala file); only likely for the
    /// names `scala._` and `Predef` bring in (`List`, `Some`).
    fn resolve_type(&self, name: &str) -> Option<(String, StdlibEvidence)> {
        if self.declared.contains(name) {
            return None;
        }
        if let Some(imported) = self.imported.get(name) {
            let package = jvm_package_of(imported.as_deref()?, false)?;
            return Some((package, StdlibEvidence::Certain));
        }
        if is_scala_predef_name(name) {
            return Some(("scala".to_string(), StdlibEvidence::Likely));
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

/// What an import binds: `(Some(name), path)` for each name (renamed ones
/// by their new name), `(None, package)` for a wildcard (`java.io._`,
/// `{given, *}`). `W => _` hides a name and binds nothing.
fn scala_import_bindings(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<(Option<String>, String)> {
    let text = node_text(node, source);
    let import = text.trim().trim_start_matches("import").trim();
    let (prefix, selectors) = match (import.find('{'), import.rfind('}')) {
        (Some(open), Some(close)) if open < close => (
            import[..open].trim_end_matches('.').trim(),
            import[open + 1..close].split(',').collect::<Vec<_>>(),
        ),
        _ => match import.rsplit_once('.') {
            Some((prefix, last)) => (prefix.trim(), vec![last]),
            None => return Vec::new(),
        },
    };
    selectors
        .into_iter()
        .filter_map(|selector| {
            let selector = selector.trim();
            let (original, bound) = match selector
                .split_once("=>")
                .or_else(|| selector.split_once(" as "))
            {
                Some((original, bound)) => (original.trim(), bound.trim()),
                None => (selector, selector),
            };
            match original {
                "" | "given" => None,
                "_" | "*" => Some((None, prefix.to_string())),
                _ if bound == "_" => None,
                _ => Some((Some(bound.to_string()), format!("{prefix}.{original}"))),
            }
        })
        .collect()
}

/// The standard-library package a call reaches, how sure that is, and the
/// call as written or resolved (`println`, `Math.max`,
/// `ListBuffer.append`):
///
/// - a name an import binds (`ListBuffer()`, `mutable.Map.empty` after
///   `import scala.collection.mutable`), a class of `java.lang`
///   (`System.currentTimeMillis`), or a path through the library
///   (`scala.math.max`, `java.nio.file.Paths.get`);
/// - a member of a value it just constructed (`new File(p).exists()`);
/// - only likely: a `Predef` name (`println`, `List(1, 2)`), a subpackage
///   of `scala` by its last name (`math.max`), or a method of a variable
///   typed by its declaration or initializer (`xs: List[Int]; xs.map(f)`).
fn scala_stdlib_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    scope: &ScalaStdlibScope,
    name: &str,
) -> Option<(String, StdlibEvidence, String)> {
    let mut callee = scala_call_callee(node)?;
    if callee.kind() == "generic_function" {
        callee = callee.child_by_field_name("function")?;
    }
    match callee.kind() {
        "identifier" => {
            if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                let (package, evidence) = scope.resolve_type(name)?;
                return Some((package, evidence, name.to_string()));
            }
            if scope.declared.contains(name) || scala_variable_type(node, name, source).is_some() {
                return None;
            }
            if let Some(imported) = scope.imported.get(name) {
                let package = jvm_package_of(imported.as_deref()?, false)?;
                return Some((package, StdlibEvidence::Certain, name.to_string()));
            }
            is_scala_predef_name(name).then(|| {
                (
                    "scala".to_string(),
                    StdlibEvidence::Likely,
                    name.to_string(),
                )
            })
        }
        "field_expression" => {
            let value = callee.child_by_field_name("value")?;
            let written = || format!("{}.{name}", node_text(value, source).trim());
            match value.kind() {
                "string" | "interpolated_string_expression" => Some((
                    "java.lang".to_string(),
                    StdlibEvidence::Certain,
                    format!("String.{name}"),
                )),
                "call_expression" | "instance_expression" => {
                    let class = scala_constructed_class(value, source)?;
                    let (package, evidence) = scope.resolve_type(&class)?;
                    Some((package, evidence, format!("{class}.{name}")))
                }
                "identifier" | "field_expression" => {
                    let segments = scala_dotted_name(value, source)?;
                    let first = segments.first()?;
                    if first.starts_with(|c: char| c.is_ascii_uppercase()) {
                        let (package, evidence) = scope.resolve_type(first)?;
                        return Some((package, evidence, written()));
                    }
                    if let Some(imported) = scope.imported.get(first) {
                        let mut path = vec![imported.clone()?];
                        path.extend(segments[1..].iter().cloned());
                        path.push(name.to_string());
                        let package = jvm_package_of(&path.join("."), false)?;
                        return Some((package, StdlibEvidence::Certain, written()));
                    }
                    if let Some(variable) = scala_variable_type(node, first, source) {
                        if segments.len() > 1 {
                            return None;
                        }
                        let class = variable?;
                        let (package, _) = scope.resolve_type(&class)?;
                        return Some((package, StdlibEvidence::Likely, format!("{class}.{name}")));
                    }
                    if scope.declared.contains(first) {
                        return None;
                    }
                    if SCALA_STDLIB_ROOTS.contains(&first.as_str()) {
                        let path = written();
                        let package = jvm_package_of(&path, false)?;
                        return Some((package, StdlibEvidence::Certain, path));
                    }
                    if is_scala_subpackage(first) {
                        let package = jvm_package_of(&format!("scala.{}", written()), false)?;
                        return Some((package, StdlibEvidence::Likely, written()));
                    }
                    None
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The class a `new C(...)` or `C(...)` expression constructs.
fn scala_constructed_class(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let class = match node.kind() {
        "instance_expression" => first_descendant_text(node, source, &["type_identifier"])?,
        _ => {
            let mut callee = scala_call_callee(node)?;
            if callee.kind() == "generic_function" {
                callee = callee.child_by_field_name("function")?;
            }
            (callee.kind() == "identifier").then(|| node_text(callee, source))?
        }
    };
    class
        .starts_with(|c: char| c.is_ascii_uppercase())
        .then_some(class)
}

/// `a.b.c` as its segments, when it is only names.
fn scala_dotted_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" => Some(vec![node_text(node, source)]),
        "field_expression" => {
            let mut segments = scala_dotted_name(node.child_by_field_name("value")?, source)?;
            segments.push(node_text(node.child_by_field_name("field")?, source));
            Some(segments)
        }
        _ => None,
    }
}

/// The class of the variable `name` visible at `node`: a parameter or a
/// `val` / `var` defined before it, typed by its annotation (`xs:
/// List[Int]`) or by the class it constructs (`val b = new ListBuffer[Int]`).
/// `Some(None)` when the nearest definition gives no class to go by (`val n
/// = f()`, a lambda parameter), so an outer one of the same name does not
/// count.
fn scala_variable_type(
    node: tree_sitter::Node<'_>,
    name: &str,
    source: &[u8],
) -> Option<Option<String>> {
    scala_variable_declaration(node, name, source).map(|declared| match declared {
        ScalaDeclared::Type(class) => Some(class),
        ScalaDeclared::Value(value) => value
            .filter(|value| matches!(value.kind(), "call_expression" | "instance_expression"))
            .and_then(|value| scala_constructed_class(value, source)),
    })
}

/// What the nearest definition of a variable says about it: the class of
/// its annotation (`repo: Repo`), or else the value it is initialized with,
/// if any (`val conn = store.connect()`).
enum ScalaDeclared<'a> {
    Type(String),
    Value(Option<tree_sitter::Node<'a>>),
}

/// The nearest definition of the variable `name` visible at `node` (see
/// [`scala_variable_type`]). A member of a class counts wherever it is
/// defined in the class body.
fn scala_variable_declaration<'a>(
    node: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<ScalaDeclared<'a>> {
    let names_it = |definition: tree_sitter::Node<'_>, field: &str| {
        definition
            .child_by_field_name(field)
            .is_some_and(|var| node_text(var, source).trim() == name)
    };
    let annotated = |definition: tree_sitter::Node<'_>| {
        definition
            .child_by_field_name("type")
            .and_then(|ty| match ty.kind() {
                "type_identifier" => Some(node_text(ty, source)),
                _ => first_descendant_text(ty, source, &["type_identifier"]),
            })
    };
    let mut current = node;
    while let Some(scope) = current.parent() {
        if scope.kind() == "lambda_expression"
            && scope
                .child_by_field_name("parameters")
                .is_some_and(|params| {
                    node_text(params, source)
                        .split([',', ':', '(', ')'])
                        .any(|part| part.trim() == name)
                })
        {
            return Some(ScalaDeclared::Value(None));
        }
        let mut cursor = scope.walk();
        for child in scope.children(&mut cursor) {
            let found = match child.kind() {
                "val_definition" | "var_definition"
                    if (child.start_byte() < node.start_byte()
                        || scope.kind() == "template_body")
                        && names_it(child, "pattern") =>
                {
                    Some(match annotated(child) {
                        Some(class) => ScalaDeclared::Type(class),
                        None => ScalaDeclared::Value(child.child_by_field_name("value")),
                    })
                }
                "parameters" | "class_parameters" => {
                    let mut inner = child.walk();
                    child
                        .children(&mut inner)
                        .find(|param| names_it(*param, "name"))
                        .map(|param| match annotated(param) {
                            Some(class) => ScalaDeclared::Type(class),
                            None => ScalaDeclared::Value(None),
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

/// The member `name` of the class enclosing `at` (`this.repo`): one of its
/// body or of its class parameters.
fn scala_member_declaration<'a>(
    at: tree_sitter::Node<'a>,
    name: &str,
    source: &[u8],
) -> Option<ScalaDeclared<'a>> {
    let mut current = at;
    while let Some(scope) = current.parent() {
        if scope.kind() == "template_body" {
            let mut cursor = scope.walk();
            let first = scope.children(&mut cursor).next()?;
            return scala_variable_declaration(first, name, source);
        }
        current = scope;
    }
    None
}

/// A member call's receiver, past calls of the same method: one line
/// holds a single edge per target, so in `b.with(1).with(2)` the edge of
/// `with` stands for both and its receiver is `b`.
fn scala_call_receiver(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    scope: &ScalaStdlibScope,
    method: &str,
) -> JvmReceiver {
    let mut receiver = scala_member_receiver(node);
    while let Some(inner) = receiver.filter(|inner| {
        inner.kind() == "call_expression"
            && scala_member_receiver(*inner).is_some()
            && scala_call_name(*inner, source).as_deref() == Some(method)
    }) {
        receiver = scala_member_receiver(inner);
    }
    match receiver {
        Some(receiver) => scala_expression_receiver(receiver, node, source, scope),
        None => JvmReceiver::Known,
    }
}

/// `repo` of `repo.save()` / `repo.find[User](id)`; none for a bare call.
fn scala_member_receiver(call: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut callee = scala_call_callee(call)?;
    if callee.kind() == "generic_function" {
        callee = callee.child_by_field_name("function")?;
    }
    (callee.kind() == "field_expression")
        .then(|| callee.child_by_field_name("value"))
        .flatten()
}

/// What the expression a method is called on is (see [`JvmReceiver`]);
/// variables are looked up from `at`.
fn scala_expression_receiver(
    expression: tree_sitter::Node<'_>,
    at: tree_sitter::Node<'_>,
    source: &[u8],
    scope: &ScalaStdlibScope,
) -> JvmReceiver {
    match expression.kind() {
        "identifier" => {
            let name = node_text(expression, source);
            if matches!(name.as_str(), "this" | "super") {
                return JvmReceiver::Known;
            }
            match scala_variable_declaration(at, &name, source) {
                Some(declared) => scala_declared_receiver(declared, source, scope),
                // An object or class (`Repo.create()`), or a name an import
                // binds.
                None if name.starts_with(|c: char| c.is_ascii_uppercase())
                    || scope.imported.contains_key(&name) =>
                {
                    JvmReceiver::Known
                }
                // A member the class inherits.
                None => JvmReceiver::Unknown(None),
            }
        }
        "field_expression" => {
            let Some(segments) = scala_dotted_name(expression, source) else {
                return JvmReceiver::Unknown(None);
            };
            if segments.len() == 2 && segments[0] == "this" {
                return scala_member_declaration(at, &segments[1], source)
                    .map_or(JvmReceiver::Unknown(None), |declared| {
                        scala_declared_receiver(declared, source, scope)
                    });
            }
            // `a.b.run()` on a variable is of a type unknown; a path to an
            // object or through a package (`com.acme.Util.run()`) is not.
            if scala_variable_declaration(at, &segments[0], source).is_some()
                && segments
                    .last()
                    .is_some_and(|last| !last.starts_with(|c: char| c.is_ascii_uppercase()))
            {
                JvmReceiver::Unknown(None)
            } else {
                JvmReceiver::Known
            }
        }
        "call_expression" | "instance_expression" => {
            match scala_constructed_class(expression, source) {
                Some(class) => scala_class_receiver(&class, scope),
                None if expression.kind() == "call_expression" => {
                    JvmReceiver::Unknown(scala_call_origin(expression, source))
                }
                None => JvmReceiver::Known,
            }
        }
        "parenthesized_expression" => expression
            .named_child(0)
            .map_or(JvmReceiver::Known, |inner| {
                scala_expression_receiver(inner, at, source, scope)
            }),
        "wildcard" | "if_expression" | "match_expression" => JvmReceiver::Unknown(None),
        _ => JvmReceiver::Known,
    }
}

fn scala_declared_receiver(
    declared: ScalaDeclared<'_>,
    source: &[u8],
    scope: &ScalaStdlibScope,
) -> JvmReceiver {
    match declared {
        ScalaDeclared::Type(class) => scala_class_receiver(&class, scope),
        ScalaDeclared::Value(Some(value))
            if matches!(value.kind(), "call_expression" | "instance_expression") =>
        {
            match scala_constructed_class(value, source) {
                Some(class) => scala_class_receiver(&class, scope),
                // `val conn = store.connect()`: what `connect` returns.
                None => JvmReceiver::Unknown(scala_call_origin(value, source)),
            }
        }
        ScalaDeclared::Value(_) => JvmReceiver::Unknown(None),
    }
}

/// A receiver of class `class`: nothing for the standard library; the
/// class of this file or of another; a type parameter says nothing.
fn scala_class_receiver(class: &str, scope: &ScalaStdlibScope) -> JvmReceiver {
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
fn scala_call_origin(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<CallOrigin> {
    Some(CallOrigin {
        name: scala_call_name(call, source)?,
        line: call.start_position().row as i64 + 1,
        unwrap: false,
    })
}
