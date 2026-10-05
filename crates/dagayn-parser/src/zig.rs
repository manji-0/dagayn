use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::member_calls::{CallOrigin, MemberCallBindings};

use super::stdlib::zig::zig_std_module;
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child, direct_child_text, first_descendant, line_count, line_of, node_text,
    resolve_import_path, strip_matching_quotes,
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
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "zig")];
    let mut edges = Vec::new();
    let mut context = ZigParseContext {
        source,
        file_path: file_path.clone(),
        repo_root,
        c_imports: HashSet::new(),
        std_aliases: HashMap::new(),
        containers: HashSet::new(),
        values: RefCell::default(),
        bindings: RefCell::default(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        context.c_imports = zig_c_import_namespaces(tree.root_node(), source);
        context.std_aliases = zig_std_aliases(tree.root_node(), source);
        zig_collect_containers(tree.root_node(), source, &mut context.containers);
        context.bindings = RefCell::new(MemberCallBindings::with_types(context.containers.clone()));
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
    /// Constants bound to `@cImport(...)`: calls through them call C.
    c_imports: HashSet<String>,
    /// Constants bound to the standard library or a path into it: `std` for
    /// `const std = @import("std")` is `("std", "std")`, and `mem` for
    /// `const mem = std.mem` is `("std", "std.mem")`.
    std_aliases: HashMap<String, (&'static str, String)>,
    /// Types the file declares (`const Point = struct {...}`), by name.
    containers: HashSet<String>,
    /// Parameters and variables of the function being walked that hold a
    /// value (`p: *Store`, `const n = count()`), rather than a type or a
    /// namespace (`const Alias = other.Thing`).
    values: RefCell<HashSet<String>>,
    /// The types of the values in scope (`const s: Store`,
    /// `const s = Store.init()`), and the calls they hold the result of
    /// (`const c = try connect()`).
    bindings: RefCell<MemberCallBindings>,
}

/// The constants of a file that name the standard library: bound to
/// `@import("std")` / `@import("builtin")`, or to a path through such a
/// constant (`const mem = std.mem;`, `const print = std.debug.print;`).
/// Zig forbids shadowing, so a name bound anywhere means the same everywhere.
fn zig_std_aliases(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> HashMap<String, (&'static str, String)> {
    let mut bindings = Vec::new();
    zig_collect_const_bindings(root, source, &mut bindings);
    let mut aliases = HashMap::new();
    // Declarations are order-independent: repeat until no path gains a root.
    loop {
        let before = aliases.len();
        for (name, value) in &bindings {
            if aliases.contains_key(name) {
                continue;
            }
            let resolved = match value.strip_prefix("@import(") {
                Some(argument) => {
                    let literal = argument.trim_end_matches(')').trim();
                    zig_std_module(strip_matching_quotes(literal))
                        .map(|module| (module, module.to_string()))
                }
                None => {
                    let (head, rest) = value.split_once('.').unwrap_or((value, ""));
                    let path_like = value
                        .chars()
                        .all(|c| c == '.' || c == '_' || c.is_ascii_alphanumeric());
                    aliases.get(head).filter(|_| path_like).map(
                        |(package, expansion): &(&'static str, String)| {
                            let expansion = if rest.is_empty() {
                                expansion.clone()
                            } else {
                                format!("{expansion}.{rest}")
                            };
                            (*package, expansion)
                        },
                    )
                }
            };
            if let Some(resolved) = resolved {
                aliases.insert(name.clone(), resolved);
            }
        }
        if aliases.len() == before {
            return aliases;
        }
    }
}

/// `(name, value)` of every `const`/`var` with an initializer, the value
/// without whitespace.
fn zig_collect_const_bindings(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<(String, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "VarDecl"
            && let Some(name) = direct_child_text(child, source, &["IDENTIFIER"])
            && let Some(value) = direct_child(child, &["ErrorUnionExpr"])
        {
            let value = node_text(value, source).replace(char::is_whitespace, "");
            out.push((name, value));
        }
        zig_collect_const_bindings(child, source, out);
    }
}

/// `const c = @cImport({ ... });` at the top level of the file.
fn zig_c_import_namespaces(root: tree_sitter::Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut cursor = root.walk();
    for decl in root.children(&mut cursor) {
        let Some(var) =
            direct_child(decl, &["VarDecl"]).or((decl.kind() == "VarDecl").then_some(decl))
        else {
            continue;
        };
        let Some(name) = direct_child_text(var, source, &["IDENTIFIER"]) else {
            continue;
        };
        let value = direct_child(var, &["ErrorUnionExpr"]).map(|expr| node_text(expr, source));
        if value.is_some_and(|value| value.trim_start().starts_with("@cImport")) {
            names.insert(name);
        }
    }
    names
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
            if let Some(proto) = direct_child(node, &["FnProto"]) {
                zig_handle_function(node, proto, context, scope, nodes, edges);
                return;
            }
        }
        "VarDecl" => {
            if zig_handle_var_decl(node, context, scope, nodes, edges) {
                return;
            }
            // After its value is walked: `const s = s.next()` would call
            // `next` on the `s` before it.
            zig_walk_children(node, context, scope, nodes, edges);
            zig_bind_var_decl(node, context);
            return;
        }
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
    let Some(name) = direct_child_text(proto, context.source, &["IDENTIFIER"]) else {
        return;
    };
    let modifiers = zig_decl_modifiers(decl, context.source);
    let return_type = zig_return_type(proto, context.source);
    let mut extra = if return_type.as_deref() == Some("type") {
        json!({"type_role": "type_function"})
    } else {
        json!({})
    };
    let has_body = direct_child(decl, &["Block"]).is_some();
    if modifiers.iter().any(|modifier| modifier == "export") && has_body {
        // `export fn f` is a C symbol other languages link against.
        extra["ffi_export"] = json!({"abi": "c", "kind": "function", "name": name});
    } else if modifiers.iter().any(|modifier| modifier == "extern") && !has_body {
        // `extern fn f(...) T;` / `extern "lib" fn f(...) T;`
        let mut import = json!({"abi": "c", "name": name});
        if let Some(library) = direct_child_text(decl, context.source, &["STRINGLITERALSINGLE"])
            .map(|literal| strip_matching_quotes(&literal).to_string())
            .filter(|library| library != "c")
        {
            import["library"] = json!(library);
        }
        extra["ffi_import"] = import;
    }
    let qualified = qualify(&context.file_path, &name, scope.container.as_deref());
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.clone(),
        file_path: context.file_path.clone(),
        line_start: decl.start_position().row as i64 + 1,
        line_end: decl.end_position().row as i64 + 1,
        language: "zig".to_string(),
        parent_name: scope.container.clone(),
        params: direct_child_text(proto, context.source, &["ParamDeclList"]),
        return_type,
        modifiers: (!modifiers.is_empty()).then(|| modifiers.join(" ")),
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        scope.contains_source(&context.file_path),
        qualified,
        context.file_path.clone(),
        line_of(decl),
    ));
    if let Some(block) = direct_child(decl, &["Block"]) {
        let body_scope = Scope {
            container: scope.container.clone(),
            func: Some(name),
        };
        let values = context.values.borrow().clone();
        let bindings = context.bindings.borrow().snapshot();
        if let Some(parameters) = direct_child(proto, &["ParamDeclList"]) {
            zig_bind_parameters(parameters, context);
        }
        zig_walk_children(block, context, &body_scope, nodes, edges);
        *context.values.borrow_mut() = values;
        context.bindings.borrow_mut().restore(bindings);
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
    let Some(name) = direct_child_text(node, context.source, &["IDENTIFIER"]) else {
        return false;
    };
    let Some(value) = direct_child(node, &["ErrorUnionExpr"])
        .and_then(|expr| direct_child(expr, &["SuffixExpr"]))
    else {
        return false;
    };
    if let Some(mut target) = zig_import_target(value, context) {
        let mut extra = json!({});
        if let Some(module) = zig_std_module(&target) {
            mark_stdlib_edge(&mut target, &mut extra, module, StdlibEvidence::Certain);
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: context.file_path.to_string(),
            target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
        return true;
    }
    let (container, role) = if let Some(container) = direct_child(value, &["ContainerDecl"]) {
        let role = direct_child(container, &["ContainerDeclType"])
            .map(|decl_type| zig_container_role(&node_text(decl_type, context.source)))
            .unwrap_or("struct");
        (Some(container), role)
    } else if direct_child(value, &["ErrorSetDecl"]).is_some() {
        (None, "error_set")
    } else {
        return false;
    };

    // A type declared in a function body is local to it: `a`'s `const S =
    // struct` is `a.S`, apart from `b.S`.
    let local = scope.func.as_ref().map(|func| Scope {
        container: Some(scope.child_path(func)),
        func: None,
    });
    let scope = local.as_ref().unwrap_or(scope);
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
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        scope.contains_source(&context.file_path),
        qualify(&context.file_path, &path, None),
        context.file_path.clone(),
        line_of(node),
    ));
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
    let name = if let Some(label) = direct_child(node, &["STRINGLITERALSINGLE"]) {
        strip_matching_quotes(&node_text(label, context.source)).to_string()
    } else if let Some(decl) = direct_child_text(node, context.source, &["IDENTIFIER"]) {
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
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        scope.contains_source(&context.file_path),
        qualify(&context.file_path, &name, scope.container.as_deref()),
        context.file_path.clone(),
        line_of(node),
    ));
    if let Some(block) = direct_child(node, &["Block"]) {
        let body_scope = Scope {
            container: scope.container.clone(),
            func: Some(name),
        };
        let values = context.values.borrow().clone();
        let bindings = context.bindings.borrow().snapshot();
        zig_walk_children(block, context, &body_scope, nodes, edges);
        *context.values.borrow_mut() = values;
        context.bindings.borrow_mut().restore(bindings);
    }
}

