use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::member_calls::{BindingsSnapshot, CallOrigin, MemberCallBindings};
use super::stdlib::swift::{is_swift_stdlib_name, swift_framework_of, swift_system_module};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child, first_descendant, first_descendant_text, last_descendant_text, line_count,
    line_of, node_text, strip_matching_quotes, type_name_without_arguments,
};
use super::{is_test_function, qualify, resolve_rust_call_targets};

pub(super) fn parse_swift_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "swift")];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let root = tree.root_node();
        let mut type_names = HashSet::new();
        let mut fields = HashMap::new();
        swift_collect_types_and_fields(root, source, &mut type_names, &mut fields);
        let context = SwiftParseContext {
            source,
            file_path: file_path.clone(),
            bindings: RefCell::new(MemberCallBindings::with_types(type_names.clone())),
            type_names,
            fields,
            locals: RefCell::new(HashSet::new()),
        };
        swift_walk_children(root, &context, None, None, &mut nodes, &mut edges);
        swift_finish_typed_receivers(&context, &nodes, &mut edges);
        swift_mark_stdlib_edges(root, source, &nodes, &mut edges);
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }
    (nodes, edges)
}

struct SwiftParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    /// The types the file declares (classes, structs, enums, protocols,
    /// extensions' types, type aliases).
    type_names: HashSet<String>,
    /// Type name -> stored property -> its type as written (`let store:
    /// Store`, `var repo = Repo()`), which types `store.save()` and
    /// `self.store.save()` in the type's methods.
    fields: HashMap<String, HashMap<String, String>>,
    bindings: RefCell<MemberCallBindings>,
    /// Names the enclosing function declares (parameters, `let` / `var`,
    /// closure parameters): a receiver of these is a variable.
    locals: RefCell<HashSet<String>>,
}

fn swift_walk_children(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
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
            "import_declaration" if enclosing_class.is_none() && enclosing_func.is_none() => {
                if let Some(target) = swift_import_target(child, context.source) {
                    edges.push(ParsedEdge::new(
                        crate::core::types::EdgeKind::ImportsFrom,
                        context.file_path.to_string(),
                        target,
                        context.file_path.clone(),
                        line_of(child),
                    ));
                }
                continue;
            }
            "class_declaration" | "protocol_declaration" => {
                if let Some(name) = swift_type_name(child, context.source) {
                    // Extensions reopen an existing type, so they stay unscoped.
                    let is_extension = swift_type_kind(child, context.source) == "extension";
                    let parent = if is_extension { None } else { owner() };
                    swift_emit_class(child, context, &name, parent, nodes, edges);
                    let path = match parent {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name.clone(),
                    };
                    swift_walk_children(child, context, Some(&path), None, nodes, edges);
                    continue;
                }
            }
            "function_declaration" | "protocol_function_declaration" => {
                if let Some(name) = swift_function_name(child, context.source) {
                    swift_emit_function(child, context, &name, owner(), nodes, edges);
                    let saved = swift_enter_function(child, context);
                    swift_walk_children(child, context, owner(), Some(&name), nodes, edges);
                    swift_leave_function(context, saved);
                    continue;
                }
            }
            "init_declaration" | "deinit_declaration" if enclosing_class.is_some() => {
                let name = if child.kind() == "init_declaration" {
                    "init"
                } else {
                    "deinit"
                };
                swift_emit_function(child, context, name, enclosing_class, nodes, edges);
                let saved = swift_enter_function(child, context);
                swift_walk_children(child, context, enclosing_class, Some(name), nodes, edges);
                swift_leave_function(context, saved);
                continue;
            }
            "property_declaration"
                if enclosing_func.is_none()
                    && direct_child(child, &["computed_property"]).is_some() =>
            {
                if let Some(name) = direct_child(child, &["pattern"])
                    .and_then(|pattern| direct_child(pattern, &["simple_identifier"]))
                    .map(|ident| node_text(ident, context.source))
                {
                    swift_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    let saved = swift_enter_function(child, context);
                    swift_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    swift_leave_function(context, saved);
                    continue;
                }
            }
            "property_declaration" | "guard_statement" | "if_statement"
                if enclosing_func.is_some() =>
            {
                swift_walk_children(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                );
                swift_bind_declaration(child, context);
                continue;
            }
            "call_expression" => {
                swift_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        swift_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn swift_emit_class(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    name: &str,
    parent: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let swift_kind = swift_type_kind(node, context.source);
    let qualified = qualify(&context.file_path, name, parent);
    let (type_role, extra_flags) = match swift_kind.as_str() {
        "protocol" => (
            "protocol",
            json!({"is_abstract": true, "is_contract": true}),
        ),
        "struct" => (
            "struct",
            json!({"container_role": "data_container", "value_semantics": true}),
        ),
        "enum" => (
            "enum",
            json!({"container_role": "data_container", "value_semantics": true}),
        ),
        _ => ("class", json!({})),
    };
    let mut extra = json!({"type_role": type_role, "swift_kind": swift_kind});
    if let (Some(extra_obj), Some(flags)) = (extra.as_object_mut(), extra_flags.as_object()) {
        for (key, value) in flags {
            extra_obj.insert(key.clone(), value.clone());
        }
    }
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "swift".to_string(),
        parent_name: parent.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        parent
            .map(|parent| qualify(&context.file_path, parent, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified.clone(),
        context.file_path.clone(),
        line_of(node),
    ));
    for base in swift_inheritance_targets(node, context.source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualified.clone(),
            target: base,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_declaration",
            }),
        });
    }
}

