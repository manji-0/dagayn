use std::collections::HashMap;
use std::path::Path;

use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    is_test_file, line_count, node_text, resolve_import_path, strip_matching_quotes,
};
use super::{add_tested_by_edges, qualify};

pub(super) fn parse_zig_with_parser(
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
        language: "zig".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = ZigParseContext {
        source,
        file_path: file_path.clone(),
        repo_root,
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        zig_walk_children(
            tree.root_node(),
            &context,
            &Scope::default(),
            &mut nodes,
            &mut edges,
        );
        let mut edges = resolve_zig_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct ZigParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    repo_root: Option<&'a Path>,
}

/// Where a node sits: the dotted container path and the enclosing function.
#[derive(Clone, Default)]
struct Scope {
    container: Option<String>,
    func: Option<String>,
}

impl Scope {
    fn child_path(&self, name: &str) -> String {
        match &self.container {
            Some(container) => format!("{container}.{name}"),
            None => name.to_string(),
        }
    }

    /// Qualified caller for calls made here; container-level initializers
    /// are attributed to the container.
    fn caller(&self, file_path: &FilePath) -> String {
        match (&self.func, &self.container) {
            (Some(func), container) => qualify(file_path, func, container.as_deref()),
            (None, Some(container)) => qualify(file_path, container, None),
            (None, None) => file_path.to_string(),
        }
    }

    fn contains_source(&self, file_path: &FilePath) -> String {
        self.container
            .as_deref()
            .map(|container| qualify(file_path, container, None))
            .unwrap_or_else(|| file_path.to_string())
    }
}

fn zig_walk_children(
    node: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        zig_visit(child, context, scope, nodes, edges);
    }
}

fn zig_visit(
    node: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    match node.kind() {
        "Decl" => {
            if let Some(proto) = zig_direct_child(node, &["FnProto"]) {
                zig_handle_function(node, proto, context, scope, nodes, edges);
                return;
            }
        }
        "VarDecl" if zig_handle_var_decl(node, context, scope, nodes, edges) => return,
        "TestDecl" => {
            zig_handle_test(node, context, scope, nodes, edges);
            return;
        }
        // `return struct { ... };` inside a type-returning function.
        "ContainerDecl" => {
            let owner = Scope {
                container: scope.func.as_ref().map(|func| scope.child_path(func)),
                func: None,
            };
            let owner = if owner.container.is_some() {
                owner
            } else {
                scope.clone()
            };
            zig_walk_children(node, context, &owner, nodes, edges);
            return;
        }
        "SuffixExpr" => zig_emit_suffix_calls(node, context, scope, edges),
        _ => {}
    }
    zig_walk_children(node, context, scope, nodes, edges);
}

fn zig_handle_function(
    decl: tree_sitter::Node<'_>,
    proto: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(name) = zig_direct_child_text(proto, context.source, &["IDENTIFIER"]) else {
        return;
    };
    let modifiers = zig_decl_modifiers(decl, context.source);
    let return_type = zig_direct_child_text(proto, context.source, &["ErrorUnionExpr"]);
    let extra = if return_type.as_deref() == Some("type") {
        json!({"type_role": "type_function"})
    } else {
        json!({})
    };
    let qualified = qualify(&context.file_path, &name, scope.container.as_deref());
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: decl.start_position().row as i64 + 1,
        line_end: decl.end_position().row as i64 + 1,
        language: "zig".to_string(),
        parent_name: scope.container.clone(),
        params: zig_direct_child_text(proto, context.source, &["ParamDeclList"]),
        return_type,
        modifiers: (!modifiers.is_empty()).then(|| modifiers.join(" ")),
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: scope.contains_source(&context.file_path),
        target: qualified,
        file_path: context.file_path.clone(),
        line: decl.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(block) = zig_direct_child(decl, &["Block"]) {
        let body_scope = Scope {
            container: scope.container.clone(),
            func: Some(name),
        };
        zig_walk_children(block, context, &body_scope, nodes, edges);
    }
}

/// `const Name = struct {...}` / `enum` / `union` / `opaque` / `error{...}` and
/// `const x = @import("...")`. Returns false for ordinary values.
fn zig_handle_var_decl(
    node: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = zig_direct_child_text(node, context.source, &["IDENTIFIER"]) else {
        return false;
    };
    let Some(value) = zig_direct_child(node, &["ErrorUnionExpr"])
        .and_then(|expr| zig_direct_child(expr, &["SuffixExpr"]))
    else {
        return false;
    };
    if let Some(target) = zig_import_target(value, context) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: context.file_path.to_string(),
            target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
        return true;
    }
    let (container, role) = if let Some(container) = zig_direct_child(value, &["ContainerDecl"]) {
        let role = zig_direct_child(container, &["ContainerDeclType"])
            .map(|decl_type| zig_container_role(&node_text(decl_type, context.source)))
            .unwrap_or("struct");
        (Some(container), role)
    } else if zig_direct_child(value, &["ErrorSetDecl"]).is_some() {
        (None, "error_set")
    } else {
        return false;
    };

    let path = scope.child_path(&name);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name,
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "zig".to_string(),
        parent_name: scope.container.clone(),
        params: None,
        return_type: None,
        modifiers: node
            .parent()
            .map(|decl| zig_decl_modifiers(decl, context.source))
            .filter(|modifiers| !modifiers.is_empty())
            .map(|modifiers| modifiers.join(" ")),
        is_test: false,
        extra: json!({"type_role": role}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: scope.contains_source(&context.file_path),
        target: qualify(&context.file_path, &path, None),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(container) = container {
        let inner = Scope {
            container: Some(path),
            func: None,
        };
        zig_walk_children(container, context, &inner, nodes, edges);
    }
    true
}