/// Emits one CALLS edge per call in a suffix chain such as `std.debug.print(...)`
/// (target `std.debug.print`) or `a.b().c()` (targets `a.b`, then `c`).
///
/// A chain rooted at a constant naming the standard library calls into it
/// all along: `std.ArrayList(u8).init(a)` calls `std.ArrayList`, then
/// `init` of the type it returned (`std.ArrayList.init`).
///
/// A method of a value is named by its type when the file says it
/// (`s.save()` with `s: Store` is `Store.save` for a type of this file, or
/// `save` with `receiver_type: "Store"` for one of another), and is `save`
/// with `receiver_unknown` otherwise (a parameter of unknown type, a field
/// of a value, a call's result, with the call in `receiver_from`).
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
    // Whether `path` still starts at the head of the chain, rather than at
    // a field of a call's result (`make().inner.run()`).
    let mut rooted = path.is_some();
    // The call the chain's value is the result of so far, and what that
    // call's own receiver came from.
    let mut last_call: Option<(CallOrigin, Option<CallOrigin>)> = None;
    // `(package, symbol)` while the chain stays in the standard library.
    let mut std_path = path
        .as_ref()
        .and_then(|head| context.std_aliases.get(head))
        .map(|(package, expansion)| (*package, expansion.clone()));
    for child in &children[1..] {
        let line = child.start_position().row as i64 + 1;
        match child.kind() {
            "FnCallArguments" => {
                if let Some(target) = path.take() {
                    let name = target.rsplit('.').next().unwrap_or(&target).to_string();
                    zig_push_call(child, context, scope, target, std_path.clone(), None, edges);
                    last_call = Some((
                        CallOrigin {
                            name,
                            line,
                            unwrap: false,
                        },
                        None,
                    ));
                } else {
                    last_call = None;
                }
                rooted = false;
            }
            "FieldOrFnCall" => {
                let Some(field) = direct_child_text(*child, context.source, &["IDENTIFIER"]) else {
                    path = None;
                    std_path = None;
                    rooted = false;
                    last_call = None;
                    continue;
                };
                if let Some((_, symbol)) = std_path.as_mut() {
                    symbol.push('.');
                    symbol.push_str(&field);
                }
                let receiver = path.take();
                let target = match &receiver {
                    Some(prefix) => format!("{prefix}.{field}"),
                    None => field.clone(),
                };
                if direct_child(*child, &["FnCallArguments"]).is_some() {
                    let marked = if std_path.is_none() {
                        zig_receiver(
                            receiver.as_deref(),
                            rooted,
                            &field,
                            line,
                            &last_call,
                            context,
                        )
                    } else {
                        None
                    };
                    let from = marked
                        .as_ref()
                        .and_then(|(_, extra)| extra.get("receiver_from"))
                        .and_then(|from| {
                            Some(CallOrigin {
                                name: from.get("call")?.as_str()?.to_string(),
                                line: from.get("line")?.as_i64()?,
                                unwrap: from.get("unwrap")?.as_bool()?,
                            })
                        });
                    match marked {
                        Some((target, extra)) => {
                            zig_push_call(child, context, scope, target, None, Some(extra), edges)
                        }
                        None => zig_push_call(
                            child,
                            context,
                            scope,
                            target,
                            std_path.clone(),
                            None,
                            edges,
                        ),
                    }
                    last_call = Some((
                        CallOrigin {
                            name: field,
                            line,
                            unwrap: false,
                        },
                        from,
                    ));
                    rooted = false;
                } else {
                    path = Some(target);
                    last_call = None;
                }
            }
            // `make().?.close()` / `ptr.*.close()`: the same value, the
            // optional unwrapped.
            "SuffixOp" if matches!(node_text(*child, context.source).trim(), ".?" | ".*") => {
                if node_text(*child, context.source).trim() == ".?"
                    && let Some((origin, _)) = last_call.as_mut()
                {
                    origin.unwrap = true;
                }
                path = None;
                std_path = None;
                rooted = false;
            }
            _ => {
                path = None;
                std_path = None;
                rooted = false;
                last_call = None;
            }
        }
    }
}

