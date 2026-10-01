use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::json;

use super::member_calls::{BindingsSnapshot, CallOrigin, MemberCallBindings};
use super::stdlib::dart::{dart_library, dart_library_exports, is_dart_core_name};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    import_candidate_exists, is_test_file, line_count, node_text, resolve_import_path,
    strip_matching_quotes,
};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_dart_with_parser(
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
        language: "dart".to_string(),
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
        let root = tree.root_node();
        let mut type_names = HashSet::new();
        let mut fields = HashMap::new();
        dart_collect_types_and_fields(root, source, &mut type_names, &mut fields);
        let context = DartContext {
            source,
            file_path: &file_path,
            prefixes: dart_import_prefixes(root, source),
            bindings: RefCell::new(MemberCallBindings::with_types(type_names.clone())),
            type_names,
            fields,
            locals: RefCell::new(HashSet::new()),
        };
        dart_walk_children(root, &context, None, None, &mut nodes, &mut edges);
        dart_finish_typed_receivers(&context, &nodes, &mut edges);
        dart_resolve_import_targets(&mut edges, &file_path, repo_root);
        dart_mark_stdlib_edges(tree.root_node(), source, &nodes, &mut edges);
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

/// Rewrites import targets that name a file in this repository.
///
/// A Dart import is a URI, so the literal (`../util.dart`,
/// `package:myapp/util.dart`) matches no file in the graph. `dart:` URIs and
/// third-party packages have no file here and keep their literal form.
fn dart_resolve_import_targets(
    edges: &mut [ParsedEdge],
    file_path: &FilePath,
    repo_root: Option<&Path>,
) {
    for edge in edges
        .iter_mut()
        .filter(|edge| edge.kind == crate::core::types::EdgeKind::ImportsFrom)
    {
        if let Some(resolved) = dart_resolve_import(&edge.target, file_path, repo_root) {
            edge.target = resolved;
        }
    }
}

fn dart_resolve_import(
    literal: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    if let Some(rest) = literal.strip_prefix("package:") {
        let (package, path) = rest.split_once('/')?;
        // Only this repository's own package maps to a path here.
        let package_root = dart_package_root(package, file_path, repo_root)?;
        let candidate = format!("{package_root}lib/{path}");
        return import_candidate_exists(Path::new(&candidate), repo_root).then_some(candidate);
    }
    if literal.starts_with("dart:") || literal.contains(':') {
        return None;
    }
    resolve_import_path(literal, file_path, repo_root, &[], false)
}

/// The nearest ancestor directory whose `pubspec.yaml` declares *package*,
/// as a prefix ending in `/` (empty at the repository root).
fn dart_package_root(
    package: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    let mut current = Path::new(file_path).parent()?.to_path_buf();
    loop {
        let pubspec = current.join("pubspec.yaml");
        let full = repo_root
            .map(|root| root.join(&pubspec))
            .unwrap_or_else(|| pubspec.clone());
        if let Ok(text) = std::fs::read_to_string(&full)
            && dart_pubspec_name(&text).as_deref() == Some(package)
        {
            let prefix = current.to_string_lossy().replace('\\', "/");
            return Some(if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}/")
            });
        }
        if !current.pop() {
            return None;
        }
    }
}

fn dart_pubspec_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("name:"))
        .map(|name| name.trim().trim_matches(['"', '\''].as_ref()).to_string())
}

fn dart_walk_children<'tree>(
    node: tree_sitter::Node<'tree>,
    context: &DartContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    dart_emit_calls_from_children(node, context, enclosing_class, enclosing_func, edges);

    // A Dart body is a *sibling* of its signature rather than a child, so the
    // signature's name has to carry across to the following `function_body`.
    // Without it every call in the body was attributed to the file.
    let mut pending_func: Option<(String, usize, tree_sitter::Node<'tree>)> = None;
    // Declarations inside a function body are local to it: `a`'s `helper`
    // is `a.helper`, apart from `b.helper`.
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
            "import_or_export" => {
                dart_emit_import(child, source, file_path, edges);
            }
            "class_definition"
            | "mixin_declaration"
            | "enum_declaration"
            | "extension_declaration" => {
                if let Some(name) = dart_direct_child_text(child, source, &["identifier"]) {
                    dart_emit_type(child, source, file_path, &name, owner(), nodes, edges);
                    let path = match owner() {
                        Some(parent) => format!("{parent}.{name}"),
                        None => name.clone(),
                    };
                    dart_walk_children(child, context, Some(&path), None, nodes, edges);
                    continue;
                }
            }
            "function_signature" | "method_signature" | "declaration" => {
                if let Some((signature, name)) = dart_signature_name(child, source, enclosing_class)
                {
                    dart_emit_function(signature, source, file_path, &name, owner(), nodes, edges);
                    pending_func = Some((name, nodes.len() - 1, signature));
                    continue;
                }
            }
            "function_body" => {
                let current = pending_func.take();
                if let Some((_, index, _)) = current.as_ref() {
                    // The node spanned the signature line only until now.
                    nodes[*index].line_end = child.end_position().row as i64 + 1;
                }
                // The body of a function declared here runs under the owner()
                // its node was emitted with.
                let (class, func) = match current.as_ref() {
                    Some((name, _, _)) => (owner(), Some(name.as_str())),
                    None => (enclosing_class, enclosing_func),
                };
                let saved = current.as_ref().map(|(_, _, signature)| {
                    dart_enter_function(*signature, child, enclosing_func.is_some(), context)
                });
                dart_walk_children(child, context, class, func, nodes, edges);
                if let Some(saved) = saved {
                    dart_leave_function(context, saved);
                }
                continue;
            }
            _ => {}
        }
        pending_func = None;
        dart_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        if child.kind() == "initialized_variable_definition" && enclosing_func.is_some() {
            dart_bind_variable(child, context);
        }
    }
}

