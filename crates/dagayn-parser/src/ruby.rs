use std::cell::RefCell;
use std::collections::HashSet;

use serde_json::{Value, json};

use super::member_calls::{CallOrigin, MemberCallBindings};

use super::stdlib::ruby::{
    is_ruby_core_constant, is_ruby_kernel_method, is_ruby_stdlib_library, ruby_library_root,
    ruby_stdlib_constant_library,
};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_ruby_with_parser(
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
        language: "ruby".to_string(),
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
        let mut class_names = HashSet::new();
        ruby_collect_class_names(tree.root_node(), source, &mut class_names);
        let context = RubyContext {
            source,
            file_path: &file_path,
            bindings: RefCell::new(MemberCallBindings::with_types(class_names.clone())),
            class_names,
        };
        ruby_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        ruby_mark_stdlib_calls(tree.root_node(), source, &nodes, &mut edges);
        ruby_resolve_local_receivers(&nodes, &mut edges);
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

/// What the walk of one file shares: the source, and the types of the
/// variables in scope (`store = Store.new`), which type member calls.
struct RubyContext<'a> {
    source: &'a [u8],
    file_path: &'a FilePath,
    bindings: RefCell<MemberCallBindings>,
    /// Classes and modules the file declares, by name.
    class_names: HashSet<String>,
}

fn ruby_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RubyContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "module" | "class" => {
                if let Some(name) = ruby_class_name(child, source) {
                    ruby_emit_class(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    let path = match enclosing_class {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name.clone(),
                    };
                    let saved = context.bindings.borrow().snapshot();
                    ruby_walk_children(child, context, Some(&path), None, nodes, edges);
                    context.bindings.borrow_mut().restore(saved);
                    continue;
                }
            }
            "method" | "singleton_method" => {
                if let Some(name) = ruby_method_name(child, source) {
                    ruby_emit_function(child, file_path, &name, enclosing_class, nodes, edges);
                    let saved = context.bindings.borrow().snapshot();
                    ruby_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    context.bindings.borrow_mut().restore(saved);
                    continue;
                }
            }
            "call" | "method_call" => {
                if enclosing_func.is_none()
                    && let Some(class) = enclosing_class
                {
                    ruby_emit_attached_function(child, source, file_path, class, nodes, edges);
                }
                ruby_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "assignment" => {
                // The value is walked first: `store = store.reload` calls
                // `reload` on the previous `store`.
                ruby_walk_children(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                );
                ruby_bind_assignment(child, context);
                continue;
            }
            _ => {}
        }
        ruby_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn ruby_emit_class(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "ruby".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": node.kind()}),
    });
    let qualified = qualify(file_path, name, enclosing_class);
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
    if let Some(superclass) = ruby_direct_child(node, &["superclass"])
        && let Some(target) = ruby_constant_name(superclass, source)
    {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualified.clone(),
            target,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({"relationship_role": "extends", "syntax_source": "superclass"}),
        });
    }
    let Some(body) = ruby_direct_child(node, &["body_statement"]) else {
        return;
    };
    let mut cursor = body.walk();
    for statement in body.children(&mut cursor) {
        if statement.kind() != "call" {
            continue;
        }
        let Some(keyword) = ruby_call_name(statement, source) else {
            continue;
        };
        if !matches!(keyword.as_str(), "include" | "extend" | "prepend") {
            continue;
        }
        let Some(arguments) = ruby_direct_child(statement, &["argument_list"]) else {
            continue;
        };
        let mut args = arguments.walk();
        for argument in arguments.children(&mut args) {
            if let Some(target) = ruby_constant_name(argument, source) {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::Inherits,
                    source: qualified.clone(),
                    target,
                    file_path: file_path.clone(),
                    line: statement.start_position().row as i64 + 1,
                    extra: json!({"relationship_role": "mixin", "syntax_source": keyword}),
                });
            }
        }
    }
}

fn ruby_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

/// The unqualified name of a `constant` or `A::B` scope resolution, looking
/// through a wrapper such as `superclass`.
fn ruby_constant_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "constant" => Some(node_text(node, source).trim().to_string()),
        "scope_resolution" => node
            .child_by_field_name("name")
            .map(|name| node_text(name, source).trim().to_string()),
        "superclass" => {
            let mut cursor = node.walk();
            let inner = node
                .children(&mut cursor)
                .find(|child| matches!(child.kind(), "constant" | "scope_resolution"))?;
            ruby_constant_name(inner, source)
        }
        _ => None,
    }
}

