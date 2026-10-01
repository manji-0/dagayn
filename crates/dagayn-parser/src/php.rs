use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::member_calls::{CallOrigin, MemberCallBindings};
use super::stdlib::php::{is_php_builtin_class, is_php_builtin_function};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    collect_namespace_paths, is_test_file, line_count, node_text, set_declared_namespaces,
    strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_php_with_parser(
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
        language: "php".to_string(),
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
        let stdlib = PhpStdlibScope::collect(tree.root_node(), source);
        let mut receivers = PhpReceivers::collect(tree.root_node(), source);
        php_walk_children(
            tree.root_node(),
            source,
            &file_path,
            &stdlib,
            &mut receivers,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        php_apply_use_aliases(tree.root_node(), source, &mut edges);
        set_declared_namespaces(
            &mut nodes,
            collect_namespace_paths(
                tree.root_node(),
                source,
                &["namespace_definition"],
                Some("name"),
                &["namespace_name"],
            ),
        );
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

#[allow(clippy::too_many_arguments)]
fn php_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    stdlib: &PhpStdlibScope,
    receivers: &mut PhpReceivers,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "namespace_use_declaration" => {
                php_emit_import(child, source, file_path, edges);
            }
            "class_declaration"
            | "interface_declaration"
            | "trait_declaration"
            | "enum_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    php_walk_children(
                        child,
                        source,
                        file_path,
                        stdlib,
                        receivers,
                        Some(&name),
                        None,
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "function_definition" | "method_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    // The parameters and locals of the function end with it.
                    let snapshot = receivers.bindings.snapshot();
                    receivers.bind_parameters(child, source);
                    php_walk_children(
                        child,
                        source,
                        file_path,
                        stdlib,
                        receivers,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    receivers.bindings.restore(snapshot);
                    continue;
                }
            }
            "object_creation_expression" => {
                if let Some(class_node) = php_direct_child(child, &["name", "qualified_name"]) {
                    let mut target = php_simple_name(&node_text(class_node, source));
                    let caller = match (enclosing_func, enclosing_class) {
                        (Some(func), _) => qualify(file_path, func, enclosing_class),
                        (None, Some(class)) => qualify(file_path, class, None),
                        (None, None) => file_path.to_string(),
                    };
                    let mut extra = json!({"call_role": "instantiation"});
                    if let Some((class, evidence)) =
                        stdlib.builtin_class(&node_text(class_node, source))
                    {
                        target = class;
                        mark_stdlib_edge(&mut target, &mut extra, "php", evidence);
                    }
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Calls,
                        source: caller,
                        target,
                        file_path: file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra,
                    });
                }
            }
            "function_call_expression"
            | "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression" => {
                php_emit_call(
                    child,
                    source,
                    file_path,
                    stdlib,
                    receivers,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        php_walk_children(
            child,
            source,
            file_path,
            stdlib,
            receivers,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        // Bound once the right-hand side's own calls are walked.
        if child.kind() == "assignment_expression" {
            receivers.bind_assignment(child, source);
        }
    }
}

fn php_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    let function_use = php_use_kind(node).or_else(|| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == "namespace_use_clause")
            .and_then(php_use_kind)
    }) == Some("function");
    for mut target in php_import_targets(node, source) {
        let mut extra = json!({});
        // `use DateTime;` / `use function strlen;`: a global builtin.
        let builtin = if function_use {
            is_php_builtin_function(&target)
        } else {
            is_php_builtin_class(&target)
        };
        if !target.contains('\\') && builtin {
            mark_stdlib_edge(&mut target, &mut extra, "php", StdlibEvidence::Certain);
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

/// The imported symbol paths of a `use` declaration.
///
/// The whole statement used to be the target (`use Exception;`), which no
/// namespace or file index could ever match. Group form
/// (`use App\Util\{One, Two};`) expands to one target per clause, and an
/// `as` alias is dropped.
fn php_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let prefix = php_direct_child_text(node, source, &["namespace_name"]);
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "namespace_use_clause" => {
                if let Some(target) = php_use_clause_target(child, source, prefix.as_deref()) {
                    targets.push(target);
                }
            }
            "namespace_use_group" => {
                let mut group = child.walk();
                for clause in child.children(&mut group) {
                    if clause.kind() != "namespace_use_clause" {
                        continue;
                    }
                    if let Some(target) = php_use_clause_target(clause, source, prefix.as_deref()) {
                        targets.push(target);
                    }
                }
            }
            _ => {}
        }
    }
    targets
}