/// Returns the `function_signature` node and its declared name.
///
/// Class members wrap the signature in a `method_signature`. A constructor is
/// named after its class (`A`) or its named suffix (`A.named` -> `named`).
fn dart_signature_name<'tree>(
    node: tree_sitter::Node<'tree>,
    source: &[u8],
    enclosing_class: Option<&str>,
) -> Option<(tree_sitter::Node<'tree>, String)> {
    let signature = if node.kind() == "function_signature" {
        node
    } else {
        dart_direct_child(
            node,
            &[
                "function_signature",
                "getter_signature",
                "setter_signature",
                "constructor_signature",
                "factory_constructor_signature",
            ],
        )?
    };
    if matches!(
        signature.kind(),
        "constructor_signature" | "factory_constructor_signature"
    ) {
        enclosing_class?;
        let mut cursor = signature.walk();
        let name = signature
            .children(&mut cursor)
            .filter(|child| child.kind() == "identifier")
            .last()
            .map(|child| node_text(child, source).trim().to_string())?;
        return (!name.is_empty()).then_some((signature, name));
    }
    let name = signature
        .child_by_field_name("name")
        .map(|name| node_text(name, source).trim().to_string())
        .or_else(|| dart_direct_child_text(signature, source, &["identifier"]))?;
    (!name.is_empty()).then_some((signature, name))
}

fn dart_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = dart_first_descendant_text(node, source, &["string_literal"]) else {
        return;
    };
    let target = strip_matching_quotes(target.trim()).to_string();
    if target.is_empty() {
        return;
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom,
        source: file_path.to_string(),
        target,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract) = match node.kind() {
        "mixin_declaration" => ("mixin", false),
        "enum_declaration" => ("enum", false),
        "extension_declaration" => ("extension", false),
        _ if dart_has_direct_child_kind(node, "abstract") => ("abstract_class", true),
        _ => ("class", false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if dart_is_value_container(type_role) {
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
        language: "dart".to_string(),
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
    for (target, role) in dart_inheritance_targets(node, source) {
        edges.push(ParsedEdge {
            kind: if role == "implements" {
                crate::core::types::EdgeKind::Implements
            } else {
                crate::core::types::EdgeKind::Inherits
            },
            source: qualified.clone(),
            target,
            file_path: file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": role,
                "syntax_source": "class_definition",
            }),
        });
    }
}

fn dart_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "enum")
}

fn dart_emit_function(
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
        language: "dart".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: dart_direct_child_text(node, source, &["formal_parameter_list"]),
        return_type: dart_return_type(node, source, enclosing_class),
        modifiers: None,
        is_test: false,
        extra: match dart_native_symbol(node, source, name) {
            Some(symbol) => json!({"ffi_import": {"abi": "c", "name": symbol}}),
            None => json!({}),
        },
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

/// What the walk of one library carries: its types and their fields, its
/// import prefixes, and the variables in scope (their types, or the call
/// they hold the result of).
struct DartContext<'a> {
    source: &'a [u8],
    file_path: &'a FilePath,
    /// The classes, mixins, enums, and extensions the library declares.
    type_names: HashSet<String>,
    /// Class name -> field -> its type (`final Store store;`, `var repo =
    /// Repo();`), which types `store.save()` and `this.store.save()` in the
    /// class's methods.
    fields: HashMap<String, HashMap<String, String>>,
    /// Import prefixes (`m` in `import 'models.dart' as m`): `m.open()`
    /// names a library member, not a member of a value.
    prefixes: HashSet<String>,
    bindings: RefCell<MemberCallBindings>,
    /// Names the enclosing function declares (parameters, local variables,
    /// loop and `catch` variables): a receiver of these is a variable.
    locals: RefCell<HashSet<String>>,
}