/// Class-body DSL keywords that declare structure rather than call code.
fn ruby_is_declarative_call(name: &str) -> bool {
    matches!(
        name,
        "require"
            | "require_relative"
            | "include"
            | "extend"
            | "prepend"
            | "attr_accessor"
            | "attr_reader"
            | "attr_writer"
            | "private"
            | "public"
            | "protected"
            | "module_function"
    )
}

fn ruby_emit_function(
    node: tree_sitter::Node<'_>,
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
        language: "ruby".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
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

/// The ffi gem: `attach_function :name, [...], :ret` (or
/// `attach_function :name, :c_symbol, [...], :ret`) in a module that
/// `extend FFI::Library` defines the module method `name`, bound to the C
/// symbol in the library `ffi_lib "lib"` names (`ffi_import`).
fn ruby_emit_attached_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    class: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    if ruby_call_name(node, source).as_deref() != Some("attach_function") {
        return;
    }
    let names = ruby_leading_symbol_args(node, source);
    let Some(name) = names.first() else {
        return;
    };
    let symbol = names.get(1).unwrap_or(name);
    let mut import = json!({"abi": "c", "name": symbol});
    if let Some(library) = ruby_ffi_library(node, source) {
        import["library"] = json!(library);
    }
    ruby_emit_function(node, file_path, name, Some(class), nodes, edges);
    if let Some(last) = nodes.last_mut() {
        last.extra = json!({"ffi_import": import});
    }
}

/// Leading `:symbol` / `"string"` arguments of a call, as written.
fn ruby_leading_symbol_args(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = arguments.walk();
    let mut names = Vec::new();
    for argument in arguments.named_children(&mut cursor) {
        match argument.kind() {
            "simple_symbol" => names.push(
                node_text(argument, source)
                    .trim_start_matches(':')
                    .to_string(),
            ),
            "string" => names.push(ruby_string_text(argument, source)),
            _ => break,
        }
    }
    names
}

/// The `ffi_lib "lib"` of the module or class enclosing *node*.
fn ruby_ffi_library(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut container = node.parent();
    while let Some(current) = container {
        if matches!(current.kind(), "module" | "class") {
            break;
        }
        container = current.parent();
    }
    let body = container?.child_by_field_name("body")?;
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|statement| statement.kind() == "call")
        .find(|statement| ruby_call_name(*statement, source).as_deref() == Some("ffi_lib"))
        .and_then(|statement| {
            ruby_leading_symbol_args(statement, source)
                .into_iter()
                .next()
        })
}