/// The target and metadata of a method call on `receiver` (the dotted path
/// before it, `None` for a call's result), or `None` when the call keeps
/// its path: `self.m()`, a function of a type or namespace (`Store.init`,
/// `mem.eql`).
fn zig_receiver(
    receiver: Option<&str>,
    rooted: bool,
    method: &str,
    line: i64,
    last_call: &Option<(CallOrigin, Option<CallOrigin>)>,
    context: &ZigParseContext<'_>,
) -> Option<(String, Value)> {
    let unknown = |from: Option<CallOrigin>| {
        let mut extra = json!({"receiver_unknown": true});
        if let Some(from) = from {
            extra["receiver_from"] = from.to_json();
        }
        Some((method.to_string(), extra))
    };
    let Some(receiver) = receiver else {
        // A call's result: `make().close()`. A chain repeating `method`
        // (`q.where(a).where(b)`) points past the repeats, which share one
        // edge per line.
        return match last_call {
            Some((origin, from)) if origin.name == method && origin.line == line => {
                unknown(from.clone())
            }
            Some((origin, _)) => unknown(Some(origin.clone())),
            None => unknown(None),
        };
    };
    if !rooted {
        return unknown(None);
    }
    let (root, rest) = match receiver.split_once('.') {
        Some((root, rest)) => (root, Some(rest)),
        None => (receiver, None),
    };
    let bindings = context.bindings.borrow();
    let is_value = root == "self"
        || context.values.borrow().contains(root)
        || bindings.is_bound(root)
        || bindings.foreign_type(root).is_some()
        || bindings.returned_by(root).is_some();
    if rest.is_some() {
        // A field of a value (`self.pool.get()`, `p.inner.run()`).
        return if is_value { unknown(None) } else { None };
    }
    if root == "self" {
        return None;
    }
    if let Some(bound) = bindings.bound_type(root) {
        return Some((format!("{bound}.{method}"), json!({})));
    }
    if let Some(type_name) = bindings.foreign_type(root) {
        return Some((method.to_string(), json!({"receiver_type": type_name})));
    }
    if let Some(origin) = bindings.returned_by(root) {
        return unknown(Some(origin.clone()));
    }
    if is_value {
        return unknown(None);
    }
    None
}