fn swift_emit_function(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
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
        language: "swift".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: swift_return_type(node, context.source),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualify(&context.file_path, name, enclosing_class),
        context.file_path.clone(),
        line_of(node),
    ));
}

fn swift_emit_call(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = match (enclosing_func, enclosing_class) {
        (Some(func), _) => qualify(&context.file_path, func, enclosing_class),
        (None, Some(class)) => qualify(&context.file_path, class, None),
        (None, None) => context.file_path.to_string(),
    };
    if let Some(call_name) = swift_call_name(node, context.source) {
        let mut extra = match swift_call_path(node, context.source) {
            Some(path) => json!({ SWIFT_CALL_PATH_KEY: path }),
            None => json!({}),
        };
        if let Some(callee) =
            swift_call_callee(node).filter(|callee| callee.kind() == "navigation_expression")
            && let Some(target) = callee.child_by_field_name("target")
        {
            // `open(p)?.save()`: optional chaining unwraps the receiver.
            let optional = direct_child(callee, &["?"]).is_some();
            match swift_receiver(target, &call_name, optional, enclosing_class, context) {
                SwiftReceiver::Typed(type_name) => {
                    extra[SWIFT_TYPED_RECEIVER_KEY] = json!(type_name);
                }
                SwiftReceiver::Unknown(origin) => {
                    extra["receiver_unknown"] = json!(true);
                    if let Some(origin) = origin {
                        extra["receiver_from"] = origin.to_json();
                    }
                }
                SwiftReceiver::Known => {}
            }
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target: call_name,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
    }
    if let Some(signature) = swift_call_signature(node, context.source)
        && let Some(edge) = swift_bridge_edge(node, context, &caller, &signature)
    {
        edges.push(edge);
    }
}

/// Scratch key on a `CALLS` edge: the type its receiver is declared with
/// (`Store` for `s.save()` after `let s: Store`), consumed by
/// [`swift_finish_typed_receivers`] once every method of the file is known.
const SWIFT_TYPED_RECEIVER_KEY: &str = "swift_typed_receiver";

/// What a member call's receiver says about the method it calls.
#[derive(Debug)]
enum SwiftReceiver {
    /// Declared with a type: `s: Store`, `let s = Store()`, a stored
    /// property `let store: Store` of the enclosing type.
    Typed(String),
    /// A variable or value of a type the file does not say, with the call
    /// it is the result of, if any (`try open(p).save()`, `let s = try
    /// open(p); s.save()`).
    Unknown(Option<CallOrigin>),
    /// `self`, `super`, a type, a module, or a name the function does not
    /// declare: left to resolution by name.
    Known,
}

/// Rewrites typed member calls once every method of the file is known: a
/// type of the standard library or of an imported system framework
/// (`let d = JSONDecoder()` after `import Foundation`) points `d.decode()`
/// at it (`JSONDecoder.decode`, likely); a method the file declares on the
/// type becomes `Type::method`, which same-file resolution binds; any
/// other keeps the bare method with `receiver_type: "Type"`, which
/// resolution across files matches (an extension in another file).
fn swift_finish_typed_receivers(
    context: &SwiftParseContext<'_>,
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
) {
    let methods = nodes
        .iter()
        .filter(|node| node.kind != crate::core::types::NodeKind::Class)
        .filter_map(|node| {
            let owner = node.parent_name.as_deref()?;
            let type_name = owner.rsplit('.').next().unwrap_or(owner);
            Some((type_name.to_string(), node.name.clone()))
        })
        .collect::<HashSet<_>>();
    let imported = edges
        .iter()
        .filter(|edge| edge.kind == crate::core::types::EdgeKind::ImportsFrom)
        .filter_map(|edge| swift_system_module(&edge.target))
        .collect::<Vec<_>>();
    for edge in edges.iter_mut() {
        let Some(type_name) = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove(SWIFT_TYPED_RECEIVER_KEY))
            .and_then(|value| value.as_str().map(str::to_string))
        else {
            continue;
        };
        let module = if context.type_names.contains(&type_name) {
            None
        } else if is_swift_stdlib_name(&type_name) {
            Some("Swift")
        } else {
            swift_framework_of(&type_name, &imported)
        };
        if let Some(module) = module {
            edge.extra
                .as_object_mut()
                .map(|extra| extra.remove(SWIFT_CALL_PATH_KEY));
            edge.target = format!("{type_name}.{}", edge.target);
            mark_stdlib_edge(
                &mut edge.target,
                &mut edge.extra,
                module,
                StdlibEvidence::Likely,
            );
        } else if methods.contains(&(type_name.clone(), edge.target.clone())) {
            edge.target = format!("{type_name}::{}", edge.target);
        } else {
            edge.extra["receiver_type"] = json!(type_name);
        }
    }
}