fn ruby_emit_call(
    node: tree_sitter::Node<'_>,
    context: &RubyContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    let call_name = ruby_call_name(node, source);
    let caller = match (enclosing_func, enclosing_class) {
        (Some(func), _) => qualify(file_path, func, enclosing_class),
        (None, Some(class)) => qualify(file_path, class, None),
        (None, None) => file_path.to_string(),
    };
    if let Some(call_name) = call_name {
        if (call_name == "require" || call_name == "require_relative")
            && let Some(target) = ruby_first_string_arg(node, source)
        {
            let mut edge = ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom,
                source: file_path.to_string(),
                target,
                file_path: file_path.clone(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            };
            // `require 'json'` loads a library shipped with Ruby;
            // `require_relative` always names a file of this repository.
            if call_name == "require" && is_ruby_stdlib_library(&edge.target) {
                let package = edge.target.clone();
                mark_stdlib_edge(
                    &mut edge.target,
                    &mut edge.extra,
                    &package,
                    StdlibEvidence::Certain,
                );
            }
            edges.push(edge);
        }
        if ruby_is_declarative_call(&call_name) {
            return;
        }
        let receiver = node.child_by_field_name("receiver");
        let mut extra = json!({});
        if let Some(receiver) = receiver {
            ruby_mark_receiver(receiver, &call_name, context, &mut extra);
        }
        // Read (and dropped) by `ruby_mark_stdlib_calls`.
        match receiver.map(|receiver| ruby_stdlib_receiver(receiver, context)) {
            None => extra["stdlib_bare"] = json!(true),
            Some(Some(path)) => extra["stdlib_receiver"] = json!(path),
            Some(None) => {}
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target: call_name,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
    }
    if let Some(signature) = ruby_call_signature(node, source)
        && let Some(edge) = ruby_bridge_edge(node, source, file_path, &caller, &signature)
    {
        edges.push(edge);
    }
}

/// The constant a call's receiver names, as the standard-library lookup
/// reads it: `JSON`, `Net::HTTP` (and `::File` as `File`), or `Set.new` for
/// a value the constant constructed, just now (`Set.new(xs).include?(x)`)
/// or into a variable (`set = Set.new(xs)`, then `set.include?(x)`). Any
/// other expression names nothing.
fn ruby_stdlib_receiver(
    receiver: tree_sitter::Node<'_>,
    context: &RubyContext<'_>,
) -> Option<String> {
    let source = context.source;
    match receiver.kind() {
        "constant" | "scope_resolution" => Some(ruby_constant_path(receiver, source)),
        "call" => {
            let method = receiver.child_by_field_name("method")?;
            if node_text(method, source) != "new" {
                return None;
            }
            let inner = receiver.child_by_field_name("receiver")?;
            if !matches!(inner.kind(), "constant" | "scope_resolution") {
                return None;
            }
            Some(format!("{}.new", ruby_constant_path(inner, source)))
        }
        "identifier" => context
            .bindings
            .borrow()
            .foreign_type(&node_text(receiver, source))
            .map(|path| format!("{path}.new")),
        _ => None,
    }
}

/// A constant as written, without a leading `::` (`::File` is `File`).
fn ruby_constant_path(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    node_text(node, source)
        .trim()
        .trim_start_matches("::")
        .to_string()
}

/// The type of a call's receiver, as far as the file says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RubyReceiverType {
    /// A class or module this file declares (`Repo.create`, or `repo`
    /// after `repo = Repo.new`), by name.
    Local(String),
    /// A constant of another file or of the standard library, by its path
    /// as written (`Net::HTTP`, `Store`).
    Other(String),
}

/// What `receiver` is: a constant (`Fast`, `A::B`), a value a constant
/// just constructed (`Store.new(x)`), or a variable bound to one. A
/// constant of this file is `Local`, any other `Other`.
fn ruby_receiver_type(
    receiver: tree_sitter::Node<'_>,
    context: &RubyContext<'_>,
) -> Option<RubyReceiverType> {
    let source = context.source;
    let of_constant = |constant: tree_sitter::Node<'_>| {
        let path = ruby_constant_path(constant, source);
        let last = path.rsplit("::").next().unwrap_or(&path).to_string();
        if context.class_names.contains(&last) {
            RubyReceiverType::Local(last)
        } else {
            RubyReceiverType::Other(path)
        }
    };
    match receiver.kind() {
        "constant" | "scope_resolution" => Some(of_constant(receiver)),
        "call" => {
            let method = receiver.child_by_field_name("method")?;
            let inner = receiver.child_by_field_name("receiver")?;
            (node_text(method, source) == "new"
                && matches!(inner.kind(), "constant" | "scope_resolution"))
            .then(|| of_constant(inner))
        }
        "identifier" => {
            let name = node_text(receiver, source);
            let bindings = context.bindings.borrow();
            if let Some(bound) = bindings.bound_type(&name) {
                return Some(RubyReceiverType::Local(bound.to_string()));
            }
            bindings
                .foreign_type(&name)
                .map(|path| RubyReceiverType::Other(path.to_string()))
        }
        _ => None,
    }
}

/// Scratch key on a `CALLS` edge: the class of this file its receiver is
/// (or is an instance of), consumed by [`ruby_resolve_local_receivers`].
const RUBY_LOCAL_TYPE_KEY: &str = "ruby_local_type";

/// Records what a method call's receiver says: its class or module when
/// the file says (`Fast.fast_sum` / `store = Store.new; store.save`:
/// `receiver_type: "Fast"` / `"Store"` for a constant of another file),
/// or `receiver_unknown` for any other value (`user.save`, `@db.query`,
/// `find(id).save`), with the call it came from when it is a call's
/// result (`receiver_from`), so no same-named method of the file is taken
/// for it. `self` and `super` are the enclosing class.
fn ruby_mark_receiver(
    receiver: tree_sitter::Node<'_>,
    method: &str,
    context: &RubyContext<'_>,
    extra: &mut Value,
) {
    match ruby_receiver_type(receiver, context) {
        Some(RubyReceiverType::Local(type_name)) => {
            extra[RUBY_LOCAL_TYPE_KEY] = json!(type_name);
        }
        Some(RubyReceiverType::Other(path)) => {
            extra["receiver_type"] = json!(path.rsplit("::").next().unwrap_or(&path));
        }
        None if matches!(receiver.kind(), "self" | "super") => {}
        None => {
            extra["receiver_unknown"] = json!(true);
            if let Some(origin) = ruby_call_origin(receiver, Some(method), context) {
                extra["receiver_from"] = origin.to_json();
            }
        }
    }
}