/// The value of an expression chain so far (`store`, `this.store`,
/// `open(p)`), as a receiver of the next call.
#[derive(Debug, Clone)]
enum DartValue {
    /// No receiver: the chain starts with the called name (`save()`).
    Absent,
    /// A plain name: a local, a field, a type, an import prefix, or a
    /// top-level variable.
    Name(String),
    This,
    /// `this.name`.
    ThisField(String),
    /// The result of a call (`open(p)`, `await open(p)`), and the type it
    /// constructs when it calls one (`Repo()`, `m.Repo()`).
    Call(CallOrigin, Option<String>),
    /// A value of a type nothing says (`s.cfg`, `items[0]`).
    Unknown,
    /// A member of a type or a library (`Repo.shared`, `m.config`), or a
    /// value nothing is known about (`super`, a literal).
    Known,
}

/// What a member call's receiver says about the method it calls.
#[derive(Debug)]
enum DartReceiver {
    /// Declared with a type: `Store s`, `final s = Store()`, a field
    /// `final Store store` of the enclosing class.
    Typed(String),
    /// A variable or value of a type the library does not say, with the
    /// call it is the result of, if any (`open(p).save()`, `final s =
    /// await open(p); s.save()`).
    Unknown(Option<CallOrigin>),
    /// None, `this`, a type, an import prefix, or a name the function does
    /// not declare: left to resolution by name.
    Known,
}

fn dart_emit_calls_from_children(
    node: tree_sitter::Node<'_>,
    context: &DartContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    let caller = match (enclosing_func, enclosing_class) {
        (Some(func), _) => qualify(file_path, func, enclosing_class),
        (None, Some(class)) => qualify(file_path, class, None),
        (None, None) => file_path.to_string(),
    };
    let line = node.start_position().row as i64 + 1;
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    dart_eval_chain(&children, line, context, |call| {
        // `DynamicLibrary.open("libfastsum.so")` from dart:ffi.
        if matches!(&call.receiver, DartValue::Name(name) if name == "DynamicLibrary")
            && call.target == "open"
            && let Some(library) =
                dart_first_descendant_text(call.arguments, source, &["string_literal"])
        {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::CrossArtifact,
                source: caller.clone(),
                target: library.trim_matches(['\'', '"']).to_string(),
                file_path: file_path.clone(),
                line,
                extra: json!({
                    "relationship_role": "loads_shared_library",
                    "bridge_kind": "ffi",
                    "evidence_kind": "syntax",
                    "evidence_source": "DynamicLibrary.open",
                    "source_language": "dart",
                    "target_language": "unknown",
                    "confidence": 0.8,
                    "confidence_tier": "HIGH",
                }),
            });
        }
        let mut extra = match call.qualifier {
            Some(qualifier) => json!({ DART_CALLEE_KEY: qualifier }),
            None => json!({}),
        };
        match dart_receiver(&call.receiver, call.optional, enclosing_class, context) {
            DartReceiver::Typed(type_name) => {
                extra[DART_TYPED_RECEIVER_KEY] = json!(type_name);
            }
            DartReceiver::Unknown(origin) => {
                extra["receiver_unknown"] = json!(true);
                if let Some(origin) = origin {
                    extra["receiver_from"] = origin.to_json();
                }
            }
            DartReceiver::Known => {}
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target: call.target.to_string(),
            file_path: file_path.clone(),
            line,
            extra,
        });
    });
}

/// A call [`dart_eval_chain`] found: the called name, its receiver, whether
/// the receiver was unwrapped (`?.`, `!`), the qualifier the call was
/// written with (see [`DART_CALLEE_KEY`]), and the `argument_part`
/// selector.
struct DartChainCall<'a, 'tree> {
    target: &'a str,
    receiver: DartValue,
    optional: bool,
    qualifier: Option<String>,
    arguments: tree_sitter::Node<'tree>,
}