/// The declared return type as written: `-> Store?`, `-> [Int]`, or the
/// type of a computed property (`var total: Int { ... }`).
fn swift_return_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "property_declaration" {
        let annotation = direct_child(node, &["type_annotation"])?;
        let ty = annotation.named_child(0)?;
        return Some(node_text(ty, source));
    }
    let mut cursor = node.walk();
    let mut after_arrow = false;
    for child in node.children(&mut cursor) {
        if after_arrow && child.is_named() {
            return Some(node_text(child, source));
        }
        after_arrow |= child.kind() == "->";
    }
    None
}

/// The types the file declares and the stored properties of each (see
/// [`SwiftParseContext::fields`]).
fn swift_collect_types_and_fields(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    types: &mut HashSet<String>,
    fields: &mut HashMap<String, HashMap<String, String>>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "protocol_declaration" => {
                if let Some(name) = swift_type_name(child, source) {
                    types.insert(name.clone());
                    if let Some(body) = child.child_by_field_name("body") {
                        let mut members = body.walk();
                        for member in body.children(&mut members) {
                            if member.kind() != "property_declaration"
                                || direct_child(member, &["computed_property"]).is_some()
                            {
                                continue;
                            }
                            if let Some((var, type_name)) =
                                swift_property_type(member, source, types)
                            {
                                fields
                                    .entry(name.clone())
                                    .or_default()
                                    .insert(var, type_name);
                            }
                        }
                    }
                }
            }
            "typealias_declaration" => {
                if let Some(name) = direct_child(child, &["type_identifier"]) {
                    types.insert(node_text(name, source));
                }
            }
            _ => {}
        }
        swift_collect_types_and_fields(child, source, types, fields);
    }
}

/// `(name, type)` of a property declared with a type annotation (`let
/// store: Store`, `var repo: Repo?`) or an initializer (`var repo =
/// Repo()`).
fn swift_property_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    types: &HashSet<String>,
) -> Option<(String, String)> {
    let var = direct_child(node, &["pattern"])
        .and_then(|pattern| direct_child(pattern, &["simple_identifier"]))
        .map(|ident| node_text(ident, source))?;
    let type_name = match direct_child(node, &["type_annotation"]) {
        Some(annotation) => swift_named_type(annotation.named_child(0)?, source)?,
        None => swift_initialized_type(node.child_by_field_name("value")?, source, types)?,
    };
    Some((var, type_name))
}