fn php_use_clause_target(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    prefix: Option<&str>,
) -> Option<String> {
    let path = php_direct_child_text(node, source, &["qualified_name", "name"])?;
    let path = path.trim().trim_start_matches('\\');
    if path.is_empty() {
        return None;
    }
    Some(match prefix {
        Some(prefix) => format!("{}\\{path}", prefix.trim().trim_start_matches('\\')),
        None => path.to_string(),
    })
}

fn php_simple_name(text: &str) -> String {
    text.trim()
        .rsplit('\\')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn php_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

/// Names listed directly under `clause` (`base_clause`, `class_interface_clause`
/// or a trait `use_declaration`), reduced to their unqualified class name.
fn php_clause_names(clause: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = clause.walk();
    clause
        .children(&mut cursor)
        .filter(|child| matches!(child.kind(), "name" | "qualified_name"))
        .map(|child| php_simple_name(&node_text(child, source)))
        .filter(|name| !name.is_empty())
        .collect()
}

/// Rewrites type and instantiation targets written through a `use ... as`
/// alias (`new P()` after `use App\Post as P`) to the imported class name.
fn php_apply_use_aliases(root: tree_sitter::Node<'_>, source: &[u8], edges: &mut [ParsedEdge]) {
    let mut aliases = std::collections::HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "namespace_use_clause" {
            let mut cursor = node.walk();
            let parts: Vec<_> = node
                .children(&mut cursor)
                .filter(|child| matches!(child.kind(), "name" | "qualified_name"))
                .collect();
            if let [original, alias] = parts.as_slice() {
                aliases.insert(
                    node_text(*alias, source).trim().to_string(),
                    php_simple_name(&node_text(*original, source)),
                );
            }
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    if aliases.is_empty() {
        return;
    }
    for edge in edges.iter_mut() {
        let rewritable = matches!(
            edge.kind,
            crate::core::types::EdgeKind::Inherits | crate::core::types::EdgeKind::Implements
        ) || edge.extra.get("call_role").and_then(|v| v.as_str())
            == Some("instantiation");
        if rewritable && let Some(original) = aliases.get(&edge.target) {
            edge.target = original.clone();
        }
    }
}

fn php_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = match node.kind() {
        "interface_declaration" => ("interface", true, true),
        "trait_declaration" => ("trait", false, false),
        "enum_declaration" => ("enum", false, false),
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
    }
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    let qualified = qualify(file_path, name, enclosing_class);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    let mut relations = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "base_clause" => relations.push((child, "extends")),
            "class_interface_clause" => relations.push((child, "implements")),
            "declaration_list" => {
                let mut members = child.walk();
                for member in child.children(&mut members) {
                    if member.kind() == "use_declaration" {
                        relations.push((member, "uses_trait"));
                    }
                }
            }
            _ => {}
        }
    }
    for (clause, role) in relations {
        for target in php_clause_names(clause, source) {
            edges.push(ParsedEdge {
                kind: if role == "implements" {
                    crate::core::types::EdgeKind::Implements
                } else {
                    crate::core::types::EdgeKind::Inherits
                },
                source: qualified.clone(),
                target,
                file_path: file_path.clone(),
                line: clause.start_position().row as i64 + 1,
                extra: json!({"relationship_role": role, "syntax_source": clause.kind()}),
            });
        }
    }
}