/// Evaluates an expression chain, the children of one node (`store`
/// `.save` `()`, `open` `(p)` `.save` `()`), reporting each call to
/// `on_call`; returns the chain's value. `line` is the line of every call
/// in it, as its edges record.
fn dart_eval_chain<'tree>(
    children: &[tree_sitter::Node<'tree>],
    line: i64,
    context: &DartContext<'_>,
    mut on_call: impl FnMut(DartChainCall<'_, 'tree>),
) -> DartValue {
    let source = context.source;
    // The value before the pending name, and the name (`save` in
    // `store.save`) that an argument list calls.
    let mut value = DartValue::Known;
    let mut pending: Option<String> = None;
    // `?.name` or `!` on the value before the pending name.
    let mut optional = false;
    // The identifier the chain starts at and how many `.name` selectors
    // follow it: `print(x)` is bare, `convert.jsonEncode(x)` has the one
    // qualifier `convert` (read by `dart_mark_stdlib_edges`).
    let mut root: Option<String> = None;
    let mut depth = 0;
    for child in children {
        match child.kind() {
            "identifier" => {
                let name = node_text(*child, source);
                value = DartValue::Absent;
                root = Some(name.clone());
                pending = Some(name);
                optional = false;
                depth = 0;
            }
            "selector" => {
                if let Some(method_name) = dart_selector_method_name(*child, source) {
                    if let Some(name) = pending.take() {
                        value = dart_member(&value, name, context);
                    }
                    optional =
                        dart_has_direct_child_kind(*child, "conditional_assignable_selector");
                    pending = Some(method_name);
                    depth += 1;
                } else if dart_has_direct_child_kind(*child, "!") {
                    if let Some(name) = pending.take() {
                        value = dart_member(&value, name, context);
                    }
                    optional = true;
                }
                if let Some(arguments) = dart_direct_child(*child, &["argument_part"]) {
                    let Some(target) = pending.take() else {
                        // `f()()`: what a call's result returns.
                        value = DartValue::Unknown;
                        continue;
                    };
                    let qualifier = match (root.take(), depth) {
                        (Some(_), 0) => Some(String::new()),
                        (Some(root), 1) => Some(root),
                        _ => None,
                    };
                    let receiver = std::mem::replace(&mut value, DartValue::Unknown);
                    on_call(DartChainCall {
                        target: &target,
                        receiver: receiver.clone(),
                        optional,
                        qualifier,
                        arguments,
                    });
                    value = dart_call_value(receiver, target, line, context);
                    optional = false;
                }
            }
            "this" => {
                value = DartValue::This;
                pending = None;
                root = None;
            }
            "parenthesized_expression" => {
                value = dart_expression_value(*child, context);
                pending = None;
                root = None;
            }
            "return" | "await" | "yield" | "const" | "new" => {}
            _ => {
                value = DartValue::Known;
                pending = None;
                root = None;
            }
        }
    }
    match pending {
        Some(name) => dart_member(&value, name, context),
        None => value,
    }
}

/// `value.name` (or the plain `name` when there is no value).
fn dart_member(value: &DartValue, name: String, context: &DartContext<'_>) -> DartValue {
    match value {
        DartValue::Absent => DartValue::Name(name),
        DartValue::This => DartValue::ThisField(name),
        DartValue::Name(owner)
            if context.prefixes.contains(owner) || dart_is_type_name(owner, context) =>
        {
            DartValue::Known
        }
        DartValue::Name(owner) if !context.locals.borrow().contains(owner) => DartValue::Known,
        DartValue::Known => DartValue::Known,
        _ => DartValue::Unknown,
    }
}

/// The value of a call to `target` on `receiver`: a constructor call
/// (`Repo()`, `m.Repo()`) builds its type. A chain repeating `target`
/// (`b.x(1).x(2)`) keeps the call before the repeats, as the calls of one
/// line to one method are one edge.
fn dart_call_value(
    receiver: DartValue,
    target: String,
    line: i64,
    context: &DartContext<'_>,
) -> DartValue {
    if let DartValue::Call(origin, _) = &receiver
        && origin.name == target
    {
        return receiver;
    }
    let constructs = match &receiver {
        DartValue::Absent => dart_is_type_name(&target, context),
        DartValue::Name(prefix) => {
            context.prefixes.contains(prefix)
                && target.starts_with(|c: char| c.is_ascii_uppercase())
        }
        _ => false,
    };
    let origin = CallOrigin {
        name: target.clone(),
        line,
        unwrap: false,
    };
    DartValue::Call(origin, constructs.then_some(target))
}

/// The value of an expression node: a chain (`open(p)`), `await` on one
/// (unwrapped: `Future<T>` gives `T`), or one in parentheses.
fn dart_expression_value(node: tree_sitter::Node<'_>, context: &DartContext<'_>) -> DartValue {
    match node.kind() {
        "parenthesized_expression" | "unary_expression" => {
            let mut cursor = node.walk();
            let inner = node.named_children(&mut cursor).next();
            inner.map_or(DartValue::Known, |inner| {
                dart_expression_value(inner, context)
            })
        }
        "await_expression" => {
            let mut cursor = node.walk();
            let children = node.children(&mut cursor).collect::<Vec<_>>();
            let line = node.start_position().row as i64 + 1;
            match dart_eval_chain(&children, line, context, |_| {}) {
                DartValue::Call(mut origin, constructs) => {
                    origin.unwrap = true;
                    DartValue::Call(origin, constructs)
                }
                value => value,
            }
        }
        "identifier" => DartValue::Name(node_text(node, context.source)),
        _ => DartValue::Known,
    }
}