/// The type a type expression names, when its methods are the value's:
/// `Store`, `Store?`, `Store!`, `Box<T>` (`Box`), `Foundation.Data`
/// (`Data`). Arrays, dictionaries, tuples, and functions name none.
fn swift_named_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "user_type" => {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .filter(|child| child.kind() == "type_identifier")
                .last()
                .map(|name| node_text(name, source))
        }
        "optional_type" | "implicitly_unwrapped_type" => {
            swift_named_type(node.named_child(0)?, source)
        }
        _ => None,
    }
}

/// `T` for an initializer call `T(...)` of a type: one the file declares,
/// or a capitalized name (Swift's convention for types).
fn swift_initialized_type(
    value: tree_sitter::Node<'_>,
    source: &[u8],
    types: &HashSet<String>,
) -> Option<String> {
    if value.kind() != "call_expression" {
        return None;
    }
    let callee = swift_call_callee(value)?;
    if callee.kind() != "simple_identifier" {
        return None;
    }
    let name = node_text(callee, source);
    swift_is_type_name(&name, types).then_some(name)
}

fn swift_is_type_name(name: &str, types: &HashSet<String>) -> bool {
    types.contains(name) || name.starts_with(|c: char| c.is_ascii_uppercase())
}

/// The bindings and locals of the enclosing scope, restored by
/// [`swift_leave_function`].
struct SwiftSavedScope {
    bindings: BindingsSnapshot,
    locals: HashSet<String>,
}

/// Enters a function, initializer, or computed property: binds its
/// parameters to their types (`func run(_ store: Store)`) and records the
/// names it declares as locals.
fn swift_enter_function(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
) -> SwiftSavedScope {
    let saved = SwiftSavedScope {
        bindings: context.bindings.borrow().snapshot(),
        locals: context.locals.borrow().clone(),
    };
    let mut locals = HashSet::new();
    swift_collect_declared_names(node, context.source, &mut locals);
    swift_collect_bound_identifiers(node, context.source, &mut locals);
    {
        let mut bindings = context.bindings.borrow_mut();
        for name in &locals {
            bindings.forget_foreign(name);
        }
    }
    let mut cursor = node.walk();
    for parameter in node.children(&mut cursor) {
        if parameter.kind() != "parameter" {
            continue;
        }
        let mut var = None;
        let mut type_name = None;
        let mut names = parameter.walk();
        for part in parameter.children_by_field_name("name", &mut names) {
            if part.kind() == "simple_identifier" {
                var = Some(node_text(part, context.source));
            } else {
                type_name = swift_named_type(part, context.source);
            }
        }
        if let (Some(var), Some(type_name)) = (var, type_name) {
            context.bindings.borrow_mut().bind_any(var, type_name);
        }
    }
    *context.locals.borrow_mut() = locals;
    saved
}

fn swift_leave_function(context: &SwiftParseContext<'_>, saved: SwiftSavedScope) {
    context.bindings.borrow_mut().restore(saved.bindings);
    *context.locals.borrow_mut() = saved.locals;
}

/// Names bound by `if let` / `guard let` (`guard let s = try? open()`).
fn swift_collect_bound_identifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut HashSet<String>,
) {
    if let Some(name) = node.child_by_field_name("bound_identifier") {
        out.insert(node_text(name, source));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        swift_collect_bound_identifiers(child, source, out);
    }
}

/// Binds the variable a local declaration gives a value: `let s: Store` and
/// `let s = Store()` to the type; `let s = try open(p)`, `guard let s =
/// try? open(p)` to the call; anything else drops what it was bound to.
fn swift_bind_declaration(node: tree_sitter::Node<'_>, context: &SwiftParseContext<'_>) {
    let source = context.source;
    let (var, value) = if node.kind() == "property_declaration" {
        let Some(var) = direct_child(node, &["pattern"])
            .and_then(|pattern| direct_child(pattern, &["simple_identifier"]))
        else {
            return;
        };
        (var, node.child_by_field_name("value"))
    } else {
        let Some(var) = node.child_by_field_name("bound_identifier") else {
            return;
        };
        // The condition after `=`.
        let mut cursor = node.walk();
        let value = node
            .children_by_field_name("condition", &mut cursor)
            .skip_while(|condition| condition.kind() != "=")
            .nth(1);
        (var, value)
    };
    let var = node_text(var, source);
    if node.kind() == "property_declaration"
        && let Some((_, type_name)) = swift_property_type(node, source, &context.type_names)
    {
        context.bindings.borrow_mut().bind_any(var, type_name);
        return;
    }
    context.bindings.borrow_mut().forget_foreign(&var);
    if let Some(origin) = value.and_then(|value| swift_call_origin(value, None, false, context)) {
        context.bindings.borrow_mut().bind_returned(var, origin);
    }
}