fn zig_handle_test(
    node: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    // `test "name" {}`, a doctest `test helper {}` (named apart from `helper`
    // itself), or an anonymous `test {}`.
    let name = if let Some(label) = zig_direct_child(node, &["STRINGLITERALSINGLE"]) {
        strip_matching_quotes(&node_text(label, context.source)).to_string()
    } else if let Some(decl) = zig_direct_child_text(node, context.source, &["IDENTIFIER"]) {
        format!("test {decl}")
    } else {
        format!("test@{}", node.start_position().row + 1)
    };
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Test,
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "zig".to_string(),
        parent_name: scope.container.clone(),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: scope.contains_source(&context.file_path),
        target: qualify(&context.file_path, &name, scope.container.as_deref()),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(block) = zig_direct_child(node, &["Block"]) {
        let body_scope = Scope {
            container: scope.container.clone(),
            func: Some(name),
        };
        zig_walk_children(block, context, &body_scope, nodes, edges);
    }
}

/// Emits one CALLS edge per call in a suffix chain such as `std.debug.print(...)`
/// (target `std.debug.print`) or `a.b().c()` (targets `a.b`, then `c`).
fn zig_emit_suffix_calls(
    node: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    let Some(head) = children.first() else {
        return;
    };
    let mut path = match head.kind() {
        "IDENTIFIER" => Some(node_text(*head, context.source)),
        _ => None,
    };
    for child in &children[1..] {
        match child.kind() {
            "FnCallArguments" => {
                if let Some(target) = path.take() {
                    zig_push_call(child, context, scope, target, edges);
                }
            }
            "FieldOrFnCall" => {
                let Some(field) = zig_direct_child_text(*child, context.source, &["IDENTIFIER"])
                else {
                    path = None;
                    continue;
                };
                let target = match path.take() {
                    Some(prefix) => format!("{prefix}.{field}"),
                    None => field,
                };
                if zig_direct_child(*child, &["FnCallArguments"]).is_some() {
                    zig_push_call(child, context, scope, target, edges);
                } else {
                    path = Some(target);
                }
            }
            _ => path = None,
        }
    }
}

fn zig_push_call(
    node: &tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    target: String,
    edges: &mut Vec<ParsedEdge>,
) {
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: scope.caller(&context.file_path),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn zig_import_target(
    value: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
) -> Option<String> {
    let builtin = zig_direct_child(value, &["BUILTINIDENTIFIER"])?;
    if node_text(builtin, context.source) != "@import" {
        return None;
    }
    let arguments = zig_direct_child(value, &["FnCallArguments"])?;
    let literal = zig_first_descendant(arguments, "STRINGLITERALSINGLE")?;
    let literal = strip_matching_quotes(&node_text(literal, context.source)).to_string();
    if !literal.ends_with(".zig") && !literal.ends_with(".zon") {
        // `std`, `builtin`, and build.zig module names.
        return Some(literal);
    }
    Some(
        resolve_import_path(&literal, &context.file_path, context.repo_root, &[], false)
            .unwrap_or(literal),
    )
}

fn zig_container_role(decl_type: &str) -> &'static str {
    let keyword = decl_type
        .split(|c: char| !c.is_ascii_alphabetic())
        .find(|word| matches!(*word, "struct" | "enum" | "union" | "opaque"));
    match keyword {
        Some("enum") => "enum",
        Some("union") => "union",
        Some("opaque") => "opaque",
        _ => "struct",
    }
}

fn zig_decl_modifiers(decl: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = decl.walk();
    let mut modifiers = Vec::new();
    // `pub` precedes the `Decl` as a sibling token in the container.
    if decl
        .prev_sibling()
        .is_some_and(|previous| !previous.is_named() && node_text(previous, source) == "pub")
    {
        modifiers.push("pub".to_string());
    }
    for child in decl.children(&mut cursor) {
        if child.is_named() {
            continue;
        }
        let text = node_text(child, source);
        if matches!(
            text.as_str(),
            "pub" | "extern" | "export" | "inline" | "noinline" | "threadlocal"
        ) {
            modifiers.push(text);
        }
    }
    modifiers
}

fn zig_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn zig_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    zig_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn zig_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = zig_first_descendant(child, kind) {
            return Some(found);
        }
    }
    None
}

/// Resolves call targets against same-file declarations: `self.m` binds to the
/// caller's container, and other names are looked up from the innermost scope
/// outwards (`Point.init` from inside `Point.len` finds `Point.init`).
fn resolve_zig_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Class"))
        .map(|node| {
            let path = match &node.parent_name {
                Some(parent) => format!("{parent}.{}", node.name),
                None => node.name.clone(),
            };
            (path.clone(), qualify(file_path, &path, None))
        })
        .collect::<HashMap<_, _>>();
    let prefix = format!("{file_path}::");
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind != "CALLS" || edge.target.contains("::") {
                return edge;
            }
            let caller = edge.source.strip_prefix(&prefix).unwrap_or("");
            let mut scopes = Vec::new();
            let mut current = caller;
            while let Some((parent, _)) = current.rsplit_once('.') {
                scopes.push(parent);
                current = parent;
            }
            if !caller.is_empty() {
                scopes.insert(0, caller);
            }
            let target = match edge.target.strip_prefix("self.") {
                // `self.m()` inside `T.f` means `T.m`.
                Some(method) => scopes
                    .get(1)
                    .map(|container| format!("{container}.{method}"))
                    .and_then(|path| symbols.get(&path)),
                None => scopes
                    .iter()
                    .map(|scope| format!("{scope}.{}", edge.target))
                    .find_map(|path| symbols.get(&path))
                    .or_else(|| symbols.get(&edge.target)),
            };
            if let Some(target) = target {
                edge.target = target.clone();
            }
            edge
        })
        .collect()
}