/// Classifies the receiver of a call (see [`DartReceiver`]); `optional`
/// when it was unwrapped (`s?.save()`, `open(p)!.save()`).
fn dart_receiver(
    value: &DartValue,
    optional: bool,
    enclosing_class: Option<&str>,
    context: &DartContext<'_>,
) -> DartReceiver {
    let field_type = |name: &str| {
        let class = enclosing_class?;
        let class = class.rsplit('.').next().unwrap_or(class);
        context.fields.get(class)?.get(name).cloned()
    };
    match value {
        DartValue::Name(name) => {
            if context.locals.borrow().contains(name) {
                let bindings = context.bindings.borrow();
                if let Some(type_name) = bindings
                    .bound_type(name)
                    .or_else(|| bindings.foreign_type(name))
                {
                    return DartReceiver::Typed(type_name.to_string());
                }
                let origin = bindings.returned_by(name).cloned().map(|mut origin| {
                    origin.unwrap |= optional;
                    origin
                });
                return DartReceiver::Unknown(origin);
            }
            match field_type(name) {
                Some(type_name) => DartReceiver::Typed(type_name),
                None => DartReceiver::Known,
            }
        }
        DartValue::ThisField(name) => match field_type(name) {
            Some(type_name) => DartReceiver::Typed(type_name),
            None => DartReceiver::Known,
        },
        DartValue::Call(_, Some(type_name)) => DartReceiver::Typed(type_name.clone()),
        DartValue::Call(origin, None) => {
            let mut origin = origin.clone();
            origin.unwrap |= optional;
            DartReceiver::Unknown(Some(origin))
        }
        DartValue::Unknown => DartReceiver::Unknown(None),
        DartValue::Absent | DartValue::This | DartValue::Known => DartReceiver::Known,
    }
}

fn dart_is_type_name(name: &str, context: &DartContext<'_>) -> bool {
    context.type_names.contains(name) || name.starts_with(|c: char| c.is_ascii_uppercase())
}

/// Scratch key on a `CALLS` edge: the type its receiver is declared with
/// (`Store` for `s.save()` after `final s = Store()`), consumed by
/// [`dart_finish_typed_receivers`] once every method of the library is
/// known.
const DART_TYPED_RECEIVER_KEY: &str = "dart_typed_receiver";

/// Rewrites typed member calls once every method of the library is known:
/// a `dart:core` type the library does not declare (`String s`) points
/// `s.trim()` at `dart:core` (`String.trim`, likely); a method the library
/// declares on the type becomes `Type::method`, which same-file resolution
/// binds; any other keeps the bare method with `receiver_type: "Type"`,
/// which resolution across files matches (an inherited method, an
/// extension, or a class of another library).
fn dart_finish_typed_receivers(
    context: &DartContext<'_>,
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
) {
    let methods = nodes
        .iter()
        .filter(|node| node.kind == crate::core::types::NodeKind::Function)
        .filter_map(|node| {
            let owner = node.parent_name.as_deref()?;
            let type_name = owner.rsplit('.').next().unwrap_or(owner);
            Some((type_name.to_string(), node.name.clone()))
        })
        .collect::<HashSet<_>>();
    for edge in edges.iter_mut() {
        let Some(type_name) = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove(DART_TYPED_RECEIVER_KEY))
            .and_then(|value| value.as_str().map(str::to_string))
        else {
            continue;
        };
        if !context.type_names.contains(&type_name) && is_dart_core_name(&type_name) {
            edge.extra
                .as_object_mut()
                .map(|extra| extra.remove(DART_CALLEE_KEY));
            edge.target = format!("{type_name}.{}", edge.target);
            mark_stdlib_edge(
                &mut edge.target,
                &mut edge.extra,
                "dart:core",
                StdlibEvidence::Likely,
            );
        } else if methods.contains(&(type_name.clone(), edge.target.clone())) {
            edge.target = format!("{type_name}::{}", edge.target);
        } else {
            edge.extra["receiver_type"] = json!(type_name);
        }
    }
}

/// The declared return type as written: `Future<Store>`, `Store?`, `void`
/// before the name of a function or getter; a constructor returns its
/// class.
fn dart_return_type(
    signature: tree_sitter::Node<'_>,
    source: &[u8],
    enclosing_class: Option<&str>,
) -> Option<String> {
    if matches!(
        signature.kind(),
        "constructor_signature" | "factory_constructor_signature"
    ) {
        let class = enclosing_class?;
        return Some(class.rsplit('.').next().unwrap_or(class).to_string());
    }
    let name = signature.child_by_field_name("name")?;
    let mut cursor = signature.walk();
    let parts = signature
        .children(&mut cursor)
        .take_while(|child| {
            child.id() != name.id() && !matches!(child.kind(), "get" | "set" | "operator")
        })
        .collect::<Vec<_>>();
    let (first, last) = (parts.first()?, parts.last()?);
    let text = std::str::from_utf8(&source[first.start_byte()..last.end_byte()]).ok()?;
    Some(text.trim().to_string())
}

/// The type a declaration names before its variable, when its methods are
/// the value's: `Store` in `Store? s`, `m.Store s`, `List<Store> xs`
/// (`List`). `var` / `final` / `dynamic` name none.
fn dart_declared_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut found = None;
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => found = Some(node_text(child, source)),
            "identifier" | "initialized_identifier_list" | "initialized_identifier" | "=" => break,
            _ => {}
        }
    }
    found.filter(|name| name != "dynamic" && name != "Object")
}