/// The call `value` is the result of, or of which a variable holds the
/// result, unwrapped by `try`, `!`, or `?` on the way. A chain repeating
/// `method` (`b.x(1).x(2)`) points past the repeats, as the calls of one
/// line to one method are one edge.
fn swift_call_origin(
    value: tree_sitter::Node<'_>,
    method: Option<&str>,
    unwrap: bool,
    context: &SwiftParseContext<'_>,
) -> Option<CallOrigin> {
    let source = context.source;
    match value.kind() {
        "call_expression" => {
            let name = swift_call_name(value, source)?;
            if method == Some(name.as_str())
                && let Some(callee) = swift_call_callee(value)
                    .filter(|callee| callee.kind() == "navigation_expression")
                && let Some(receiver) = callee.child_by_field_name("target")
            {
                let unwrap = direct_child(callee, &["?"]).is_some();
                return swift_call_origin(receiver, method, unwrap, context);
            }
            Some(CallOrigin {
                name,
                line: value.start_position().row as i64 + 1,
                unwrap,
            })
        }
        "try_expression" => {
            swift_call_origin(value.child_by_field_name("expr")?, method, true, context)
        }
        "await_expression" => {
            swift_call_origin(value.child_by_field_name("expr")?, method, unwrap, context)
        }
        "postfix_expression" => {
            let bang = value
                .child_by_field_name("operation")
                .is_some_and(|operation| operation.kind() == "bang");
            swift_call_origin(
                value.child_by_field_name("target")?,
                method,
                unwrap || bang,
                context,
            )
        }
        "tuple_expression" | "parenthesized_expression" if value.named_child_count() == 1 => {
            swift_call_origin(value.named_child(0)?, method, unwrap, context)
        }
        "simple_identifier" => {
            let name = node_text(value, source);
            if !context.locals.borrow().contains(&name) {
                return None;
            }
            let mut origin = context.bindings.borrow().returned_by(&name).cloned()?;
            origin.unwrap |= unwrap;
            Some(origin)
        }
        _ => None,
    }
}

/// Classifies the receiver of a call to `method` (see [`SwiftReceiver`]).
fn swift_receiver(
    target: tree_sitter::Node<'_>,
    method: &str,
    optional: bool,
    enclosing_class: Option<&str>,
    context: &SwiftParseContext<'_>,
) -> SwiftReceiver {
    let source = context.source;
    let field_type = |name: &str| {
        let class = enclosing_class?;
        let class = class.rsplit('.').next().unwrap_or(class);
        context.fields.get(class)?.get(name).cloned()
    };
    match target.kind() {
        "simple_identifier" => {
            let name = node_text(target, source);
            if context.locals.borrow().contains(&name) {
                let bindings = context.bindings.borrow();
                if let Some(type_name) = bindings
                    .bound_type(&name)
                    .or_else(|| bindings.foreign_type(&name))
                {
                    return SwiftReceiver::Typed(type_name.to_string());
                }
                drop(bindings);
                return SwiftReceiver::Unknown(swift_call_origin(target, None, optional, context));
            }
            match field_type(&name) {
                Some(type_name) => SwiftReceiver::Typed(type_name),
                None => SwiftReceiver::Known,
            }
        }
        "navigation_expression" => {
            // `self.store.save()`: the stored property's type.
            let Some(object) = target.child_by_field_name("target") else {
                return SwiftReceiver::Known;
            };
            if object.kind() == "self_expression" {
                let property = target
                    .child_by_field_name("suffix")
                    .and_then(|suffix| suffix.child_by_field_name("suffix"))
                    .map(|name| node_text(name, source));
                return match property.and_then(|name| field_type(&name)) {
                    Some(type_name) => SwiftReceiver::Typed(type_name),
                    None => SwiftReceiver::Known,
                };
            }
            if swift_is_local_value(object, context) {
                SwiftReceiver::Unknown(None)
            } else {
                SwiftReceiver::Known
            }
        }
        "call_expression" => {
            if let Some(type_name) = swift_initialized_type(target, source, &context.type_names) {
                return SwiftReceiver::Typed(type_name);
            }
            SwiftReceiver::Unknown(swift_call_origin(target, Some(method), optional, context))
        }
        "try_expression" | "await_expression" | "postfix_expression" | "tuple_expression" => {
            match swift_call_origin(target, Some(method), optional, context) {
                Some(origin) => SwiftReceiver::Unknown(Some(origin)),
                None if swift_is_local_value(target, context) => SwiftReceiver::Unknown(None),
                None => SwiftReceiver::Known,
            }
        }
        _ => SwiftReceiver::Known,
    }
}

