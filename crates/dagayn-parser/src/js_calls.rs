use serde_json::{Value, json};

use super::js_like::{javascript_is_local_name, javascript_push_node, javascript_walk_children};
use super::js_members::{
    JavaScriptTypeRef, annotation_type_name, javascript_base_member, javascript_receiver_type,
    javascript_resolved_base, javascript_this_type, javascript_type_member,
    javascript_written_bases, resolve_javascript_type_name,
};
use super::js_modules::{
    JavaScriptExportResolution, JavaScriptParseContext, decode_javascript_string_literal,
    javascript_external_symbol, javascript_module_index, resolve_javascript_call_target,
    resolve_javascript_import_path_in, resolve_javascript_namespace_member,
};
use super::js_tests::{
    JavaScriptTestCall, javascript_is_test_api_call, javascript_test_call, javascript_test_title,
};
use super::qualify;
use super::types::{ParsedEdge, ParsedNode};
use super::util::node_text;

pub(super) fn javascript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    if node.kind() == "call_expression"
        && javascript_callee_node(node).is_some_and(|callee| callee.kind() == "super")
    {
        javascript_emit_super_call(node, context, owner_path, enclosing_func, edges);
        return false;
    }
    if context.test_file {
        match javascript_test_call(node, context.source, context.defined_names) {
            Some(JavaScriptTestCall::Test { runner, modifiers }) => {
                javascript_emit_test(
                    node,
                    context,
                    owner_path,
                    enclosing_func,
                    (&runner, modifiers),
                    nodes,
                    edges,
                );
                return true;
            }
            // Hooks, `test.step`, the `test.each(table)` factory: no node
            // and no edge; their callbacks belong to the enclosing node.
            Some(JavaScriptTestCall::RunnerApi) => return false,
            None => {}
        }
    }
    let Some(call_name) = javascript_call_name(node, context.source) else {
        return false;
    };

    if javascript_callee_node(node).is_some_and(|callee| {
        callee.kind() == "identifier"
            && javascript_is_local_name(context, &node_text(callee, context.source))
    }) {
        // A call of a local declaration stays inside the enclosing function.
        return false;
    }
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    let (target, mut extra) = javascript_member_call_target(node, context, owner_path, &call_name)
        .unwrap_or_else(|| {
            (
                resolve_javascript_call_target(&call_name, context),
                json!({}),
            )
        });
    if context.test_file
        && javascript_is_test_api_call(node, context.source)
        && let Some(map) = extra.as_object_mut()
    {
        // Assertion / mock APIs are not the code under test.
        map.insert("test_api".to_string(), json!(true));
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
    if let Some(edge) = javascript_bridge_edge(node, context, &caller) {
        edges.push(edge);
    }
    false
}

/// A synthetic `Test` node for a test-runner call: `runner:title@Lline`
/// (`runner@Lline` without a title), spanning the whole call, so for
/// `test.each(table)("title", fn)` the outer call. It is contained by the
/// enclosing test (`describe`) or the File, and the calls in its callbacks
/// and table belong to it.
fn javascript_emit_test(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    (runner, modifiers): (&str, Vec<String>),
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let line = node.start_position().row as i64 + 1;
    let synthetic_name = match javascript_test_title(node, context.source) {
        Some(title) => format!("{runner}:{title}@L{line}"),
        None => format!("{runner}@L{line}"),
    };
    let qualified = qualify(&context.file_path, &synthetic_name, owner_path);
    let extra = if modifiers.is_empty() {
        json!({})
    } else {
        json!({"test_modifiers": modifiers})
    };
    javascript_push_node(
        context,
        nodes,
        node,
        ParsedNode {
            kind: crate::core::types::NodeKind::Test,
            name: synthetic_name.clone(),
            file_path: context.file_path.clone(),
            line_start: line,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: owner_path.map(str::to_string),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: true,
            extra,
        },
    );
    let container = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: context.file_path.clone(),
        line,
        extra: json!({}),
    });
    javascript_walk_children(
        node,
        context,
        owner_path,
        Some(&synthetic_name),
        nodes,
        edges,
    );
}