/// The call an expression is the result of (`find(id)`, `repo.find(id)`),
/// or of which a variable holds the result (`user = find(id)`). In a chain
/// repeating `method` (`q.where(a).where(b)`) it is the call before the
/// repeats, since the repeats share one edge per line.
fn ruby_call_origin(
    expression: tree_sitter::Node<'_>,
    method: Option<&str>,
    context: &RubyContext<'_>,
) -> Option<CallOrigin> {
    match expression.kind() {
        "call" => {
            let name = ruby_call_name(expression, context.source)?;
            if method == Some(name.as_str()) {
                let inner = expression.child_by_field_name("receiver")?;
                return ruby_call_origin(inner, method, context);
            }
            Some(CallOrigin {
                name,
                line: expression.start_position().row as i64 + 1,
                unwrap: false,
            })
        }
        "identifier" => context
            .bindings
            .borrow()
            .returned_by(&node_text(expression, context.source))
            .cloned(),
        _ => None,
    }
}

/// Binds the variable an assignment sets to what its value says: the class
/// it constructs (`store = Store.new`), the variable it copies, or the call
/// it is the result of (`user = find(id)`).
fn ruby_bind_assignment(node: tree_sitter::Node<'_>, context: &RubyContext<'_>) {
    let Some(left) = node
        .child_by_field_name("left")
        .filter(|left| left.kind() == "identifier")
    else {
        return;
    };
    let var = node_text(left, context.source);
    let Some(right) = node.child_by_field_name("right") else {
        context.bindings.borrow_mut().forget_foreign(&var);
        return;
    };
    let typed = matches!(right.kind(), "call" | "identifier")
        .then(|| ruby_receiver_type(right, context))
        .flatten();
    match typed {
        Some(RubyReceiverType::Local(type_name)) => {
            context.bindings.borrow_mut().bind(var, type_name);
        }
        Some(RubyReceiverType::Other(path)) => {
            context.bindings.borrow_mut().bind_any(var, path);
        }
        None => match ruby_call_origin(right, None, context) {
            Some(origin) => context.bindings.borrow_mut().bind_returned(var, origin),
            None => context.bindings.borrow_mut().forget_foreign(&var),
        },
    }
}

/// Points a call on a class of this file (`Repo.create`, `repo.save` after
/// `repo = Repo.new`) at `Repo::create` when the class defines the method,
/// which same-file resolution binds; a method it does not define (`new`,
/// one inherited from another file) keeps `receiver_type`.
fn ruby_resolve_local_receivers(nodes: &[ParsedNode], edges: &mut [ParsedEdge]) {
    for edge in edges.iter_mut() {
        let Some(type_name) = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove(RUBY_LOCAL_TYPE_KEY))
            .and_then(|value| value.as_str().map(str::to_string))
        else {
            continue;
        };
        let suffix = format!(".{type_name}");
        let defined = nodes.iter().any(|node| {
            node.kind == crate::core::types::NodeKind::Function
                && node.name == edge.target
                && node
                    .parent_name
                    .as_deref()
                    .is_some_and(|parent| parent == type_name || parent.ends_with(&suffix))
        });
        if defined {
            edge.target = format!("{type_name}::{}", edge.target);
        } else {
            edge.extra["receiver_type"] = json!(type_name);
        }
    }
}

/// Names of the classes and modules declared anywhere in the file.
fn ruby_collect_class_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    if matches!(node.kind(), "class" | "module")
        && let Some(name) = ruby_class_name(node, source)
    {
        names.insert(name);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        ruby_collect_class_names(child, source, names);
    }
}