/// Whether an expression is rooted at a value of the function: a local
/// variable (`items.first`, `s!`) or a call's result.
fn swift_is_local_value(node: tree_sitter::Node<'_>, context: &SwiftParseContext<'_>) -> bool {
    match node.kind() {
        "simple_identifier" => context
            .locals
            .borrow()
            .contains(&node_text(node, context.source)),
        "call_expression" => swift_call_callee(node).is_some_and(|callee| {
            callee.kind() != "simple_identifier"
                || !swift_is_type_name(&node_text(callee, context.source), &context.type_names)
                || swift_is_local_value(callee, context)
        }),
        "self_expression" | "super_expression" => false,
        _ => node
            .named_child(0)
            .is_some_and(|first| swift_is_local_value(first, context)),
    }
}

/// Scratch key on a `CALLS` edge: the callee as written from a plain name
/// (`print`, `Swift.print`, `FileManager.default.fileExists`), consumed by
/// [`swift_mark_stdlib_edges`].
const SWIFT_CALL_PATH_KEY: &str = "swift_call_path";

/// The callee of a call as a dotted path rooted at a plain name, or `None`
/// when it starts anywhere else (`JSONDecoder().decode`, `self.save`).
fn swift_call_path(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    fn path(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
        match node.kind() {
            "simple_identifier" => Some(node_text(node, source)),
            "navigation_expression" => {
                let target = node.named_child(0)?;
                let suffix = direct_child(node, &["navigation_suffix"])?;
                let member = direct_child(suffix, &["simple_identifier"])?;
                Some(format!(
                    "{}.{}",
                    path(target, source)?,
                    node_text(member, source)
                ))
            }
            _ => None,
        }
    }
    path(swift_call_callee(node)?, source)
}

/// Names the file declares: its types and functions (`nodes`), and its
/// constants, variables, parameters, closure parameters, loop variables,
/// and type aliases.
fn swift_collect_declared_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "pattern" | "lambda_parameter" => {
            if let Some(name) = direct_child(node, &["simple_identifier"]) {
                names.insert(node_text(name, source));
            }
        }
        // `_ user: String`, `count n: Int`: both the label and the name.
        "parameter" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "simple_identifier" => {
                        names.insert(node_text(child, source));
                    }
                    ":" => break,
                    _ => {}
                }
            }
        }
        "typealias_declaration" => {
            if let Some(name) = direct_child(node, &["type_identifier"]) {
                names.insert(node_text(name, source));
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        swift_collect_declared_names(child, source, names);
    }
}