/// `super(...)` in a constructor: `CALLS` to the base class
/// (`call_kind: "super"`), as `new Base()` would be. An unresolved base
/// (an external package) keeps its rightmost written name.
fn javascript_emit_super_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(this) = javascript_this_type(context) else {
        return;
    };
    let target = match javascript_resolved_base(context, &this) {
        Some(base) => format!("{}::{}", base.file, base.path),
        None => {
            let Some(base) = javascript_written_bases(context, &this).into_iter().next() else {
                return;
            };
            base.rsplit('.').next().unwrap_or(&base).to_string()
        }
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({"call_kind": "super"}),
    });
}

pub(super) fn javascript_emit_value_references(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    match node.kind() {
        "pair" => {
            if let Some(value) = javascript_pair_value_identifier(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "shorthand_property_identifier" => {
            let value = node_text(node, context.source);
            javascript_emit_reference_if_known(node, context, &caller, &value, edges);
        }
        "assignment_expression" => {
            if let Some(value) = javascript_last_identifier_child(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "array" | "arguments" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    let value = node_text(child, context.source);
                    javascript_emit_reference_if_known(child, context, &caller, &value, edges);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn javascript_emit_jsx_component_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = javascript_jsx_component_target(node, context) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, owner_path))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn javascript_jsx_component_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let (base_name, component_name) = javascript_jsx_component_reference(node, context.source)?;
    if base_name.is_none() && javascript_is_local_name(context, &component_name) {
        return None;
    }
    if let Some(base_name) = base_name {
        return resolve_javascript_namespace_member(&base_name, &component_name, context)
            .or_else(|| {
                let mut cursor = node.walk();
                node.children(&mut cursor)
                    .find(|child| child.kind() == "member_expression")
                    .and_then(|member| javascript_external_member_target(member, context))
            })
            .or(Some(component_name));
    }
    Some(resolve_javascript_call_target(&component_name, context))
}

fn javascript_jsx_component_reference(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Option<String>, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(child, source);
                return looks_like_jsx_component_name(&name).then_some((None, name));
            }
            "member_expression" => {
                let component_name = javascript_rightmost_identifier(child, source)?;
                if !looks_like_jsx_component_name(&component_name) {
                    return None;
                }
                let base_name = javascript_leftmost_identifier(child, source);
                return Some((base_name, component_name));
            }
            _ => {}
        }
    }
    None
}

fn looks_like_jsx_component_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_uppercase())
}

fn javascript_emit_reference_if_known(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if javascript_should_skip_value_reference(name)
        || (!context.defined_names.contains(name) && !context.import_map.contains_key(name))
    {
        return;
    }
    let target = resolve_javascript_call_target(name, context);
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::References,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

/// `ns.Name` where `ns` is a namespace (or default) import resolves to the
/// exporting module's QN; anything else stays unresolved.
pub(super) fn javascript_namespace_member_target(
    object: tree_sitter::Node<'_>,
    name: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    if object.kind() != "identifier" {
        return None;
    }
    resolve_javascript_namespace_member(&node_text(object, context.source), name, context)
}

pub(super) fn javascript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    match callee.kind() {
        "identifier" | "property_identifier" | "type_identifier" => Some(node_text(callee, source)),
        "member_expression" => javascript_rightmost_identifier(callee, source),
        _ => None,
    }
}