fn php_emit_function(
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
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: php_direct_child_text(node, source, &["formal_parameters"]),
        // As written (`?Store`, `Store`, `static`), for resolution across
        // files to type what a call of it returns.
        return_type: node
            .child_by_field_name("return_type")
            .map(|return_type| node_text(return_type, source).trim().to_string()),
        modifiers: None,
        is_test: false,
        extra: json!({}),
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

#[allow(clippy::too_many_arguments)]
fn php_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    stdlib: &PhpStdlibScope,
    receivers: &PhpReceivers,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(signature) = php_call_signature(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = php_call_name(node, source) {
        let mut target = call_name;
        let mut extra = json!({});
        if let Some((symbol, evidence)) = stdlib.builtin_call(node, source) {
            target = symbol;
            mark_stdlib_edge(&mut target, &mut extra, "php", evidence);
        } else {
            receivers.mark_receiver(node, source, enclosing_class, &target, &mut extra);
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
    if let Some(edge) = php_bridge_edge(node, source, file_path, &caller, &signature) {
        edges.push(edge);
    }
}

/// The invoked symbol, used as the CALLS target.
///
/// `Broker::build()` resolves to `build`. Emitting `Broker::build` made the
/// target look already-qualified to bare-name resolution, which only ever
/// matches `file::Class.method`, so the edge could never bind to a node.
fn php_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "scoped_call_expression" => php_direct_child_texts(node, source, &["name"]).pop(),
        _ => php_call_signature(node, source),
    }
}

/// The call as written, used to match cross-artifact bridges (`FFI::cdef`).
fn php_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_call_expression" => {
            php_direct_child_text(node, source, &["name", "qualified_name"])
                .map(|name| name.trim_start_matches('\\').to_string())
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            php_last_direct_child_text(node, source, "name")
        }
        "scoped_call_expression" => {
            let names = php_direct_child_texts(node, source, &["name"]);
            if names.len() >= 2 {
                return Some(format!("{}::{}", names[0], names[1]));
            }
            if let Some(scope) = php_direct_child_text(node, source, &["relative_scope"])
                && matches!(scope.as_str(), "parent" | "self")
            {
                return names.last().cloned();
            }
            names.last().cloned()
        }
        _ => None,
    }
}

fn php_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "exec" | "shell_exec" | "system" | "passthru" | "proc_open" | "popen" => {
            ("invokes_binary", "subprocess")
        }
        "file_get_contents" | "fread" | "readfile" => ("reads_file", "file_io"),
        "file_put_contents" | "fwrite" => ("writes_file", "file_io"),
        "fopen" => ("opens_file", "file_io"),
        "FFI::cdef" | "FFI::load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match php_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
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
            "source_language": "php",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn php_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "argument" {
            php_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if matches!(arg.kind(), "encapsed_string" | "string") {
            return Some(php_string_text(arg, source));
        }
        return None;
    }
    None
}

fn php_first_non_punctuation_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"))
}

fn php_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn php_direct_child_text(
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

fn php_last_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            found = Some(node_text(child, source));
        }
    }
    found
}