fn zig_push_call(
    node: &tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
    scope: &Scope,
    target: String,
    std_path: Option<(&'static str, String)>,
    receiver: Option<Value>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let Some((package, mut symbol)) = std_path {
        let mut extra = json!({});
        mark_stdlib_edge(&mut symbol, &mut extra, package, StdlibEvidence::Certain);
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: scope.caller(&context.file_path),
            target: symbol,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
        return;
    }
    // `c.fast_sum(...)` through `const c = @cImport(...)` calls C.
    let c_import = target
        .split_once('.')
        .is_some_and(|(namespace, _)| context.c_imports.contains(namespace));
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: scope.caller(&context.file_path),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: match receiver {
            Some(extra) => extra,
            None if c_import => json!({"c_import": true}),
            None => json!({}),
        },
    });
}

/// The return type of a function as written: `!Store`, `?*Store`,
/// `anyerror!Store`, `void`; a `callconv(..)` / `align(..)` before it is
/// not part of it.
fn zig_return_type(proto: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = proto.walk();
    let mut after_params = false;
    let start = proto.children(&mut cursor).find(|child| {
        if child.kind() == "ParamDeclList" {
            after_params = true;
            return false;
        }
        after_params
            && !matches!(
                child.kind(),
                "ByteAlign" | "AddrSpace" | "LinkSection" | "CallConv"
            )
    })?;
    let text = source.get(start.start_byte()..proto.end_byte())?;
    let text = String::from_utf8_lossy(text).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Types declared anywhere in the file (`const Point = struct {...}`).
fn zig_collect_containers(node: tree_sitter::Node<'_>, source: &[u8], names: &mut HashSet<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "VarDecl"
            && let Some(name) = direct_child_text(child, source, &["IDENTIFIER"])
            && child
                .named_children(&mut child.walk())
                .any(|value| first_descendant(value, &["ContainerDecl"]).is_some())
        {
            names.insert(name);
        }
        zig_collect_containers(child, source, names);
    }
}