/// Points the calls into Ruby's standard library at it: a call on a
/// constant of a library shipped with Ruby at that library (`JSON.parse` at
/// `json`; certain once the file requires it, likely otherwise, since
/// another file may), on a core class at `core` (`File.read`), and a bare
/// `Kernel` method at `core` (`puts`, likely: a superclass elsewhere may
/// define one). A constant or method this file defines is its own, and a
/// method called on a variable is left alone.
fn ruby_mark_stdlib_calls(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
) {
    let defined_methods = nodes
        .iter()
        .filter(|node| node.kind == crate::core::types::NodeKind::Function)
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    let mut defined_constants = nodes
        .iter()
        .filter(|node| node.kind == crate::core::types::NodeKind::Class)
        .map(|node| node.name.clone())
        .collect::<HashSet<_>>();
    ruby_collect_constant_assignments(root, source, &mut defined_constants);
    let mut required = edges
        .iter()
        .filter(|edge| {
            edge.kind == crate::core::types::EdgeKind::ImportsFrom && edge.extra["stdlib"] == true
        })
        .map(|edge| ruby_library_root(&edge.target).to_string())
        .collect::<HashSet<_>>();
    // `yaml` is Psych.
    if required.contains("yaml") {
        required.insert("psych".to_string());
    }
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::Calls {
            continue;
        }
        let Some(extra) = edge.extra.as_object_mut() else {
            continue;
        };
        let bare = extra.remove("stdlib_bare").is_some();
        let receiver = extra
            .remove("stdlib_receiver")
            .and_then(|value| value.as_str().map(str::to_string));
        let (package, evidence, symbol) = match receiver {
            Some(path) => {
                let constant = path.strip_suffix(".new").unwrap_or(&path);
                let root = constant.split("::").next().unwrap_or(constant);
                if defined_constants.contains(root) {
                    continue;
                }
                let symbol = format!("{path}.{}", edge.target);
                if let Some(library) = ruby_stdlib_constant_library(constant) {
                    let evidence = if required.contains(ruby_library_root(library)) {
                        StdlibEvidence::Certain
                    } else {
                        StdlibEvidence::Likely
                    };
                    (library, evidence, symbol)
                } else if is_ruby_core_constant(root) {
                    ("core", StdlibEvidence::Certain, symbol)
                } else {
                    continue;
                }
            }
            None if bare
                && is_ruby_kernel_method(&edge.target)
                && !defined_methods.contains(edge.target.as_str()) =>
            {
                ("core", StdlibEvidence::Likely, edge.target.clone())
            }
            None => continue,
        };
        edge.target = symbol;
        mark_stdlib_edge(&mut edge.target, &mut edge.extra, package, evidence);
    }
}

/// Constants assigned anywhere in the file (`Point = Struct.new(:x)`),
/// which shadow a standard-library constant of the same name.
fn ruby_collect_constant_assignments(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    constants: &mut HashSet<String>,
) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "assignment"
            && let Some(left) = node.child_by_field_name("left")
            && left.kind() == "constant"
        {
            constants.insert(node_text(left, source).trim().to_string());
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
}

fn ruby_class_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    ruby_constant_name(name, source)
}

fn ruby_method_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    ruby_direct_child_text(node, source, &["identifier"])
}

fn ruby_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    // `recv.name(...)` / `Const.name(...)`: the method, not the receiver
    // (the first child), which named `xs.size` a call to `xs` and dropped
    // `Fast.fast_sum` entirely.
    if let Some(method) = node.child_by_field_name("method") {
        return matches!(method.kind(), "identifier" | "constant")
            .then(|| node_text(method, source));
    }
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).find(|child| {
        !matches!(
            child.kind(),
            "argument_list" | "do_block" | "block" | "." | "::" | "&."
        )
    })?;
    matches!(first.kind(), "identifier").then(|| node_text(first, source))
}

fn ruby_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "argument_list" | "do_block" | "block") {
            break;
        }
        parts.push(node_text(child, source));
    }
    let signature = parts.join("").trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn ruby_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "exec" | "spawn" | "Kernel.system" | "Process.spawn" | "IO.popen"
        | "Open3.capture3" | "Open3.popen3" => ("invokes_binary", "subprocess"),
        "File.read" | "File.readlines" | "IO.read" => ("reads_file", "file_io"),
        "File.write" | "IO.write" => ("writes_file", "file_io"),
        "File.open" => ("opens_file", "file_io"),
        "Fiddle.dlopen" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match ruby_first_string_arg(node, source) {
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
            "source_language": "ruby",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn ruby_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(ruby_string_text(child, source));
        }
        return None;
    }
    None
}

fn ruby_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn ruby_direct_child_text(
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