/// The type a variable's initializer constructs: `Repo()`, `m.Repo()`,
/// `const Repo()`, `new Repo()`.
fn dart_initialized_type(
    values: &[tree_sitter::Node<'_>],
    context: &DartContext<'_>,
) -> Option<String> {
    match dart_values_value(values, context) {
        DartValue::Call(_, constructs) => constructs,
        _ => None,
    }
}

/// The value the children after `=` give (see [`dart_eval_chain`]).
fn dart_values_value(values: &[tree_sitter::Node<'_>], context: &DartContext<'_>) -> DartValue {
    let [first, ..] = values else {
        return DartValue::Known;
    };
    if values.len() == 1 && first.kind() != "identifier" {
        return dart_expression_value(*first, context);
    }
    let line = first.parent().map_or(first.start_position().row, |parent| {
        parent.start_position().row
    }) as i64
        + 1;
    dart_eval_chain(values, line, context, |_| {})
}

/// The children after the `=` of a variable or field.
fn dart_initializer<'tree>(node: tree_sitter::Node<'tree>) -> Vec<tree_sitter::Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .skip_while(|child| child.kind() != "=")
        .skip(1)
        .collect()
}

/// The classes the library declares and the typed fields of each (see
/// [`DartContext::fields`]).
fn dart_collect_types_and_fields(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    types: &mut HashSet<String>,
    fields: &mut HashMap<String, HashMap<String, String>>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_definition" | "mixin_declaration" | "enum_declaration" | "extension_declaration"
        ) && let Some(name) = dart_direct_child_text(child, source, &["identifier"])
        {
            types.insert(name.clone());
            if let Some(body) = child.child_by_field_name("body") {
                let mut members = body.walk();
                for member in body.children(&mut members) {
                    if member.kind() != "declaration" {
                        continue;
                    }
                    let declared = dart_declared_type(member, source);
                    let Some(list) = dart_direct_child(member, &["initialized_identifier_list"])
                    else {
                        continue;
                    };
                    let mut items = list.walk();
                    for item in list.children(&mut items) {
                        let Some(var) = dart_direct_child_text(item, source, &["identifier"])
                        else {
                            continue;
                        };
                        let type_name = declared.clone().or_else(|| {
                            let values = dart_initializer(item);
                            let first = values.first()?;
                            let callee = node_text(*first, source);
                            (first.kind() == "identifier"
                                && callee.starts_with(|c: char| c.is_ascii_uppercase())
                                && values.len() == 2
                                && dart_selector_has_arguments(values[1]))
                            .then_some(callee)
                        });
                        if let Some(type_name) = type_name {
                            fields
                                .entry(name.clone())
                                .or_default()
                                .insert(var, type_name);
                        }
                    }
                }
            }
        }
        dart_collect_types_and_fields(child, source, types, fields);
    }
}

/// Every import prefix (`m` in `import 'models.dart' as m`).
fn dart_import_prefixes(root: tree_sitter::Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut prefixes = HashSet::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if let Some(prefix) = dart_direct_child(child, &["library_import"])
            .and_then(|import| dart_direct_child(import, &["import_specification"]))
            .and_then(|specification| {
                dart_direct_child_text(specification, source, &["identifier"])
            })
        {
            prefixes.insert(prefix);
        }
    }
    prefixes
}

/// The bindings and locals of the enclosing scope, restored by
/// [`dart_leave_function`].
struct DartSavedScope {
    bindings: BindingsSnapshot,
    locals: HashSet<String>,
}

/// Enters a function body: binds the signature's parameters to their types
/// (`void run(Store s, {required Repo r})`) and records the names the
/// function declares as locals. A local function (`nested`) also sees the
/// variables of the function around it.
fn dart_enter_function(
    signature: tree_sitter::Node<'_>,
    body: tree_sitter::Node<'_>,
    nested: bool,
    context: &DartContext<'_>,
) -> DartSavedScope {
    let saved = DartSavedScope {
        bindings: context.bindings.borrow().snapshot(),
        locals: context.locals.borrow().clone(),
    };
    let mut locals = HashSet::new();
    dart_collect_declared_names(signature, context.source, &mut locals);
    dart_collect_declared_names(body, context.source, &mut locals);
    {
        let mut bindings = context.bindings.borrow_mut();
        for name in &locals {
            bindings.forget_foreign(name);
        }
    }
    dart_bind_parameters(signature, context);
    let mut current = context.locals.borrow_mut();
    if !nested {
        current.clear();
    }
    current.extend(locals);
    saved
}

fn dart_bind_parameters(node: tree_sitter::Node<'_>, context: &DartContext<'_>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "formal_parameter" {
            if let (Some(name), Some(type_name)) = (
                child.child_by_field_name("name"),
                dart_declared_type(child, context.source),
            ) {
                context
                    .bindings
                    .borrow_mut()
                    .bind_any(node_text(name, context.source), type_name);
            }
        } else if child.kind() != "function_body" {
            dart_bind_parameters(child, context);
        }
    }
}

fn dart_leave_function(context: &DartContext<'_>, saved: DartSavedScope) {
    context.bindings.borrow_mut().restore(saved.bindings);
    *context.locals.borrow_mut() = saved.locals;
}