/// The type a type expression names: `Store` for `Store`, `*Store`,
/// `?*const Store`, `!Store`; nothing for a primitive (`u8`, `anytype`) or
/// a generic instance (`std.ArrayList(u8)`).
fn zig_type_name(nodes: &[tree_sitter::Node<'_>], source: &[u8]) -> Option<String> {
    let last = nodes
        .iter()
        .rev()
        .find(|node| node.kind() != "PrefixTypeOp")?;
    let text = node_text(*last, source);
    let text = text.trim().trim_start_matches(['!', '?', '*']);
    let text = text.rsplit('!').next().unwrap_or(text).trim();
    let name = text.rsplit('.').next().unwrap_or(text);
    (name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()))
    .then(|| name.to_string())
}

/// Binds `var` to a type named in the file: one it declares, or one of
/// another file (`receiver_type`).
fn zig_bind_type(var: String, type_name: String, context: &ZigParseContext<'_>) {
    let mut bindings = context.bindings.borrow_mut();
    if context.containers.contains(&type_name) {
        bindings.forget_foreign(&var);
        bindings.bind(var, type_name);
    } else {
        bindings.bind_any(var, type_name);
    }
}

/// Binds the parameters of a function: typed ones to their type
/// (`p: *Store`), the others as values of unknown type (`q: anytype`).
fn zig_bind_parameters(parameters: tree_sitter::Node<'_>, context: &ZigParseContext<'_>) {
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        let Some(name) = direct_child_text(parameter, context.source, &["IDENTIFIER"]) else {
            continue;
        };
        context.values.borrow_mut().insert(name.clone());
        let type_name = direct_child(parameter, &["ParamType"]).and_then(|param_type| {
            let mut cursor = param_type.walk();
            let parts = param_type.named_children(&mut cursor).collect::<Vec<_>>();
            zig_type_name(&parts, context.source)
        });
        match type_name {
            Some(type_name) => zig_bind_type(name, type_name, context),
            None => context.bindings.borrow_mut().forget_foreign(&name),
        }
    }
}