fn php_direct_child_texts(
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

/// `function` / `const` for `use function ...;` / `use const ...;`, `None`
/// for a class import. The keyword sits on the declaration in the group
/// form (`use function A\{b, c};`) and on the clause otherwise.
fn php_use_kind(node: tree_sitter::Node<'_>) -> Option<&'static str> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find_map(|child| match child.kind() {
            "function" => Some("function"),
            "const" => Some("const"),
            _ => None,
        })
}

/// What a PHP file declares and imports, read before the walk so each call
/// into the standard library is marked where it is emitted.
///
/// Name resolution follows PHP: an unqualified class name belongs to the
/// current namespace (only a non-namespaced file reaches the global
/// `DateTime` that way), while an unqualified function call falls back to
/// the global function when the namespace has none of that name.
#[derive(Default)]
struct PhpStdlibScope {
    namespaced: bool,
    /// Class imports by alias, lower-case (`dt` -> `DateTime` for
    /// `use DateTime as DT;`).
    class_uses: HashMap<String, String>,
    /// Aliases of `use function` imports, lower-case.
    function_uses: HashSet<String>,
    /// Functions and classes the file declares, lower-case.
    defined_functions: HashSet<String>,
    defined_classes: HashSet<String>,
    /// Variables by name (`$d`) with the builtin class they hold, from
    /// `$d = new \DateTime()` or a typed parameter `\DateTime $d`; `None`
    /// when another assignment or declaration gives them something else.
    vars: HashMap<String, Option<String>>,
}

impl PhpStdlibScope {
    fn collect(root: tree_sitter::Node<'_>, source: &[u8]) -> Self {
        let mut scope = Self::default();
        let mut bindings = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "namespace_definition" => {
                    scope.namespaced |= node.child_by_field_name("name").is_some();
                }
                "namespace_use_declaration" => scope.collect_use(node, source),
                "function_definition" => {
                    if let Some(name) = php_direct_child_text(node, source, &["name"]) {
                        scope.defined_functions.insert(name.to_ascii_lowercase());
                    }
                }
                "class_declaration"
                | "interface_declaration"
                | "trait_declaration"
                | "enum_declaration" => {
                    if let Some(name) = php_direct_child_text(node, source, &["name"]) {
                        scope.defined_classes.insert(name.to_ascii_lowercase());
                    }
                }
                "assignment_expression" | "simple_parameter" | "property_promotion_parameter" => {
                    bindings.push(node);
                }
                _ => {}
            }
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
        // Bound after the whole file is read: the imports and declarations
        // decide what a class name means.
        for node in bindings {
            let (variable, class) = if node.kind() == "assignment_expression" {
                let Some(left) = node
                    .child_by_field_name("left")
                    .filter(|left| left.kind() == "variable_name")
                else {
                    continue;
                };
                let class = node
                    .child_by_field_name("right")
                    .filter(|right| right.kind() == "object_creation_expression")
                    .and_then(|right| php_direct_child(right, &["name", "qualified_name"]));
                (left, class)
            } else {
                let Some(name) = node.child_by_field_name("name") else {
                    continue;
                };
                let class = node
                    .child_by_field_name("type")
                    .filter(|ty| ty.kind() == "named_type")
                    .and_then(|ty| php_direct_child(ty, &["name", "qualified_name"]));
                (name, class)
            };
            let class = class
                .and_then(|class| scope.builtin_class(&node_text(class, source)))
                .map(|(class, _)| class);
            let variable = node_text(variable, source).trim().to_string();
            match scope.vars.get_mut(&variable) {
                Some(existing) if *existing != class => *existing = None,
                Some(_) => {}
                None => {
                    scope.vars.insert(variable, class);
                }
            }
        }
        scope
    }

    fn collect_use(&mut self, node: tree_sitter::Node<'_>, source: &[u8]) {
        let kind = php_use_kind(node);
        let prefix = php_direct_child_text(node, source, &["namespace_name"]);
        let mut clauses = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "namespace_use_clause" => clauses.push(child),
                "namespace_use_group" => {
                    let mut group = child.walk();
                    clauses.extend(
                        child
                            .children(&mut group)
                            .filter(|clause| clause.kind() == "namespace_use_clause"),
                    );
                }
                _ => {}
            }
        }
        for clause in clauses {
            let Some(path) = php_use_clause_target(clause, source, prefix.as_deref()) else {
                continue;
            };
            let alias = clause
                .child_by_field_name("alias")
                .map(|alias| node_text(alias, source).trim().to_string())
                .unwrap_or_else(|| php_simple_name(&path))
                .to_ascii_lowercase();
            match php_use_kind(clause).or(kind) {
                Some("function") => {
                    self.function_uses.insert(alias);
                }
                Some(_) => {}
                None => {
                    self.class_uses.insert(alias, path);
                }
            }
        }
    }

    /// The builtin class a class name written in this file resolves to:
    /// certain for `\DateTime` or an imported `DateTime`, likely for a bare
    /// `DateTime` in a file without a namespace; nothing in a namespaced
    /// file, where the bare name is the namespace's own class.
    fn builtin_class(&self, written: &str) -> Option<(String, StdlibEvidence)> {
        let written = written.trim();
        if let Some(global) = written.strip_prefix('\\') {
            return (!global.contains('\\') && is_php_builtin_class(global))
                .then(|| (global.to_string(), StdlibEvidence::Certain));
        }
        let (first, rest) = match written.split_once('\\') {
            Some((first, rest)) => (first, Some(rest)),
            None => (written, None),
        };
        if let Some(path) = self.class_uses.get(&first.to_ascii_lowercase()) {
            return (rest.is_none() && !path.contains('\\') && is_php_builtin_class(path))
                .then(|| (path.clone(), StdlibEvidence::Certain));
        }
        if rest.is_some()
            || self.namespaced
            || self.defined_classes.contains(&written.to_ascii_lowercase())
            || !is_php_builtin_class(written)
        {
            return None;
        }
        Some((written.to_string(), StdlibEvidence::Likely))
    }

    /// The builtin a call reaches, as `strlen` or `DateTime::format`:
    ///
    /// * `\strlen($s)` certain, `strlen($s)` likely (a namespace function of
    ///   that name would win), never when the file declares or imports a
    ///   function of that name.
    /// * `\DateTime::createFromFormat(...)` / `(new \DateTime())->format()`
    ///   as certain as the class name is.
    /// * `$d->format()` on a variable bound to a builtin class, likely.
    fn builtin_call(
        &self,
        node: tree_sitter::Node<'_>,
        source: &[u8],
    ) -> Option<(String, StdlibEvidence)> {
        match node.kind() {
            "function_call_expression" => {
                let function = node.child_by_field_name("function")?;
                let written = node_text(function, source);
                let written = written.trim();
                match function.kind() {
                    "name" => {
                        let lower = written.to_ascii_lowercase();
                        (!self.function_uses.contains(&lower)
                            && !self.defined_functions.contains(&lower)
                            && is_php_builtin_function(written))
                        .then(|| (written.to_string(), StdlibEvidence::Likely))
                    }
                    "qualified_name" => {
                        let global = written.strip_prefix('\\')?;
                        (!global.contains('\\') && is_php_builtin_function(global))
                            .then(|| (global.to_string(), StdlibEvidence::Certain))
                    }
                    _ => None,
                }
            }
            "scoped_call_expression" => {
                let scope = node.child_by_field_name("scope")?;
                if !matches!(scope.kind(), "name" | "qualified_name") {
                    return None;
                }
                let method = node_text(node.child_by_field_name("name")?, source);
                let (class, evidence) = self.builtin_class(&node_text(scope, source))?;
                Some((format!("{class}::{}", method.trim()), evidence))
            }
            "member_call_expression" | "nullsafe_member_call_expression" => {
                let method = node_text(node.child_by_field_name("name")?, source);
                let mut object = node.child_by_field_name("object")?;
                while object.kind() == "parenthesized_expression" {
                    object = object.named_child(0)?;
                }
                let (class, evidence) = match object.kind() {
                    "object_creation_expression" => self.builtin_class(&node_text(
                        php_direct_child(object, &["name", "qualified_name"])?,
                        source,
                    ))?,
                    "variable_name" => {
                        let variable = node_text(object, source);
                        let class = self.vars.get(variable.trim())?.clone()?;
                        (class, StdlibEvidence::Likely)
                    }
                    _ => return None,
                };
                Some((format!("{class}::{}", method.trim()), evidence))
            }
            _ => None,
        }
    }
}