/// Points the calls and imports that reach the Swift standard library or a
/// system framework at it: `import Foundation` (certain), a call qualified
/// by the module (`Swift.print(x)`, `Foundation.Date()`, certain), a
/// standard-library name (`print(x)`, `String(n)`, `Array(repeating:)`,
/// likely), and a framework name the file's imports bring into scope
/// (`Date()`, `FileManager.default.fileExists(...)` after
/// `import Foundation`, likely). A name the file declares is its own.
fn swift_mark_stdlib_edges(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
) {
    let mut declared = nodes
        .iter()
        .filter(|node| node.kind != crate::core::types::NodeKind::File)
        .map(|node| node.name.clone())
        .collect::<HashSet<_>>();
    swift_collect_declared_names(root, source, &mut declared);
    let imported = edges
        .iter()
        .filter(|edge| edge.kind == crate::core::types::EdgeKind::ImportsFrom)
        .filter_map(|edge| swift_system_module(&edge.target))
        .collect::<Vec<_>>();
    for edge in edges.iter_mut() {
        match edge.kind {
            crate::core::types::EdgeKind::ImportsFrom => {
                if let Some(module) = swift_system_module(&edge.target) {
                    mark_stdlib_edge(
                        &mut edge.target,
                        &mut edge.extra,
                        module,
                        StdlibEvidence::Certain,
                    );
                }
            }
            crate::core::types::EdgeKind::Calls => {
                let Some(path) = edge
                    .extra
                    .as_object_mut()
                    .and_then(|extra| extra.remove(SWIFT_CALL_PATH_KEY))
                    .and_then(|path| path.as_str().map(str::to_string))
                else {
                    continue;
                };
                let root = path.split('.').next().unwrap_or_default();
                if declared.contains(root) {
                    continue;
                }
                let qualified_by_module = path.contains('.')
                    && (root == "Swift" || imported.contains(&root))
                    && swift_system_module(root) == Some(root);
                let found = if qualified_by_module {
                    swift_system_module(root).map(|module| (module, StdlibEvidence::Certain))
                } else if is_swift_stdlib_name(root) {
                    Some(("Swift", StdlibEvidence::Likely))
                } else {
                    swift_framework_of(root, &imported)
                        .map(|module| (module, StdlibEvidence::Likely))
                };
                if let Some((module, evidence)) = found {
                    edge.target = path;
                    mark_stdlib_edge(&mut edge.target, &mut edge.extra, module, evidence);
                }
            }
            _ => {}
        }
    }
}

fn swift_type_kind(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if node.kind() == "protocol_declaration" {
        return "protocol".to_string();
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = node_text(child, source);
        if matches!(
            text.as_str(),
            "class" | "struct" | "enum" | "actor" | "extension"
        ) {
            return text;
        }
    }
    "class".to_string()
}

/// The imported module path, without the `import` keyword or a kind specifier.
///
/// The whole statement used to be the target, so `import Foundation` could not
/// be compared against any module or file name.
fn swift_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let path = direct_child(node, &["identifier"])?;
    let target: String = node_text(path, source).split_whitespace().collect();
    (!target.is_empty()).then_some(target)
}

fn swift_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    direct_child(node, &["type_identifier"])
        .map(|child| node_text(child, source))
        .or_else(|| first_descendant_text(node, source, &["type_identifier"]))
}

fn swift_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    direct_child(node, &["simple_identifier"]).map(|child| node_text(child, source))
}

fn swift_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.kind() == "inheritance_specifier")
        .filter_map(|specifier| {
            let base = specifier
                .child_by_field_name("inherits_from")
                .unwrap_or(specifier);
            type_name_without_arguments(base, source)
        })
        .collect()
}

fn swift_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = swift_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(callee, source)),
        "navigation_expression" => last_descendant_text(callee, source, &["simple_identifier"]),
        _ => None,
    }
}

fn swift_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = swift_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(callee, source)),
        "navigation_expression" => {
            let parts = swift_descendant_texts(callee, source, &["simple_identifier"]);
            (!parts.is_empty()).then(|| parts.join("."))
        }
        _ => None,
    }
}

fn swift_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "call_suffix")
}

fn swift_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Process.run" => ("invokes_binary", "subprocess"),
        "String.contentsOf" | "Data.contentsOf" | "FileManager.contentsOfFile" => {
            ("reads_file", "file_io")
        }
        "FileManager.createFile" => ("writes_file", "file_io"),
        "dlopen" | "Bundle.load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match swift_first_string_arg(node, context.source) {
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
            "source_language": "swift",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn swift_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let args = first_descendant(node, &["value_arguments"])?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() != "value_argument" {
            continue;
        }
        let mut value_cursor = child.walk();
        for value in child.children(&mut value_cursor) {
            if value.kind() == "line_string_literal" {
                return Some(swift_string_text(value, source));
            }
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn swift_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = node_text(node, source);
    strip_matching_quotes(text.trim()).to_string()
}

fn swift_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    swift_collect_descendant_texts(node, source, kinds, &mut out);
    out
}

fn swift_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    out: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
        swift_collect_descendant_texts(child, source, kinds, out);
    }
}