/// Binds a `const` / `var` to what it says: its type (`const s: Store`),
/// the type its value constructs (`Store.init(..)`, `Store{ .. }`), or the
/// call its value is the result of (`const c = try connect()`). A plain
/// path (`const Alias = other.Thing`) names a type or a namespace, not a
/// value.
fn zig_bind_var_decl(node: tree_sitter::Node<'_>, context: &ZigParseContext<'_>) {
    let source = context.source;
    let Some(name) = direct_child_text(node, source, &["IDENTIFIER"]) else {
        return;
    };
    if context.std_aliases.contains_key(&name) || context.c_imports.contains(&name) {
        return;
    }
    let mut cursor = node.walk();
    let (mut declared, mut value) = (Vec::new(), Vec::new());
    let mut section = 0;
    for child in node.children(&mut cursor) {
        match (child.is_named(), node_text(child, source).as_str()) {
            (false, ":") => section = 1,
            (false, "=") => section = 2,
            (true, _) if child.kind() != "IDENTIFIER" || section > 0 => match section {
                1 => declared.push(child),
                2 => value.push(child),
                _ => {}
            },
            _ => {}
        }
    }
    let is_path = value.len() == 1
        && value[0].kind() == "ErrorUnionExpr"
        && node_text(value[0], source)
            .chars()
            .all(|c| c == '.' || c == '_' || c.is_ascii_alphanumeric());
    if is_path && declared.is_empty() {
        context.values.borrow_mut().remove(&name);
        context.bindings.borrow_mut().forget_foreign(&name);
        return;
    }
    context.values.borrow_mut().insert(name.clone());
    if let Some(type_name) = zig_type_name(&declared, source) {
        zig_bind_type(name, type_name, context);
        return;
    }
    // `Store{ .x = 1 }`.
    if let [literal_type, init] = value.as_slice()
        && init.kind() == "InitList"
        && let Some(type_name) = zig_type_name(&[*literal_type], source)
    {
        zig_bind_type(name, type_name, context);
        return;
    }
    let Some(expression) = value.first().copied() else {
        return;
    };
    // `try connect()` unwraps the error union the call returned.
    let (expression, unwrap) = match expression.kind() {
        "UnaryExpr"
            if expression
                .child_by_field_name("operator")
                .is_some_and(|operator| node_text(operator, source) == "try") =>
        {
            match expression.child_by_field_name("left") {
                Some(left) => (left, true),
                None => return,
            }
        }
        _ => (expression, false),
    };
    let Some(suffix) = direct_child(expression, &["SuffixExpr"]) else {
        return;
    };
    // `Store.init(..)` constructs a `Store`.
    let mut cursor = suffix.walk();
    let parts = suffix.children(&mut cursor).collect::<Vec<_>>();
    if let [head, call] = parts.as_slice()
        && head.kind() == "IDENTIFIER"
        && call.kind() == "FieldOrFnCall"
        && direct_child_text(*call, source, &["IDENTIFIER"]).as_deref() == Some("init")
        && direct_child(*call, &["FnCallArguments"]).is_some()
        && let Some(type_name) = zig_type_name(&[*head], source)
    {
        zig_bind_type(name, type_name, context);
        return;
    }
    // The last call of the value: `connect()`, `pool.acquire()`.
    let last = parts.last().copied();
    let origin = match last.map(|last| (last.kind(), last)) {
        Some(("FnCallArguments", last)) => parts
            .iter()
            .rev()
            .nth(1)
            .filter(|callee| callee.kind() == "IDENTIFIER")
            .map(|callee| (node_text(*callee, source), last)),
        Some(("FieldOrFnCall", last)) if direct_child(last, &["FnCallArguments"]).is_some() => {
            direct_child_text(last, source, &["IDENTIFIER"]).map(|name| (name, last))
        }
        _ => None,
    };
    let mut bindings = context.bindings.borrow_mut();
    match origin {
        Some((call, last)) => bindings.bind_returned(
            name,
            CallOrigin {
                name: call,
                line: last.start_position().row as i64 + 1,
                unwrap,
            },
        ),
        None => bindings.forget_foreign(&name),
    }
}

fn zig_import_target(
    value: tree_sitter::Node<'_>,
    context: &ZigParseContext<'_>,
) -> Option<String> {
    let builtin = direct_child(value, &["BUILTINIDENTIFIER"])?;
    if node_text(builtin, context.source) != "@import" {
        return None;
    }
    let arguments = direct_child(value, &["FnCallArguments"])?;
    let literal = first_descendant(arguments, &["STRINGLITERALSINGLE"])?;
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
            let external = edge.extra.get("external").and_then(|value| value.as_bool());
            if edge.kind != "CALLS" || edge.target.contains("::") || external == Some(true) {
                return edge;
            }
            // A method of a value of another file's type, or of an unknown
            // one, is none of this file's functions.
            if edge.extra["receiver_unknown"] == true || edge.extra.get("receiver_type").is_some() {
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