/// Target of a member call (`recv.m()`) and the edge metadata it carries,
/// or `None` when the callee is not a member expression.
///
/// The receiver is bound only with evidence: a same-file object container or
/// namespace, a namespace / named import (`fns.decl()`, `Outer.helper()`),
/// or a receiver whose class is known (`this`, `super`, a typed field or
/// variable, a class named directly). A member found on a base is `MEDIUM`.
/// Anything else keeps the bare member name with `receiver_unknown: true`,
/// which same-file resolution leaves alone, so `res.json()` never becomes an
/// unrelated `json` and `this.users.findAll()` never the caller itself.
fn javascript_member_call_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
    call_name: &str,
) -> Option<(String, Value)> {
    let callee = javascript_callee_node(node)?;
    if callee.kind() != "member_expression" {
        return None;
    }
    if let Some(target) = javascript_object_member_target(callee, context, owner_path)
        .or_else(|| javascript_imported_member_target(callee, context))
        .or_else(|| javascript_external_member_target(callee, context))
    {
        return Some((target, json!({})));
    }
    let unknown = Some((call_name.to_string(), json!({"receiver_unknown": true})));
    let (Some(object), Some(property)) = (
        callee.child_by_field_name("object"),
        callee.child_by_field_name("property"),
    ) else {
        return unknown;
    };
    if !matches!(
        property.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return unknown;
    }
    let method = node_text(property, context.source);
    let found = if object.kind() == "super" {
        javascript_this_type(context)
            .and_then(|this| javascript_base_member(context, &this, &method))
    } else {
        javascript_receiver_type(context, object)
            .and_then(|ty| javascript_type_member(context, &ty, &method))
    };
    if let Some(found) = found {
        let extra = if found.inherited {
            json!({"confidence": 0.6, "confidence_tier": "MEDIUM"})
        } else {
            json!({})
        };
        return Some((found.qualified, extra));
    }
    // A variable bound to a same-file type that does not declare the member
    // (an enum, a type alias, a class inheriting from outside) keeps the
    // `Type::m` form for same-file resolution.
    if object.kind() == "identifier"
        && let Some(target) = context
            .bindings
            .borrow()
            .resolve_member(&node_text(object, context.source), &method)
    {
        return Some((target, json!({})));
    }
    unknown
}

/// `fns.decl()` / `fns.api.get()` through a namespace import, and
/// `Outer.helper()` / `Outer.Deep.deepFn()` / `api.get()` through a named or
/// default import of a namespace or object container.
fn javascript_imported_member_target(
    callee: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let path = javascript_member_path(callee, context.source)?;
    let (owner, method) = path.rsplit_once('.')?;
    let (root, rest) = match owner.split_once('.') {
        Some((root, rest)) => (root, Some(rest)),
        None => (owner, None),
    };
    if context.bindings.borrow().is_bound(root) || javascript_is_local_name(context, root) {
        return None;
    }
    let binding = context.import_map.get(root)?;
    let mut segments = rest
        .map(|rest| rest.split('.').collect::<Vec<_>>())
        .unwrap_or_default();
    segments.push(method);
    // Module objects (`import * as ns`, `export * as ns from`, CommonJS
    // exports) are entered segment by segment; the first declaration
    // reached is the container of the rest of the path.
    let (resolved, consumed) = resolve_javascript_import_path_in(
        &context.file_path,
        root,
        binding,
        &segments,
        context.repo_root,
        context.caches,
    )?;
    let JavaScriptExportResolution::Symbol(container) = resolved else {
        return None;
    };
    if consumed == segments.len() {
        return Some(container);
    }
    let container = JavaScriptTypeRef::from_qualified(&container)?;
    let rest = &segments[consumed..segments.len() - 1];
    let owner = if rest.is_empty() {
        container.path
    } else {
        format!("{}.{}", container.path, rest.join("."))
    };
    let member_path = format!("{owner}.{method}");
    let known = if container.file == context.file_path.as_str() {
        context.member_paths.contains(&member_path)
    } else {
        javascript_module_index(&container.file, context)
            .is_some_and(|index| index.member_paths.contains(&member_path))
    };
    known.then(|| qualify(&container.file, method, Some(&owner)))
}