/// Binds a local variable: `Store s = ...` and `final s = Store()` to the
/// type; `final s = open(p)` and `var s = await open(p)` to the call;
/// anything else drops what it was bound to.
fn dart_bind_variable(node: tree_sitter::Node<'_>, context: &DartContext<'_>) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let var = node_text(name, context.source);
    let values = dart_initializer(node);
    let type_name = dart_declared_type(node, context.source)
        .or_else(|| dart_initialized_type(&values, context));
    if let Some(type_name) = type_name {
        context.bindings.borrow_mut().bind_any(var, type_name);
        return;
    }
    context.bindings.borrow_mut().forget_foreign(&var);
    let origin = match dart_values_value(&values, context) {
        DartValue::Call(origin, None) => Some(origin),
        DartValue::Name(other) => context.bindings.borrow().returned_by(&other).cloned(),
        _ => None,
    };
    if let Some(origin) = origin {
        context.bindings.borrow_mut().bind_returned(var, origin);
    }
}

/// Scratch key on a `CALLS` edge: the qualifier the call was written with
/// (`convert` for `convert.jsonEncode(x)`, empty for a bare `print(x)`),
/// consumed by [`dart_mark_stdlib_edges`].
const DART_CALLEE_KEY: &str = "dart_callee_qualifier";

/// The `dart:` imports of a library: the prefixed ones by prefix
/// (`import 'dart:convert' as convert`), and the unprefixed ones with the
/// names their `show` / `hide` combinators admit.
#[derive(Default)]
struct DartStdlibImports {
    prefixes: HashMap<String, String>,
    /// Prefixes of any other import (`http` in
    /// `import 'package:http/http.dart' as http`).
    other_prefixes: HashSet<String>,
    unprefixed: Vec<DartUnprefixedImport>,
}

struct DartUnprefixedImport {
    library: String,
    show: Option<Vec<String>>,
    hide: Vec<String>,
}

impl DartUnprefixedImport {
    fn admits(&self, name: &str) -> bool {
        self.show
            .as_ref()
            .is_none_or(|show| show.iter().any(|shown| shown == name))
            && !self.hide.iter().any(|hidden| hidden == name)
    }
}

fn dart_collect_stdlib_imports(root: tree_sitter::Node<'_>, source: &[u8]) -> DartStdlibImports {
    let mut imports = DartStdlibImports::default();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let Some(specification) = dart_direct_child(child, &["library_import"])
            .and_then(|import| dart_direct_child(import, &["import_specification"]))
        else {
            continue;
        };
        let Some(uri) = dart_first_descendant_text(specification, source, &["string_literal"])
        else {
            continue;
        };
        let uri = strip_matching_quotes(uri.trim()).to_string();
        let mut prefix = None;
        let mut show: Option<Vec<String>> = None;
        let mut hide = Vec::new();
        let mut inner = specification.walk();
        for part in specification.children(&mut inner) {
            match part.kind() {
                "identifier" => prefix = Some(node_text(part, source)),
                "combinator" => {
                    let mut names = part.walk();
                    let names = part
                        .children(&mut names)
                        .filter(|name| name.kind() == "identifier")
                        .map(|name| node_text(name, source))
                        .collect::<Vec<_>>();
                    if dart_has_direct_child_kind(part, "show") {
                        show.get_or_insert_with(Vec::new).extend(names);
                    } else {
                        hide.extend(names);
                    }
                }
                _ => {}
            }
        }
        match (dart_library(&uri), prefix) {
            (Some(library), Some(prefix)) => {
                imports.prefixes.insert(prefix, library.to_string());
            }
            (None, Some(prefix)) => {
                imports.other_prefixes.insert(prefix);
            }
            (Some(library), None) => imports.unprefixed.push(DartUnprefixedImport {
                library: library.to_string(),
                show,
                hide,
            }),
            (None, None) => {}
        }
    }
    imports
}

/// Every name the library declares: its classes, functions, and methods
/// (`nodes`), and its variables, constants, parameters, loop variables,
/// `catch` parameters, and type aliases.
fn dart_collect_declared_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    if matches!(
        node.kind(),
        "initialized_variable_definition"
            | "initialized_identifier"
            | "static_final_declaration"
            | "formal_parameter"
            | "constructor_param"
            | "for_loop_parts"
            | "catch_parameters"
            | "type_alias"
    ) {
        // The names before the value: `f` in `final f = File(p)`, `i` in
        // `for (var i in xs)`.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "=" | "in" | ":" => break,
                "identifier" => {
                    names.insert(node_text(child, source));
                }
                "type_identifier" if node.kind() == "type_alias" => {
                    names.insert(node_text(child, source));
                }
                _ => {}
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dart_collect_declared_names(child, source, names);
    }
}