/// The declared types of the receivers of member calls, for resolution
/// across files.
///
/// A variable (`$repo`, as written) is bound by a typed parameter (`Repo
/// $repo`, `private Repo $repo` promoted in a constructor), by `$repo = new
/// Repo()`, or to the call it was assigned from (`$s = makeStore();`). A
/// typed property (`private Repo $repo;`) types `$this->repo`.
struct PhpReceivers {
    bindings: MemberCallBindings,
    /// Classes, interfaces, traits and enums the file declares.
    classes: HashSet<String>,
    /// Class-typed properties by class: `Svc` -> `repo` -> `Repo`.
    properties: HashMap<String, HashMap<String, String>>,
}

/// What a PHP member call's receiver is.
enum PhpReceiver {
    /// `$this`, or a class of this file: the same-file binding by name
    /// stands.
    Known,
    /// A class of another file.
    Foreign(String),
    /// Of unknown type, maybe the result of a call.
    Unknown(Option<CallOrigin>),
}

impl PhpReceivers {
    fn collect(root: tree_sitter::Node<'_>, source: &[u8]) -> Self {
        let mut classes = HashSet::new();
        let mut properties = HashMap::<String, HashMap<String, String>>::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if matches!(
                node.kind(),
                "class_declaration"
                    | "interface_declaration"
                    | "trait_declaration"
                    | "enum_declaration"
            ) && let Some(name) = php_direct_child_text(node, source, &["name"])
            {
                let typed = php_typed_properties(node, source);
                if !typed.is_empty() {
                    properties.entry(name.clone()).or_default().extend(typed);
                }
                classes.insert(name);
            }
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
        Self {
            bindings: MemberCallBindings::with_types(classes.clone()),
            classes,
            properties,
        }
    }