/// `fs.readFile` / `React.useEffect` / `z.object` where the root is an
/// import binding of an external package: `pkg::path`
/// ([`javascript_external_symbol`]).
pub(super) fn javascript_external_member_target(
    member: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let path = javascript_member_path(member, context.source)?;
    let mut segments = path.split('.');
    let root = segments.next()?;
    if context.bindings.borrow().is_bound(root)
        || javascript_is_local_name(context, root)
        || context.defined_names.contains(root)
    {
        return None;
    }
    javascript_external_symbol(root, &segments.collect::<Vec<_>>(), context)
}

/// `api.get()` / `api.nested.deep()` where `api` is a same-file object
/// container and the member exists, and `this.m()` inside a container
/// member when the container has `m`.
fn javascript_object_member_target(
    callee: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    owner_path: Option<&str>,
) -> Option<String> {
    if context.member_paths.is_empty() {
        return None;
    }
    if let (Some(owner), Some(object), Some(property)) = (
        owner_path,
        callee.child_by_field_name("object"),
        callee.child_by_field_name("property"),
    ) && object.kind() == "this"
        && !context.namespace_paths.contains(owner)
    {
        let method = node_text(property, context.source);
        return context
            .member_paths
            .contains(&format!("{owner}.{method}"))
            .then(|| qualify(&context.file_path, &method, Some(owner)));
    }
    let path = javascript_member_path(callee, context.source)?;
    let (owner, method) = path.rsplit_once('.')?;
    let root = owner.split('.').next()?;
    if context.bindings.borrow().is_bound(root) || !context.member_paths.contains(&path) {
        return None;
    }
    Some(qualify(&context.file_path, method, Some(owner)))
}

/// Dotted text of a pure identifier member chain (`a.b.c`), or `None` when
/// any segment is computed, a call, `this`, and so on.
pub(super) fn javascript_member_path(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(node, source)),
        "member_expression" => {
            let object = javascript_member_path(node.child_by_field_name("object")?, source)?;
            let property = node.child_by_field_name("property")?;
            (property.kind() == "property_identifier")
                .then(|| format!("{object}.{}", node_text(property, source)))
        }
        _ => None,
    }
}

pub(super) fn javascript_bind_declarator(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) {
    if node.kind() != "variable_declarator" {
        return;
    }
    let mut ident = None;
    let mut annotated = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "type_annotation" => {
                annotated = annotation_type_name(child, context.source);
            }
            "new_expression" | "call_expression" => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let Some(ident) = ident else {
        return;
    };
    if let Some(value) = value
        && let Some(type_name) = javascript_inferred_constructor(value, context)
    {
        javascript_bind_receiver(context, ident, type_name);
        return;
    }
    if let Some(type_name) = annotated {
        javascript_bind_type_name(context, ident, &type_name);
    }
}

/// Binds `ident` to the type written as `type_name`: a class or interface of
/// this file (by owner path) or of another module (by its `file::path`
/// QN); other same-file types (enums, aliases) by name as before.
fn javascript_bind_type_name(context: &JavaScriptParseContext<'_>, ident: String, type_name: &str) {
    match resolve_javascript_type_name(context, context.file_path.as_str(), type_name) {
        Some(ty) if ty.file == context.file_path.as_str() => {
            context.bindings.borrow_mut().bind_path(ident, ty.path);
        }
        Some(ty) => {
            let qualified = format!("{}::{}", ty.file, ty.path);
            context.bindings.borrow_mut().bind_path(ident, qualified);
        }
        None => context.bindings.borrow_mut().bind(ident, type_name),
    }
}

/// Binds the typed parameters of a function (`run(r: Repo)`,
/// `constructor(private repo: Repo)`) for the calls in its body.
pub(super) fn javascript_bind_parameters(
    function_node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) {
    let Some(parameters) = function_node.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !matches!(
            parameter.kind(),
            "required_parameter" | "optional_parameter"
        ) {
            continue;
        }
        let (Some(pattern), Some(type_name)) = (
            parameter
                .child_by_field_name("pattern")
                .filter(|pattern| pattern.kind() == "identifier"),
            parameter
                .child_by_field_name("type")
                .and_then(|annotation| annotation_type_name(annotation, context.source)),
        ) else {
            continue;
        };
        javascript_bind_type_name(context, node_text(pattern, context.source), &type_name);
    }
}