/// Points the calls and imports that reach a Dart core library at it: an
/// import of `dart:io` (certain), a call through a `dart:` import prefix
/// (`convert.jsonEncode(x)`, certain), a name of an unprefixed `dart:`
/// import (`File(p)` after `import 'dart:io'`, likely) or of `dart:core`
/// (`print(x)`, `DateTime.now()`, likely). A name the library declares,
/// or the prefix of another import, is never the standard library's.
fn dart_mark_stdlib_edges(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
) {
    let imports = dart_collect_stdlib_imports(root, source);
    let mut declared = nodes
        .iter()
        .filter(|node| node.kind != crate::core::types::NodeKind::File)
        .map(|node| node.name.clone())
        .collect::<HashSet<_>>();
    dart_collect_declared_names(root, source, &mut declared);
    for edge in edges.iter_mut() {
        match edge.kind {
            crate::core::types::EdgeKind::ImportsFrom => {
                if let Some(library) = dart_library(&edge.target).map(str::to_string) {
                    mark_stdlib_edge(
                        &mut edge.target,
                        &mut edge.extra,
                        &library,
                        StdlibEvidence::Certain,
                    );
                }
            }
            crate::core::types::EdgeKind::Calls => {
                let Some(qualifier) = edge
                    .extra
                    .as_object_mut()
                    .and_then(|extra| extra.remove(DART_CALLEE_KEY))
                    .and_then(|qualifier| qualifier.as_str().map(str::to_string))
                else {
                    continue;
                };
                let (name, symbol) = if qualifier.is_empty() {
                    (edge.target.clone(), edge.target.clone())
                } else {
                    (qualifier.clone(), format!("{qualifier}.{}", edge.target))
                };
                let found = if let Some(library) = imports.prefixes.get(&qualifier) {
                    Some((library.clone(), StdlibEvidence::Certain))
                } else if declared.contains(&name) || imports.other_prefixes.contains(&name) {
                    None
                } else {
                    imports
                        .unprefixed
                        .iter()
                        .find(|import| {
                            import.admits(&name) && dart_library_exports(&import.library, &name)
                        })
                        .map(|import| import.library.clone())
                        .or_else(|| is_dart_core_name(&name).then(|| "dart:core".to_string()))
                        .map(|library| (library, StdlibEvidence::Likely))
                };
                if let Some((library, evidence)) = found {
                    edge.target = symbol;
                    mark_stdlib_edge(&mut edge.target, &mut edge.extra, &library, evidence);
                }
            }
            _ => {}
        }
    }
}

/// `@Native<...>(symbol: "sym") external T f(...)`: the C symbol an
/// `external` function binds (`symbol`, else its own name).
fn dart_native_symbol(
    signature: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
) -> Option<String> {
    let mut current = signature.prev_named_sibling();
    while let Some(annotation) = current.filter(|node| node.kind() == "annotation") {
        let is_native = annotation
            .child_by_field_name("name")
            .is_some_and(|annotation_name| node_text(annotation_name, source) == "Native");
        if is_native {
            let mut cursor = annotation.walk();
            let symbol = annotation
                .children(&mut cursor)
                .filter(|child| child.kind() == "arguments")
                .flat_map(|arguments| {
                    let mut inner = arguments.walk();
                    arguments.children(&mut inner).collect::<Vec<_>>()
                })
                .filter(|argument| argument.kind() == "named_argument")
                .find(|argument| {
                    dart_first_descendant_text(*argument, source, &["label"])
                        .is_some_and(|label| label.trim_end_matches(':').trim() == "symbol")
                })
                .and_then(|argument| {
                    dart_first_descendant_text(argument, source, &["string_literal"])
                })
                .map(|literal| literal.trim_matches(['\'', '"']).to_string());
            return Some(symbol.unwrap_or_else(|| name.to_string()));
        }
        current = annotation.prev_named_sibling();
    }
    None
}

fn dart_selector_method_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // `.name`, or `?.name` on a nullable value.
        if matches!(
            child.kind(),
            "unconditional_assignable_selector" | "conditional_assignable_selector"
        ) {
            return dart_first_descendant_text(child, source, &["identifier"]);
        }
    }
    None
}

fn dart_selector_has_arguments(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .any(|child| child.kind() == "argument_part")
}

/// `(base, role)` for `extends B<T>`, `with M`, `implements C<D>, E`; type
/// arguments are not bases.
fn dart_inheritance_targets(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<(String, &'static str)> {
    fn direct_types(
        node: tree_sitter::Node<'_>,
        source: &[u8],
        role: &'static str,
        out: &mut Vec<(String, &'static str)>,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "type_identifier" => out.push((node_text(child, source), role)),
                "mixins" => direct_types(child, source, "mixin", out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "superclass" => direct_types(child, source, "extends", &mut out),
            "interfaces" => direct_types(child, source, "implements", &mut out),
            _ => {}
        }
    }
    out
}

fn dart_has_direct_child_kind(node: tree_sitter::Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();

    node.children(&mut cursor).any(|child| child.kind() == kind)
}

fn dart_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn dart_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    dart_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn dart_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = dart_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}