    /// `function run(Repo $repo, $untyped)`: typed parameters bind, others
    /// drop what an outer scope bound to the name.
    fn bind_parameters(&mut self, function: tree_sitter::Node<'_>, source: &[u8]) {
        let Some(parameters) = function.child_by_field_name("parameters") else {
            return;
        };
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            let Some(name) = parameter.child_by_field_name("name") else {
                continue;
            };
            let name = node_text(name, source).trim().to_string();
            match parameter
                .child_by_field_name("type")
                .and_then(|ty| php_class_type(ty, source))
            {
                Some(class) => self.bindings.bind_any(name, class),
                None => self.bindings.forget_foreign(&name),
            }
        }
    }

    /// `$repo = new Repo();` / `$s = makeStore();` / `$t = $s;`.
    fn bind_assignment(&mut self, node: tree_sitter::Node<'_>, source: &[u8]) {
        let (Some(left), Some(mut right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        if left.kind() != "variable_name" {
            return;
        }
        let var = node_text(left, source).trim().to_string();
        while right.kind() == "parenthesized_expression" {
            match right.named_child(0) {
                Some(inner) => right = inner,
                None => return,
            }
        }
        if right.kind() == "object_creation_expression"
            && let Some(class) = php_direct_child(right, &["name", "qualified_name"])
        {
            let class = php_simple_name(&node_text(class, source));
            if !class.is_empty() {
                self.bindings.bind_any(var, class);
                return;
            }
        }
        if right.kind() == "variable_name" {
            let other = node_text(right, source);
            let other = other.trim();
            if let Some(class) = self
                .bindings
                .bound_type(other)
                .or_else(|| self.bindings.foreign_type(other))
                .map(str::to_string)
            {
                self.bindings.bind_any(var, class);
                return;
            }
        }
        match php_call_origin(right, source, &self.bindings) {
            Some(origin) => self.bindings.bind_returned(var, origin),
            None => self.bindings.forget_foreign(&var),
        }
    }

    /// Records what a member call's receiver says:
    ///
    /// * `$repo->save()` on a class of another file (`Repo $repo`, `$repo =
    ///   new Repo()`, `$this->repo` of a typed property): `receiver_type:
    ///   "Repo"`.
    /// * `$factory->create()->save()`, `$s = makeStore(); $s->save()`:
    ///   `receiver_unknown` and `receiver_from` the call it came from; any
    ///   other receiver of unknown type (`$untyped->save()`),
    ///   `receiver_unknown` alone.
    ///
    /// `$this`, a class of this file, and static calls (`Repo::create()`,
    /// `parent::__construct()`) keep the same-file binding.
    fn mark_receiver(
        &self,
        node: tree_sitter::Node<'_>,
        source: &[u8],
        enclosing_class: Option<&str>,
        method: &str,
        extra: &mut serde_json::Value,
    ) {
        if !matches!(
            node.kind(),
            "member_call_expression" | "nullsafe_member_call_expression"
        ) {
            return;
        }
        let Some(object) = node.child_by_field_name("object") else {
            return;
        };
        match self.receiver(object, source, enclosing_class, method) {
            PhpReceiver::Known => {}
            PhpReceiver::Foreign(class) => extra["receiver_type"] = json!(class),
            PhpReceiver::Unknown(origin) => {
                extra["receiver_unknown"] = json!(true);
                if let Some(origin) = origin {
                    extra["receiver_from"] = origin.to_json();
                }
            }
        }
    }

    fn receiver(
        &self,
        object: tree_sitter::Node<'_>,
        source: &[u8],
        enclosing_class: Option<&str>,
        method: &str,
    ) -> PhpReceiver {
        let class = |name: &str| {
            if self.classes.contains(name) {
                PhpReceiver::Known
            } else {
                PhpReceiver::Foreign(name.to_string())
            }
        };
        match object.kind() {
            "parenthesized_expression" => match object.named_child(0) {
                Some(inner) => self.receiver(inner, source, enclosing_class, method),
                None => PhpReceiver::Unknown(None),
            },
            "variable_name" => {
                let name = node_text(object, source);
                let name = name.trim();
                if name == "$this" || self.bindings.bound_type(name).is_some() {
                    return PhpReceiver::Known;
                }
                if let Some(foreign) = self.bindings.foreign_type(name) {
                    return PhpReceiver::Foreign(foreign.to_string());
                }
                PhpReceiver::Unknown(self.bindings.returned_by(name).cloned())
            }
            // `$this->repo->save()`
            "member_access_expression" | "nullsafe_member_access_expression" => object
                .child_by_field_name("object")
                .filter(|owner| {
                    owner.kind() == "variable_name" && node_text(*owner, source).trim() == "$this"
                })
                .and_then(|_| {
                    let property = node_text(object.child_by_field_name("name")?, source);
                    self.properties
                        .get(enclosing_class?)?
                        .get(property.trim())
                        .map(|type_name| class(type_name))
                })
                .unwrap_or(PhpReceiver::Unknown(None)),
            // `(new Repo())->save()`
            "object_creation_expression" => {
                match php_direct_child(object, &["name", "qualified_name"]) {
                    Some(name) => class(&php_simple_name(&node_text(name, source))),
                    None => PhpReceiver::Unknown(None),
                }
            }
            _ => {
                let base = php_past_repeats(object, source, method);
                if base.id() != object.id() {
                    return self.receiver(base, source, enclosing_class, method);
                }
                PhpReceiver::Unknown(php_call_origin(object, source, &self.bindings))
            }
        }
    }
}

/// The class a parameter or property type names: `Repo`, `\App\Repo`,
/// `?Repo`. Union, intersection and primitive types name none.
fn php_class_type(ty: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let named = match ty.kind() {
        "named_type" => ty,
        "optional_type" => php_direct_child(ty, &["named_type"])?,
        _ => return None,
    };
    let name = php_direct_child(named, &["name", "qualified_name"])?;
    let name = php_simple_name(&node_text(name, source));
    (!name.is_empty() && !matches!(name.as_str(), "self" | "static" | "parent")).then_some(name)
}

/// The class-typed properties of a class: `private Repo $repo;` and a
/// promoted constructor parameter `private Repo $repo`.
fn php_typed_properties(class: tree_sitter::Node<'_>, source: &[u8]) -> HashMap<String, String> {
    let mut typed = HashMap::new();
    let Some(body) = class.child_by_field_name("body") else {
        return typed;
    };
    let mut cursor = body.walk();
    for member in body.children(&mut cursor) {
        match member.kind() {
            "property_declaration" => {
                let Some(class) = member
                    .child_by_field_name("type")
                    .and_then(|ty| php_class_type(ty, source))
                else {
                    continue;
                };
                let mut elements = member.walk();
                for element in member.children(&mut elements) {
                    if element.kind() == "property_element"
                        && let Some(name) = element.child_by_field_name("name")
                    {
                        let name = node_text(name, source);
                        typed.insert(
                            name.trim().trim_start_matches('$').to_string(),
                            class.clone(),
                        );
                    }
                }
            }
            "method_declaration" => {
                let Some(parameters) = member.child_by_field_name("parameters") else {
                    continue;
                };
                let mut params = parameters.walk();
                for parameter in parameters.named_children(&mut params) {
                    if parameter.kind() != "property_promotion_parameter" {
                        continue;
                    }
                    if let (Some(name), Some(class)) = (
                        parameter.child_by_field_name("name"),
                        parameter
                            .child_by_field_name("type")
                            .and_then(|ty| php_class_type(ty, source)),
                    ) {
                        let name = node_text(name, source);
                        typed.insert(name.trim().trim_start_matches('$').to_string(), class);
                    }
                }
            }
            _ => {}
        }
    }
    typed
}

/// The receiver of a chain past calls of the same method: a line holds a
/// single edge per target, so in `$b->flag(1)->flag(2)->build()` the edge
/// of `flag` stands for both and its receiver is `$b`.
fn php_past_repeats<'tree>(
    mut receiver: tree_sitter::Node<'tree>,
    source: &[u8],
    method: &str,
) -> tree_sitter::Node<'tree> {
    while matches!(
        receiver.kind(),
        "member_call_expression" | "nullsafe_member_call_expression"
    ) && receiver
        .child_by_field_name("name")
        .is_some_and(|name| node_text(name, source).trim() == method)
        && let Some(object) = receiver.child_by_field_name("object")
    {
        receiver = object;
    }
    receiver
}

/// The call an expression is the result of: `makeStore()`,
/// `$factory->create()`, `Repo::open()` (the called name as written, its
/// rightmost identifier, and the line of its CALLS edge), or a variable
/// bound to one (`$s = makeStore();`).
fn php_call_origin(
    expression: tree_sitter::Node<'_>,
    source: &[u8],
    bindings: &MemberCallBindings,
) -> Option<CallOrigin> {
    match expression.kind() {
        "parenthesized_expression" => php_call_origin(expression.named_child(0)?, source, bindings),
        "function_call_expression"
        | "member_call_expression"
        | "nullsafe_member_call_expression"
        | "scoped_call_expression" => {
            let name = php_simple_name(&php_call_name(expression, source)?);
            (!name.is_empty()).then(|| CallOrigin {
                name,
                line: expression.start_position().row as i64 + 1,
                unwrap: false,
            })
        }
        "variable_name" => bindings
            .returned_by(node_text(expression, source).trim())
            .cloned(),
        _ => None,
    }
}