pub(super) fn javascript_bind_assignment(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) {
    if node.kind() != "assignment_expression" {
        return;
    }
    let mut ident = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "new_expression" | "call_expression" => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let (Some(ident), Some(value)) = (ident, value) else {
        return;
    };
    if let Some(type_name) = javascript_inferred_constructor(value, context) {
        javascript_bind_receiver(context, ident, type_name);
    }
}

fn javascript_bind_receiver(
    context: &JavaScriptParseContext<'_>,
    ident: String,
    type_name: String,
) {
    if type_name.contains('.') || type_name.contains("::") {
        // Only produced for verified member paths and imported classes.
        context.bindings.borrow_mut().bind_path(ident, type_name);
    } else {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn javascript_inferred_constructor(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    // `new Outer.Inner()`: a same-file namespace member binds by its path.
    if node.kind() == "new_expression"
        && let Some(path) = javascript_callee_node(node)
            .filter(|callee| callee.kind() == "member_expression")
            .and_then(|callee| javascript_member_path(callee, context.source))
        && context.member_paths.contains(&path)
    {
        return Some(path);
    }
    let call_name = javascript_call_name(node, context.source)?;
    if let Some(type_name) = context
        .bindings
        .borrow()
        .constructor_type(&call_name)
        .map(str::to_string)
    {
        return Some(type_name);
    }
    // `new DefaultShape()` / `new ns.Repo()` of an imported class.
    let callee = javascript_callee_node(node).filter(|_| node.kind() == "new_expression")?;
    let path = javascript_member_path(callee, context.source)?;
    let ty = resolve_javascript_type_name(context, context.file_path.as_str(), &path)?;
    (ty.file != context.file_path.as_str()).then(|| format!("{}::{}", ty.file, ty.path))
}

fn javascript_callee_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if node.kind() == "new_expression" {
        if let Some(constructor) = node.child_by_field_name("constructor") {
            return Some(constructor);
        }
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        return children
            .into_iter()
            .find(|child| !matches!(child.kind(), "new" | "arguments" | "type_arguments"));
    }
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .find(|child| child.kind() != "arguments")
}

fn javascript_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier"
                | "property_identifier"
                | "type_identifier"
                | "private_property_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_leftmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_leftmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    javascript_callee_node(node)
        .map(|callee| node_text(callee, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn javascript_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
) -> Option<ParsedEdge> {
    let signature = javascript_call_signature(node, context.source)?;
    let (relationship_role, bridge_kind) = javascript_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) =
        match javascript_first_string_arg(node, context.source) {
            Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
            _ => (
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
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn javascript_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "child_process.exec"
        | "child_process.execFile"
        | "child_process.execSync"
        | "child_process.execFileSync"
        | "child_process.spawn"
        | "child_process.spawnSync"
        | "child_process.fork" => Some(("invokes_binary", "subprocess")),
        "fs.readFile" | "fs.readFileSync" | "fs.promises.readFile" => {
            Some(("reads_file", "file_io"))
        }
        "fs.writeFile" | "fs.writeFileSync" | "fs.promises.writeFile" => {
            Some(("writes_file", "file_io"))
        }
        _ => None,
    }
}

fn javascript_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string" | "template_string") {
            return Some(decode_javascript_string_literal(child, source));
        }
        return None;
    }
    None
}

fn javascript_pair_value_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut seen_colon = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == ":" {
            seen_colon = true;
            continue;
        }
        if seen_colon && child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn javascript_last_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .rev()
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(child, source))
}

fn javascript_should_skip_value_reference(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "null"
            | "undefined"
            | "None"
            | "True"
            | "False"
            | "self"
            | "this"
            | "cls"
            | "super"
    ) || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
}
