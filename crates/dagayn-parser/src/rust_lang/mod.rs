use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use super::member_calls::MemberCallBindings;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text};
use super::{add_tested_by_edges, is_test_function, qualify, resolve_rust_call_targets};

mod ffi;
pub(crate) mod modules;

use ffi::*;

pub(super) fn parse_rust_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo: Option<(&Path, &modules::RustModuleCache)>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: "rust".to_string(),
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
        let mut defined_names = HashSet::new();
        collect_rust_defined_names(root, source, &mut defined_names);
        let mut free_functions = HashSet::new();
        collect_rust_free_functions(root, source, &mut free_functions);
        let struct_fields = collect_rust_struct_fields(root, source);
        let mut type_names = HashSet::new();
        collect_rust_type_names(root, source, &mut type_names);
        let scope =
            repo.map(|(root, cache)| modules::RustModuleScope::new(root, cache, &file_path));
        let context = RustParseContext {
            source,
            file_path: file_path.clone(),
            scope: scope.as_ref(),
            uses: RefCell::new(HashMap::new()),
            repo_glob: std::cell::Cell::new(false),
            free_functions: &free_functions,
            struct_fields: &struct_fields,
            locals: RefCell::new(Vec::new()),
            defined_names: &defined_names,
            bindings: RefCell::new(MemberCallBindings::with_types(type_names)),
            component_bindings: rust_uses_component_bindings(source),
        };
        rust_walk_children(root, &context, None, None, &mut nodes, &mut edges);
        let component_bindings = context.component_bindings;
        rust_wasm_host_edges(
            root,
            source,
            &file_path,
            component_bindings,
            None,
            None,
            &mut edges,
        );
        record_neon_exported_functions(root, source, &mut nodes);
        if let Some(namespace) = rust_uniffi_namespace(root, source) {
            nodes[0].extra["uniffi_namespace"] = json!(namespace);
        }
        let bare: Vec<bool> = edges
            .iter()
            .map(|edge| {
                edge.kind == crate::core::types::EdgeKind::Calls
                    && !edge.target.contains("::")
                    && edge.extra.get("receiver_unknown").is_none()
            })
            .collect();
        let mut edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        rust_keep_bare_calls_in_scope(&nodes, &mut edges, &bare, &file_path);
        rust_finish_call_targets(
            &mut edges,
            &file_path,
            scope.as_ref(),
            &context.uses.borrow(),
        );
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

fn rust_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    // Items inside a function body are local to it: `fn a() { fn helper() }`
    // declares `a.helper`, so two functions' `helper`s stay apart.
    let local_owner = std::cell::OnceCell::new();
    // Built on first use: this runs for every node of a function body.
    let owner = || {
        local_owner
            .get_or_init(|| enclosing_func.map(|func| rust_scope_join(enclosing_class, func)))
            .as_deref()
            .or(enclosing_class)
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "mod_item" if child.child_by_field_name("body").is_some() => {
                if let Some(name) = rust_identifier_child(child, context.source) {
                    let path = rust_scope_join(owner(), &name);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Class,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: owner().map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({"type_role": "module"}),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: rust_container(&context.file_path, owner()),
                        target: qualify(&context.file_path, &name, owner()),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_walk_children(child, context, Some(&path), None, nodes, edges);
                    continue;
                }
            }
            "struct_item" | "enum_item" | "trait_item" | "type_item" => {
                if let Some(name) = rust_type_name(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, owner());
                    let kind = if child.kind() == "type_item" {
                        crate::core::types::NodeKind::Type
                    } else {
                        crate::core::types::NodeKind::Class
                    };
                    nodes.push(ParsedNode {
                        kind,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: owner().map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: rust_type_modifiers(child, context.source),
                        is_test: false,
                        extra: rust_type_extra(child, context.source),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: rust_container(&context.file_path, owner()),
                        target: qualified,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_emit_type_references(
                        child,
                        context,
                        &qualify(&context.file_path, &name, owner()),
                        Some(&name),
                        edges,
                    );
                    // `trait A: B + fmt::Display` -- supertraits.
                    if let Some(bounds) = child
                        .child_by_field_name("bounds")
                        .filter(|_| child.kind() == "trait_item")
                    {
                        let mut cursor = bounds.walk();
                        for bound in bounds.named_children(&mut cursor) {
                            let Some(target) = rust_receiver_type(bound, context.source) else {
                                continue;
                            };
                            edges.push(ParsedEdge {
                                kind: crate::core::types::EdgeKind::Inherits,
                                source: qualify(&context.file_path, &name, owner()),
                                target,
                                file_path: context.file_path.clone(),
                                line: child.start_position().row as i64 + 1,
                                extra: json!({
                                    "relationship_role": "supertrait",
                                    "syntax_source": "trait_item",
                                }),
                            });
                        }
                    }
                    let path = rust_scope_join(owner(), &name);
                    rust_walk_children(child, context, Some(&path), None, nodes, edges);
                    continue;
                }
            }
            "impl_item" if let Some(type_name) = rust_impl_type_name(child, context.source) => {
                let type_name = rust_scope_join(owner(), &type_name);
                if let Some(trait_name) = rust_impl_trait_name(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Implements,
                        source: qualify(&context.file_path, &type_name, None),
                        target: trait_name,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({
                            "relationship_role": "implements",
                            "syntax_source": "impl_item",
                        }),
                    });
                }
                rust_walk_children(child, context, Some(&type_name), None, nodes, edges);
                continue;
            }
            "function_item" | "function_signature_item" => {
                if let Some(name) = rust_identifier_child(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, owner());
                    let params = rust_child_text(child, context.source, "parameters");
                    let is_test =
                        is_test_function(&name, &context.file_path, child, context.source);
                    let mut extra = if child.kind() == "function_signature_item" {
                        json!({"is_abstract": true})
                    } else {
                        json!({})
                    };
                    if let Some(block) = rust_enclosing_foreign_block(child) {
                        // Declared in `extern "C" { ... }`: implemented on
                        // the other side, never exported from here. A cxx
                        // `extern "Rust"` block declares what C++ may call.
                        if let Some(export) = rust_cxx_rust_export(block, context.source, &name) {
                            extra["ffi_export"] = export;
                        } else if let Some(import) =
                            rust_foreign_ffi_import(child, block, context.source, &name)
                        {
                            extra["ffi_import"] = import;
                        }
                    } else if let Some(export) =
                        rust_function_ffi_export(child, context.source, &name)
                    {
                        extra["ffi_export"] = export;
                    } else if context.component_bindings
                        && let Some(export) = rust_component_export(child, context.source, &name)
                    {
                        extra["ffi_export"] = export;
                    }
                    nodes.push(ParsedNode {
                        kind: if is_test {
                            crate::core::types::NodeKind::Test
                        } else {
                            crate::core::types::NodeKind::Function
                        },
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: owner().map(str::to_string),
                        params,
                        return_type: None,
                        modifiers: None,
                        is_test,
                        extra,
                    });
                    let container = rust_container(&context.file_path, owner());
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: container,
                        target: qualified,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_emit_type_references(
                        child,
                        context,
                        &qualify(&context.file_path, &name, owner()),
                        Some(&name),
                        edges,
                    );
                    let snapshot = context.bindings.borrow().snapshot();
                    if enclosing_func.is_none()
                        && let Some(class_name) = enclosing_class
                    {
                        context
                            .bindings
                            .borrow_mut()
                            .bind_implicit_receivers(class_name);
                    }
                    rust_bind_parameters(child, context);
                    context
                        .locals
                        .borrow_mut()
                        .push(rust_local_bindings(child, context.source));
                    rust_walk_children(child, context, owner(), Some(&name), nodes, edges);
                    context.locals.borrow_mut().pop();
                    context.bindings.borrow_mut().restore(snapshot);
                    continue;
                }
            }
            "use_declaration" => rust_emit_use(child, context, edges),
            "call_expression" | "macro_invocation" => {
                let bound = rust_bound_member_target(child, context);
                // `x.m()` on a receiver whose type is unknown must not bind to
                // some other type's `m` by name.
                let receiver_unknown = bound.is_none()
                    && child
                        .child_by_field_name("function")
                        .is_some_and(|function| function.kind() == "field_expression");
                if let Some(call_name) = bound.or_else(|| rust_call_name(child, context.source)) {
                    let mut call_name = rust_rewrite_relative_path(call_name, enclosing_class);
                    // Macros are their own namespace: `matches!` is not `fn matches`.
                    if child.kind() == "macro_invocation" {
                        call_name.push('!');
                    }
                    let caller = enclosing_func
                        .map(|name| qualify(&context.file_path, name, enclosing_class))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Calls,
                        source: caller.clone(),
                        target: call_name.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: if receiver_unknown {
                            json!({"receiver_unknown": true})
                        } else {
                            json!({})
                        },
                    });
                    if let Some(edge) = rust_bridge_edge(
                        child,
                        context.source,
                        &context.file_path,
                        &caller,
                        &call_name,
                    ) {
                        edges.push(edge);
                    }
                    if child.kind() == "macro_invocation" {
                        let mut cursor = child.walk();
                        let arguments = child
                            .children(&mut cursor)
                            .find(|part| part.kind() == "token_tree");
                        if let Some(arguments) = arguments {
                            rust_emit_token_tree_calls(
                                arguments,
                                context,
                                enclosing_class,
                                &caller,
                                edges,
                            );
                        }
                    }
                }
            }

            "arguments" => {
                rust_emit_argument_references(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        rust_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        rust_bind_let(child, context);
    }
}

struct RustParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    /// Where the file sits in its crate, when the repository root is known.
    scope: Option<&'a modules::RustModuleScope<'a>>,
    /// Names `use` brings into scope: local name -> the path it names.
    uses: RefCell<HashMap<String, Vec<String>>>,
    /// A `use ...::*` of a module of this repository is in effect.
    repo_glob: std::cell::Cell<bool>,
    /// Free functions of this file (not methods), by name.
    free_functions: &'a HashSet<String>,
    /// Field types of the structs this file declares: struct -> field -> type.
    struct_fields: &'a HashMap<String, HashMap<String, String>>,
    /// Names bound by the enclosing function bodies (parameters, `let`,
    /// closure parameters, patterns), innermost last.
    locals: RefCell<Vec<HashSet<String>>>,
    defined_names: &'a HashSet<String>,
    bindings: RefCell<MemberCallBindings>,
    /// The file generates WebAssembly component bindings
    /// (`wit_bindgen::generate!`, `wasmtime::component::bindgen!`,
    /// cargo-component's `bindings` module).
    component_bindings: bool,
}

fn collect_rust_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            if let Some(name) = rust_type_name(node, source) {
                names.insert(name);
            }
        }
        "impl_item" => {
            if let Some(name) = rust_impl_type_name(node, source) {
                names.insert(name);
            }
        }
        "function_item" | "function_signature_item" => {
            if let Some(name) = rust_identifier_child(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_defined_names(child, source, names);
    }
}

fn collect_rust_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            if let Some(name) = rust_type_name(node, source) {
                names.insert(name);
            }
        }
        "impl_item" => {
            if let Some(name) = rust_impl_type_name(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_type_names(child, source, names);
    }
}

fn rust_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    rust_identifier_child(node, source)
}

fn rust_impl_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("type")
        .and_then(|ty| rust_type_ident(ty, source))
}

fn rust_impl_trait_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("trait")
        .and_then(|ty| rust_type_ident(ty, source))
}

fn rust_type_ident(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node_text(node, source)),
        "generic_type" | "scoped_type_identifier" | "pointer_type" | "reference_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if let Some(name) = rust_type_ident(child, source) {
                    return Some(name);
                }
            }
            None
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    return Some(node_text(child, source));
                }
            }
            None
        }
    }
}

fn rust_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "field_identifier"
        ) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_type_role(kind: &str) -> &'static str {
    match kind {
        "enum_item" => "enum",
        "trait_item" => "trait",
        "type_item" => "alias",
        "struct_item" => "struct",
        _ => "class",
    }
}

fn rust_type_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> serde_json::Value {
    let type_role = rust_type_role(node.kind());
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "trait" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
        if rust_is_value_container(type_role, node, source) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    if let Some(derive_traits) = rust_derive_traits(node, source) {
        // `#[derive(uniffi::Object)]` (or `Record` / `Enum` / `Error`) is a
        // type the foreign bindings expose.
        if derive_traits.iter().any(|name| {
            matches!(
                name.as_str(),
                "uniffi::Object" | "uniffi::Record" | "uniffi::Enum" | "uniffi::Error"
            )
        }) && let Some(type_name) = rust_type_name(node, source)
        {
            extra["ffi_export"] = json!({"abi": "uniffi", "kind": "class", "name": type_name});
        }
        extra["derive_traits"] = json!(derive_traits);
    }
    if node.kind() == "struct_item" || node.kind() == "enum_item" {
        let attrs = rust_leading_attribute_texts(node, source);
        if let Some(attr) = attrs.iter().find(|attr| rust_attr_is(attr, "pyclass")) {
            let name = rust_attr_string_arg(attr, "name")
                .or_else(|| rust_type_name(node, source))
                .unwrap_or_default();
            extra["ffi_export"] = json!({"abi": "pyo3", "kind": "class", "name": name});
        } else if let Some(attr) = attrs.iter().find(|attr| rust_attr_is(attr, "wasm_bindgen")) {
            let name = rust_attr_string_arg(attr, "js_name")
                .or_else(|| rust_type_name(node, source))
                .unwrap_or_default();
            extra["ffi_export"] = json!({"abi": "wasm", "kind": "class", "name": name});
        } else if let Some(attr) = attrs.iter().find(|attr| rust_attr_is(attr, "napi"))
            // `#[napi(object)]` is a plain object type, not a class.
            && !rust_attr_has_flag(attr, "object")
        {
            let name = rust_attr_string_arg(attr, "js_name")
                .or_else(|| rust_type_name(node, source))
                .unwrap_or_default();
            extra["ffi_export"] = json!({"abi": "napi", "kind": "class", "name": name});
        }
    }
    extra
}

static RUST_ATTR_STRING_ARG_RE: LazyLock<Regex> = LazyLock::new(|| {
    // `key = "value"`, or `key = ident` (wasm-bindgen's `js_name = meanOf`).
    Regex::new(r#"\b(\w+)\s*=\s*(?:"([^"]*)"|([A-Za-z_$][\w$]*))"#).expect("valid regex")
});

/// Texts of the outer attributes written before *node*, whitespace removed
/// (`#[pyo3(name="x")]`).
fn rust_leading_attribute_texts(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    rust_node_with_leading_attributes(node)
        .filter(|candidate| candidate.kind() == "attribute_item")
        .map(|attr| {
            node_text(attr, source)
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect()
        })
        .collect()
}

/// True for `#[path]`, `#[path(...)]`, `#[unsafe(path)]`, and the
/// `pyo3::`-qualified spellings.
fn rust_attr_is(attr: &str, name: &str) -> bool {
    let Some(inner) = attr
        .strip_prefix("#[")
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    let inner = inner
        .strip_prefix("unsafe(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(inner);
    let path = inner.split(['(', '=']).next().unwrap_or_default();
    path == name || path.rsplit("::").next() == Some(name)
}

/// True when the attribute's argument list contains the bare word *flag*
/// (`#[napi(object)]`, `#[napi(constructor)]`).
fn rust_attr_has_flag(attr: &str, flag: &str) -> bool {
    attr.split_once('(')
        .map(|(_, args)| args.trim_end_matches([']', ')']))
        .is_some_and(|args| args.split(',').any(|arg| arg == flag))
}

fn rust_attr_string_arg(attr: &str, key: &str) -> Option<String> {
    RUST_ATTR_STRING_ARG_RE
        .captures_iter(attr)
        .find(|captures| &captures[1] == key)
        .and_then(|captures| captures.get(2).or_else(|| captures.get(3)))
        .map(|value| value.as_str().to_string())
}

/// The `impl` block directly containing a method.
fn rust_enclosing_impl(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    node.parent()
        .filter(|parent| parent.kind() == "declaration_list")
        .and_then(|list| list.parent())
        .filter(|item| item.kind() == "impl_item")
}

fn rust_node_with_leading_attributes(
    node: tree_sitter::Node<'_>,
) -> impl Iterator<Item = tree_sitter::Node<'_>> {
    let mut attrs = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        if matches!(sibling.kind(), "attribute_item" | "inner_attribute_item") {
            attrs.push(sibling);
            current = sibling.prev_sibling();
            continue;
        }
        break;
    }
    attrs.reverse();
    attrs.into_iter().chain(std::iter::once(node))
}

fn rust_is_value_container(type_role: &str, node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    matches!(type_role, "struct" | "enum") || rust_derives_value_semantics(node, source)
}

fn rust_derives_value_semantics(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    rust_derive_traits(node, source).is_some_and(|traits| {
        traits
            .iter()
            .any(|name| matches!(name.as_str(), "Serialize" | "Deserialize"))
    })
}

fn rust_type_modifiers(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut modifiers = Vec::new();
    if rust_has_pub_visibility(node, source) {
        modifiers.push("pub");
    }
    if modifiers.is_empty() {
        None
    } else {
        Some(modifiers.join(" "))
    }
}

fn rust_has_pub_visibility(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    for candidate in rust_node_with_leading_attributes(node) {
        let mut cursor = candidate.walk();
        for child in candidate.children(&mut cursor) {
            if child.kind() == "visibility_modifier" {
                let text = node_text(child, source);
                if text.starts_with("pub") {
                    return true;
                }
            }
        }
    }
    false
}

fn rust_derive_traits(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut traits = Vec::new();
    for candidate in rust_node_with_leading_attributes(node) {
        let texts = if matches!(candidate.kind(), "attribute_item" | "inner_attribute_item") {
            vec![node_text(candidate, source)]
        } else {
            let mut cursor = candidate.walk();
            candidate
                .children(&mut cursor)
                .filter(|child| matches!(child.kind(), "attribute_item" | "inner_attribute_item"))
                .map(|child| node_text(child, source))
                .collect::<Vec<_>>()
        };
        for text in texts {
            let Some(args) = text.strip_prefix("#[derive(") else {
                continue;
            };
            let Some(args) = args.strip_suffix(")]") else {
                continue;
            };
            for trait_name in args.split(',') {
                let trimmed = trait_name.trim();
                if !trimmed.is_empty() {
                    traits.push(trimmed.to_string());
                }
            }
        }
    }
    if traits.is_empty() {
        None
    } else {
        Some(traits)
    }
}

/// Rewrites `Self::f` and `super::f` against the enclosing scope path so the
/// type-scoped resolver can match them.
fn rust_rewrite_relative_path(call_name: String, enclosing: Option<&str>) -> String {
    if let (Some(rest), Some(scope)) = (call_name.strip_prefix("Self::"), enclosing) {
        return format!("{scope}::{rest}");
    }
    // `super::f` inside an inline `mod a { mod b { .. } }` of this file names
    // `a::f` (or the file's own `f` from `mod a`). Outside inline modules it
    // names the parent module, which module resolution handles. Scope
    // segments that are types (UpperCamel) are not modules.
    if let Some(rest) = call_name.strip_prefix("super::")
        && let Some(scope) = enclosing
    {
        let mut modules: Vec<&str> = scope.split('.').collect();
        while modules
            .last()
            .is_some_and(|segment| segment.starts_with(|c: char| c.is_ascii_uppercase()))
        {
            modules.pop();
        }
        if modules.pop().is_some() {
            return if modules.is_empty() {
                rest.to_string()
            } else {
                format!("{}::{rest}", modules.join("."))
            };
        }
    }
    call_name
}

/// Undoes a same-file resolution Rust's scoping rules forbid. A bare call
/// `helper()` names a function of the caller's own body or of an enclosing
/// module, never a method (`Self::helper` / `self.helper()` would be
/// written) nor a function of another module such as `mod tests`.
fn rust_keep_bare_calls_in_scope(
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
    bare: &[bool],
    file_path: &FilePath,
) {
    let prefix = format!("{file_path}::");
    let path_of = |node: &ParsedNode| match &node.parent_name {
        Some(parent) => format!("{parent}.{}", node.name),
        None => node.name.clone(),
    };
    // Scopes a bare name can reach: modules and function bodies.
    let reachable: HashSet<String> = nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                crate::core::types::NodeKind::Function | crate::core::types::NodeKind::Test
            ) || node.extra.get("type_role").and_then(|role| role.as_str()) == Some("module")
        })
        .map(path_of)
        .collect();
    for (edge, was_bare) in edges.iter_mut().zip(bare) {
        if !*was_bare {
            continue;
        }
        let Some(symbol) = edge.target.strip_prefix(&prefix) else {
            continue;
        };
        let Some((parent, name)) = symbol.rsplit_once('.') else {
            continue;
        };
        let caller = edge.source.strip_prefix(&prefix).unwrap_or("");
        let in_scope = reachable.contains(parent)
            && (caller == parent || caller.starts_with(&format!("{parent}.")));
        if !in_scope {
            edge.target = name.to_string();
        }
    }
}

/// Rewrites the call targets same-file resolution left alone into forms
/// resolution across files can match:
///
/// - `Type::method` (a bound receiver or a `Type::f()` path whose type is
///   declared elsewhere) -> `method` with `receiver_type: "Type"`;
/// - `module::f` / `crate::a::f` / `super::f` -> `f` with `module_file`, the
///   file of the module, when it is in this repository;
/// - a name imported under an alias (`use a::f as g; g()`) -> `f`.
fn rust_finish_call_targets(
    edges: &mut [ParsedEdge],
    file_path: &FilePath,
    scope: Option<&modules::RustModuleScope<'_>>,
    uses: &HashMap<String, Vec<String>>,
) {
    let is_type = |segment: &str| segment.starts_with(|c: char| c.is_ascii_uppercase());
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::Calls
            || edge.target.starts_with(file_path.as_str())
        {
            continue;
        }
        let mut segments: Vec<String> = edge.target.split("::").map(str::to_string).collect();
        if segments.len() == 1 {
            if let Some(path) = uses.get(&segments[0])
                && let Some(original) = path.last()
                && original != &segments[0]
            {
                edge.extra["alias"] = json!(segments[0]);
                edge.target = original.clone();
            }
            continue;
        }
        // `helpers::f` where `use crate::helpers;` brought `helpers` in.
        if let Some(path) = uses.get(&segments[0]) {
            let mut expanded = path.clone();
            expanded.extend(segments.drain(1..));
            segments = expanded;
        }
        let method = segments.pop().expect("at least two segments");
        let owner = segments.last().cloned().unwrap_or_default();
        if is_type(&owner) {
            // A type from another crate (`io::Error::new`, `Instant::now()`
            // with `use std::time::Instant`) must not bind to a same-named
            // type of this repository.
            let type_path = if segments.len() > 1 {
                Some(segments.clone())
            } else {
                uses.get(&owner).cloned()
            };
            let external = match (&type_path, scope) {
                (Some(path), Some(scope)) => scope.resolve(path).is_none(),
                (Some(path), None) => path.len() > 1,
                (None, _) => false,
            };
            if owner != "Self" && !external {
                edge.extra["receiver_type"] = json!(owner);
                edge.target = method;
            }
            continue;
        }
        let Some(resolved) = scope.and_then(|scope| scope.resolve(&segments)) else {
            continue;
        };
        if resolved.rest.is_empty() {
            edge.extra["path"] = json!(edge.target);
            edge.extra["module_file"] = json!(resolved.file);
            edge.target = method;
        }
    }
}

fn rust_scope_join(enclosing: Option<&str>, name: &str) -> String {
    match enclosing {
        Some(parent) => format!("{parent}.{name}"),
        None => name.to_string(),
    }
}

fn rust_container(file_path: &FilePath, enclosing: Option<&str>) -> String {
    enclosing
        .map(|name| qualify(file_path, name, None))
        .unwrap_or_else(|| file_path.to_string())
}

/// Expands a `use` tree into one path per imported item, with its local
/// alias: `use a::{b, c::d as e, self}` yields `a::b`, `a::c::d as e`, `a`.
fn rust_use_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<(String, Option<String>)> {
    let mut targets = Vec::new();
    if let Some(argument) = node.child_by_field_name("argument") {
        rust_collect_use_tree(argument, source, "", None, &mut targets);
    }
    targets
}

fn rust_collect_use_tree(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    prefix: &str,
    alias: Option<String>,
    targets: &mut Vec<(String, Option<String>)>,
) {
    let join = |tail: &str| {
        if prefix.is_empty() {
            tail.to_string()
        } else {
            format!("{prefix}::{tail}")
        }
    };
    match node.kind() {
        "use_as_clause" => {
            let alias = node
                .child_by_field_name("alias")
                .map(|alias| node_text(alias, source).trim().to_string());
            if let Some(path) = node.child_by_field_name("path") {
                rust_collect_use_tree(path, source, prefix, alias, targets);
            }
        }
        "scoped_use_list" => {
            let nested = node
                .child_by_field_name("path")
                .map(|path| join(node_text(path, source).trim()))
                .unwrap_or_else(|| prefix.to_string());
            if let Some(list) = node.child_by_field_name("list") {
                rust_collect_use_tree(list, source, &nested, None, targets);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for item in node.named_children(&mut cursor) {
                rust_collect_use_tree(item, source, prefix, None, targets);
            }
        }
        "self" if !prefix.is_empty() => targets.push((prefix.to_string(), alias)),
        "line_comment" | "block_comment" => {}
        _ => {
            let text = node_text(node, source);
            let text = text.trim();
            if !text.is_empty() {
                targets.push((join(text), alias));
            }
        }
    }
}

/// Names one `use` imports from one module file: the module as written, the
/// `(name, local alias)` pairs, and whether it globs.
type UseGroup = (String, Vec<(String, String)>, bool);

/// One IMPORTS_FROM per module a `use` names: the module's file when it is
/// in this repository (`use crate::util::node_text` -> `src/util.rs`, with
/// `names: [["node_text", "node_text"]]`), else the path as written
/// (`std::collections::HashMap`). `pub use` re-exports and `*` globs are
/// marked so name resolution can see through them.
fn rust_emit_use(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    edges: &mut Vec<ParsedEdge>,
) {
    let re_export = node
        .named_child(0)
        .is_some_and(|first| first.kind() == "visibility_modifier");
    let line = node.start_position().row as i64 + 1;
    // file -> (module as written, [(name, alias)], glob)
    let mut by_file: BTreeMap<String, UseGroup> = BTreeMap::new();
    for (path, alias) in rust_use_targets(node, context.source) {
        let segments: Vec<String> = path.split("::").map(|s| s.trim().to_string()).collect();
        let local = alias
            .clone()
            .unwrap_or_else(|| segments.last().cloned().unwrap_or_default());
        if local != "*" && local != "_" {
            context
                .uses
                .borrow_mut()
                .insert(local.clone(), segments.clone());
        }
        let resolved = context.scope.and_then(|scope| scope.resolve(&segments));
        match resolved {
            Some(resolved) => {
                let module = segments[..segments.len() - resolved.rest.len()].join("::");
                let entry = by_file
                    .entry(resolved.file)
                    .or_insert_with(|| (module, Vec::new(), false));
                match resolved.rest.first().map(String::as_str) {
                    Some("*") => {
                        entry.2 = true;
                        context.repo_glob.set(true);
                    }
                    Some(name) => entry.1.push((name.to_string(), local)),
                    None => {}
                }
            }
            None => edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom,
                source: context.file_path.to_string(),
                target: path,
                file_path: context.file_path.clone(),
                line,
                extra: json!({}),
            }),
        }
    }
    for (file, (module, names, glob)) in by_file {
        let mut extra = json!({"module": module});
        if !names.is_empty() {
            extra["names"] = json!(names);
        }
        if glob {
            extra["glob"] = json!(true);
        }
        if re_export {
            extra["re_export"] = json!(true);
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: context.file_path.to_string(),
            target: file,
            file_path: context.file_path.clone(),
            line,
            extra,
        });
    }
}

fn rust_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "scoped_identifier" => return Some(node_text(child, source)),
            "field_expression" => return rust_rightmost_identifier(child, source),
            _ => {}
        }
    }
    None
}

fn rust_bound_member_target(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
) -> Option<String> {
    let mut cursor = node.walk();
    let field = node
        .children(&mut cursor)
        .find(|child| child.kind() == "field_expression")?;
    let method = rust_rightmost_identifier(field, context.source)?;
    let receiver = field.child_by_field_name("value")?;
    rust_receiver_owner(receiver, context).map(|ty| format!("{ty}::{method}"))
}

/// The type of a method call's receiver when the syntax gives it away: a
/// bound variable or `self`, an enum variant (`ConfidenceTier::High`), or a
/// field of a struct this file declares (`self.conn`, `context.file_path`).
fn rust_receiver_owner(
    receiver: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
) -> Option<String> {
    let source = context.source;
    let variable_type = |name: &str| {
        let bindings = context.bindings.borrow();
        bindings
            .bound_type(name)
            .filter(|ty| !ty.contains("::"))
            .or_else(|| bindings.foreign_type(name))
            .map(str::to_string)
    };
    match receiver.kind() {
        "identifier" | "self" => variable_type(&node_text(receiver, source)),
        "scoped_identifier" => {
            let path = receiver.child_by_field_name("path")?;
            let name = node_text(receiver.child_by_field_name("name")?, source);
            let owner = match path.kind() {
                "identifier" | "type_identifier" => node_text(path, source),
                "scoped_identifier" => node_text(path.child_by_field_name("name")?, source),
                _ => return None,
            };
            let is_type = |value: &str| value.starts_with(|c: char| c.is_ascii_uppercase());
            if !is_type(&name) {
                return None;
            }
            if owner == "Self" {
                return variable_type("Self");
            }
            is_type(&owner).then_some(owner)
        }
        // `Store::open().save()`, `Store::open(p)?.save()`.
        "call_expression" | "try_expression" => rust_constructed_type(receiver, source),
        "field_expression" => {
            let owner = rust_receiver_owner(receiver.child_by_field_name("value")?, context)?;
            let owner = owner.rsplit(['.', ':']).next().unwrap_or(&owner);
            let field = node_text(receiver.child_by_field_name("field")?, source);
            context.struct_fields.get(owner)?.get(&field).cloned()
        }
        _ => None,
    }
}

/// `struct S { conn: Connection, store: &'a GraphStore }` -> S -> field -> type.
fn collect_rust_struct_fields(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> HashMap<String, HashMap<String, String>> {
    fn visit(
        node: tree_sitter::Node<'_>,
        source: &[u8],
        out: &mut HashMap<String, HashMap<String, String>>,
    ) {
        if node.kind() == "struct_item"
            && let Some(name) = node
                .child_by_field_name("name")
                .map(|n| node_text(n, source))
            && let Some(body) = node.child_by_field_name("body")
        {
            let mut fields = HashMap::new();
            let mut cursor = body.walk();
            for field in body.named_children(&mut cursor) {
                if field.kind() != "field_declaration" {
                    continue;
                }
                if let (Some(field_name), Some(ty)) = (
                    field.child_by_field_name("name"),
                    field.child_by_field_name("type"),
                ) && let Some(ty) = rust_receiver_type(ty, source)
                {
                    fields.insert(node_text(field_name, source), ty);
                }
            }
            out.insert(name, fields);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, source, out);
        }
    }
    let mut out = HashMap::new();
    visit(root, source, &mut out);
    out
}

/// Calls in a macro's arguments. tree-sitter keeps them as a flat token tree
/// (`assert_eq!(f(&x), 1)`, `json!({"a": g(x)})`, `format!("{}", x.m())`),
/// so they are read from the tokens: a path followed by `(...)` is a call, a
/// `.name(...)` a method call, a `name!(...)` a nested macro. Tagged
/// `in_macro`.
fn rust_emit_token_tree_calls(
    tree: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    enclosing_class: Option<&str>,
    caller: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let source = context.source;
    let mut cursor = tree.walk();
    let tokens: Vec<tree_sitter::Node<'_>> = tree.children(&mut cursor).collect();
    let text = |index: usize| tokens.get(index).map(|token| node_text(*token, source));
    let is_segment = |kind: &str| matches!(kind, "identifier" | "self" | "super" | "crate");
    let opens_call = |index: usize| {
        tokens.get(index).is_some_and(|token| {
            token.kind() == "token_tree"
                && token
                    .child(0)
                    .is_some_and(|open| node_text(open, source) == "(")
        })
    };
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token.kind() == "token_tree" {
            rust_emit_token_tree_calls(token, context, enclosing_class, caller, edges);
            index += 1;
            continue;
        }
        if !is_segment(token.kind()) {
            index += 1;
            continue;
        }
        let start = index;
        let mut segments = vec![node_text(token, source)];
        let mut end = index;
        loop {
            if text(end + 1).as_deref() != Some("::") {
                break;
            }
            // Turbofish: `f::<T>(...)`.
            if text(end + 2).as_deref() == Some("<") {
                let mut depth = 0;
                let mut cursor = end + 2;
                while cursor < tokens.len() {
                    match text(cursor).as_deref() {
                        Some("<") => depth += 1,
                        Some(">") => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    cursor += 1;
                }
                end = cursor;
                continue;
            }
            match tokens.get(end + 2) {
                Some(next) if is_segment(next.kind()) => {
                    segments.push(node_text(*next, source));
                    end += 2;
                }
                _ => break,
            }
        }
        index = end + 1;
        let previous = start.checked_sub(1).and_then(text);
        if matches!(
            previous.as_deref(),
            Some("fn" | "struct" | "enum" | "trait" | "mod" | "impl")
        ) {
            continue;
        }
        let line = token.start_position().row as i64 + 1;
        let (target, mut extra) = if text(end + 1).as_deref() == Some("!") && opens_call(end + 2) {
            (format!("{}!", segments.join("::")), json!({}))
        } else if !opens_call(end + 1) {
            continue;
        } else if previous.as_deref() == Some(".") {
            if segments.len() != 1 {
                continue;
            }
            let method = segments.pop().expect("one segment");
            let receiver = start
                .checked_sub(2)
                .and_then(|at| tokens.get(at))
                .filter(|receiver| matches!(receiver.kind(), "identifier" | "self"))
                .map(|receiver| node_text(*receiver, source));
            let bindings = context.bindings.borrow();
            match receiver.as_deref().and_then(|receiver| {
                bindings.resolve_member(receiver, &method).or_else(|| {
                    bindings
                        .foreign_type(receiver)
                        .map(|ty| format!("{ty}::{method}"))
                })
            }) {
                Some(bound) => (bound, json!({})),
                None => (method, json!({"receiver_unknown": true})),
            }
        } else {
            (
                rust_rewrite_relative_path(segments.join("::"), enclosing_class),
                json!({}),
            )
        };
        extra["in_macro"] = json!(true);
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.to_string(),
            target,
            file_path: context.file_path.clone(),
            line,
            extra,
        });
    }
}

/// The type whose methods a value of this annotated type answers:
/// `&mut GraphStore` -> `GraphStore`, `Box<Repo>` / `Rc<Repo>` / `Arc<Repo>`
/// -> `Repo` (auto-deref), `impl Store` / `dyn Store` -> `Store`.
fn rust_receiver_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node_text(node, source)),
        "scoped_type_identifier" => node
            .child_by_field_name("name")
            .map(|name| node_text(name, source)),
        "reference_type" | "pointer_type" => {
            rust_receiver_type(node.child_by_field_name("type")?, source)
        }
        "generic_type" => {
            let base = rust_receiver_type(node.child_by_field_name("type")?, source)?;
            if matches!(base.as_str(), "Box" | "Rc" | "Arc") {
                let arguments = node.child_by_field_name("type_arguments")?;
                let mut cursor = arguments.walk();
                let inner = arguments.named_children(&mut cursor).next()?;
                rust_receiver_type(inner, source)
            } else {
                Some(base)
            }
        }
        "abstract_type" | "dynamic_type" => {
            rust_receiver_type(node.child_by_field_name("trait")?, source)
        }
        _ => None,
    }
}

/// Binds typed parameters (`store: &mut GraphStore`) for method calls on
/// them.
fn rust_bind_parameters(function: tree_sitter::Node<'_>, context: &RustParseContext<'_>) {
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter.kind() != "parameter" {
            continue;
        }
        let (Some(pattern), Some(ty)) = (
            parameter.child_by_field_name("pattern"),
            parameter.child_by_field_name("type"),
        ) else {
            continue;
        };
        let pattern = if pattern.kind() == "mut_pattern" {
            match pattern.named_child(0) {
                Some(inner) => inner,
                None => continue,
            }
        } else {
            pattern
        };
        if pattern.kind() != "identifier" {
            continue;
        }
        if let Some(type_name) = rust_receiver_type(ty, context.source) {
            context
                .bindings
                .borrow_mut()
                .bind_any(node_text(pattern, context.source), type_name);
        }
    }
}

/// The constructed type of `Type::new(..)`, also through `?`, `.unwrap()`,
/// `.expect(..)`, `.unwrap_or_default()` (`GraphStore::open(p).expect("x")`),
/// and `Type { .. }`. A path whose type segment is UpperCamel names a type by
/// Rust convention.
fn rust_constructed_type(value: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut value = value;
    loop {
        match value.kind() {
            "try_expression" | "await_expression" | "parenthesized_expression" => {
                value = value.named_child(0)?;
            }
            "call_expression" => {
                let function = value.child_by_field_name("function")?;
                match function.kind() {
                    "field_expression" => {
                        let field = function.child_by_field_name("field")?;
                        if !matches!(
                            node_text(field, source).as_str(),
                            "unwrap" | "expect" | "unwrap_or_default"
                        ) {
                            return None;
                        }
                        value = function.child_by_field_name("value")?;
                    }
                    "scoped_identifier" => {
                        let path = function.child_by_field_name("path")?;
                        // The whole path for `std::time::Instant::now()`, so
                        // a type of another crate is recognised as such.
                        let (type_name, full) = match path.kind() {
                            "identifier" | "type_identifier" => {
                                let name = node_text(path, source);
                                (name.clone(), name)
                            }
                            "scoped_identifier" => (
                                node_text(path.child_by_field_name("name")?, source),
                                node_text(path, source).split_whitespace().collect(),
                            ),
                            "generic_type" => {
                                let name = rust_receiver_type(path, source)?;
                                (name.clone(), name)
                            }
                            _ => return None,
                        };
                        return (type_name != "Self"
                            && type_name.starts_with(|c: char| c.is_ascii_uppercase()))
                        .then_some(full);
                    }
                    _ => return None,
                }
            }
            "struct_expression" => {
                return rust_receiver_type(value.child_by_field_name("name")?, source);
            }
            _ => return None,
        }
    }
}

fn rust_bind_let(node: tree_sitter::Node<'_>, context: &RustParseContext<'_>) {
    if node.kind() != "let_declaration" {
        return;
    }
    let mut ident = node.child_by_field_name("pattern").and_then(|pattern| {
        if pattern.kind() == "identifier" {
            Some(node_text(pattern, context.source))
        } else {
            rust_identifier_child(pattern, context.source)
        }
    });
    let mut annotated = node
        .child_by_field_name("type")
        .and_then(|ty| rust_type_ident(ty, context.source));
    let mut value = node.child_by_field_name("value");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if ident.is_none() => {
                ident = Some(node_text(child, context.source));
            }
            "type_identifier" if annotated.is_none() => {
                annotated = Some(node_text(child, context.source));
            }
            "generic_type" | "scoped_type_identifier" | "reference_type" if annotated.is_none() => {
                if let Some(name) = rust_type_ident(child, context.source) {
                    annotated = Some(name);
                }
            }
            "call_expression" | "struct_expression" | "macro_invocation" if value.is_none() => {
                value = Some(child);
            }
            _ => {}
        }
    }
    let Some(ident) = ident else {
        return;
    };
    if let Some(value) = value
        && let Some(call_name) =
            rust_call_name(value, context.source).or_else(|| rust_type_ident(value, context.source))
    {
        let type_name = context
            .bindings
            .borrow()
            .constructor_type(&call_name)
            .map(str::to_string);
        if let Some(type_name) = type_name {
            context.bindings.borrow_mut().bind(ident, type_name);
            return;
        }
    }
    if let Some(type_name) = node
        .child_by_field_name("type")
        .and_then(|ty| rust_receiver_type(ty, context.source))
        .or_else(|| value.and_then(|value| rust_constructed_type(value, context.source)))
    {
        context.bindings.borrow_mut().bind_any(ident, type_name);
        return;
    }
    if let Some(type_name) = annotated {
        context.bindings.borrow_mut().bind(ident, type_name);
    }
}

fn rust_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = rust_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

/// A function passed as a value (`rows.map(edge_from_row)`): REFERENCES to
/// it. Only a free function counts (a method is `Type::m`), and a name a
/// local binding shadows (`node_to_value(node)`) is the local. A function a
/// `use` brought in is left for resolution across files (`value_reference`,
/// with the module's file when known).
fn rust_emit_argument_references(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let file_path = &context.file_path;
    let caller = enclosing_func
        .map(|name| qualify(file_path, name, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "identifier" {
            continue;
        }
        let name = node_text(child, context.source);
        if rust_should_skip_value_reference(&name)
            || context
                .locals
                .borrow()
                .iter()
                .any(|scope| scope.contains(&name))
        {
            continue;
        }
        let line = child.start_position().row as i64 + 1;
        if context.free_functions.contains(&name) {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::References,
                source: caller.clone(),
                target: qualify(file_path, &name, None),
                file_path: file_path.clone(),
                line,
                extra: json!({}),
            });
            continue;
        }
        // Unbound and snake_case: a function a glob import (`use
        // crate::helpers::*`) may have brought in, left to resolution.
        let path = context.uses.borrow().get(&name).cloned();
        if path.is_none() && !name.starts_with(|c: char| c.is_ascii_lowercase()) {
            continue;
        }
        let path = path.unwrap_or_else(|| vec![name.clone()]);
        let original = path.last().cloned().unwrap_or_else(|| name.clone());
        let mut extra = json!({"value_reference": true});
        if path.len() > 1
            && let Some(resolved) = context.scope.and_then(|scope| scope.resolve(&path))
            && resolved.rest.len() == 1
        {
            extra["module_file"] = json!(resolved.file);
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::References,
            source: caller.clone(),
            target: original,
            file_path: file_path.clone(),
            line,
            extra,
        });
    }
}

/// Free functions of the file: `fn`s outside `impl` and `trait` blocks.
fn collect_rust_free_functions(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "impl_item" | "trait_item" => return,
        "function_item" => {
            if let Some(name) = rust_identifier_child(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_free_functions(child, source, names);
    }
}

/// Names a function body binds: parameters, `let` / `if let` / `while let` /
/// `for` / `match` patterns, and closure parameters. Nested `fn`s are their
/// own scope.
fn rust_local_bindings(function: tree_sitter::Node<'_>, source: &[u8]) -> HashSet<String> {
    fn pattern_names(node: tree_sitter::Node<'_>, source: &[u8], names: &mut HashSet<String>) {
        if node.kind() == "identifier" {
            names.insert(node_text(node, source));
            return;
        }
        // Types in closure parameters (`|x: Node|`) and paths in patterns
        // (`Some(x)`, `Kind::A { f }`) name no binding.
        if matches!(
            node.kind(),
            "scoped_identifier" | "type_identifier" | "primitive_type" | "generic_type"
        ) {
            return;
        }
        let mut cursor = node.walk();
        for (index, child) in node.children(&mut cursor).enumerate() {
            let field = node.field_name_for_child(index as u32);
            if matches!(field, Some("type" | "function")) {
                continue;
            }
            pattern_names(child, source, names);
        }
    }
    fn visit(node: tree_sitter::Node<'_>, source: &[u8], names: &mut HashSet<String>) {
        match node.kind() {
            "let_declaration" | "let_condition" | "for_expression" | "match_arm" | "parameter" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    pattern_names(pattern, source, names);
                }
            }
            "closure_parameters" => pattern_names(node, source, names),
            "function_item" => return,
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, source, names);
        }
    }
    let mut names = HashSet::new();
    let mut cursor = function.walk();
    for child in function.children(&mut cursor) {
        visit(child, source, &mut names);
    }
    names
}

/// Types named in an item (signature and body): REFERENCES to each. A type
/// declared in this file is its node; one a `use` brought in from this
/// repository (`use crate::types::ParsedEdge`, `types::ParsedEdge`) or that a
/// glob of a repository module may have (`use super::*`) is left for
/// resolution across files (`value_reference`, with `module_file` when the
/// module is known). An enum variant path (`EdgeKind::Calls`) names its
/// type.
fn rust_emit_type_references(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    source_qualified: &str,
    skip_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut emitted = HashSet::new();
    rust_collect_type_references(
        node,
        context,
        source_qualified,
        skip_name,
        edges,
        &mut emitted,
    );
}

/// Standard library types that no glob of this repository provides.
const RUST_STD_TYPES: &[&str] = &[
    "String", "Vec", "Option", "Result", "Box", "Rc", "Arc", "Cell", "RefCell", "Mutex", "RwLock",
    "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque", "Cow", "Path", "PathBuf", "Self",
    "Some", "None", "Ok", "Err", "Duration", "Instant", "Ordering",
];

fn rust_collect_type_references(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    source_qualified: &str,
    skip_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
    emitted: &mut HashSet<String>,
) {
    let source = context.source;
    let is_type = |name: &str| name.starts_with(|c: char| c.is_ascii_uppercase());
    // (type name, module path it was named through)
    let named: Option<(String, Vec<String>)> = match node.kind() {
        "type_identifier" => Some((node_text(node, source), Vec::new())),
        "scoped_type_identifier" => node.child_by_field_name("name").map(|name| {
            let path = node
                .child_by_field_name("path")
                .map(|path| node_text(path, source))
                .unwrap_or_default();
            (
                node_text(name, source),
                path.split("::")
                    .map(|segment| segment.trim().to_string())
                    .collect(),
            )
        }),
        // `EdgeKind::Calls`, `types::EdgeKind::Calls`.
        "scoped_identifier" => node
            .child_by_field_name("name")
            .filter(|name| is_type(&node_text(*name, source)))
            .and_then(|_| node.child_by_field_name("path"))
            .and_then(|path| {
                let text = node_text(path, source);
                let mut segments: Vec<String> = text
                    .split("::")
                    .map(|segment| segment.trim().to_string())
                    .collect();
                let name = segments.pop()?;
                is_type(&name).then_some((name, segments))
            }),
        "token_tree" => {
            // Macro arguments are tokens: `vec![ParsedNode { kind:
            // crate::types::NodeKind::File }]` names ParsedNode and NodeKind.
            let mut cursor = node.walk();
            let tokens: Vec<tree_sitter::Node<'_>> = node.children(&mut cursor).collect();
            let mut index = 0;
            while index < tokens.len() {
                let token = tokens[index];
                if token.kind() == "token_tree" {
                    rust_collect_type_references(
                        token,
                        context,
                        source_qualified,
                        skip_name,
                        edges,
                        emitted,
                    );
                    index += 1;
                    continue;
                }
                if !matches!(token.kind(), "identifier" | "crate" | "self" | "super") {
                    index += 1;
                    continue;
                }
                let mut segments = vec![node_text(token, source)];
                let mut end = index;
                while tokens
                    .get(end + 1)
                    .is_some_and(|sep| node_text(*sep, source) == "::")
                    && tokens.get(end + 2).is_some_and(|next| {
                        matches!(next.kind(), "identifier" | "crate" | "self" | "super")
                    })
                {
                    segments.push(node_text(tokens[end + 2], source));
                    end += 2;
                }
                index = end + 1;
                if let Some(at) = segments.iter().position(|segment| is_type(segment)) {
                    rust_emit_type_reference(
                        token,
                        segments[at].clone(),
                        segments[..at].to_vec(),
                        context,
                        source_qualified,
                        skip_name,
                        edges,
                        emitted,
                    );
                }
            }
            return;
        }
        _ => None,
    };
    if let Some((name, module)) = named {
        rust_emit_type_reference(
            node,
            name,
            module,
            context,
            source_qualified,
            skip_name,
            edges,
            emitted,
        );
    }
    if matches!(node.kind(), "scoped_type_identifier" | "scoped_identifier") {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        rust_collect_type_references(child, context, source_qualified, skip_name, edges, emitted);
    }
}

#[allow(clippy::too_many_arguments)]
fn rust_emit_type_reference(
    node: tree_sitter::Node<'_>,
    name: String,
    module: Vec<String>,
    context: &RustParseContext<'_>,
    source_qualified: &str,
    skip_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
    emitted: &mut HashSet<String>,
) {
    let is_type = |name: &str| name.starts_with(|c: char| c.is_ascii_uppercase());
    if skip_name != Some(name.as_str()) && is_type(&name) && emitted.insert(name.clone()) {
        let line = node.start_position().row as i64 + 1;
        let mut extra = json!({
            "relationship_role": "type_reference",
            "evidence_kind": "rust_type_identifier"
        });
        let target = if module.is_empty() && context.defined_names.contains(&name) {
            Some(qualify(&context.file_path, &name, None))
        } else {
            let path = if module.is_empty() {
                context.uses.borrow().get(&name).cloned()
            } else {
                // `types::ParsedEdge` where `use super::types;` named `types`.
                let mut path = context
                    .uses
                    .borrow()
                    .get(&module[0])
                    .cloned()
                    .unwrap_or_else(|| vec![module[0].clone()]);
                path.extend(module[1..].iter().cloned());
                path.push(name.clone());
                Some(path)
            };
            match path {
                Some(path) => context
                    .scope
                    .and_then(|scope| scope.resolve(&path))
                    .filter(|resolved| resolved.rest.len() == 1)
                    .map(|resolved| {
                        extra["value_reference"] = json!(true);
                        extra["module_file"] = json!(resolved.file);
                        path.last().cloned().unwrap_or_else(|| name.clone())
                    }),
                None if context.repo_glob.get() && !RUST_STD_TYPES.contains(&name.as_str()) => {
                    extra["value_reference"] = json!(true);
                    Some(name.clone())
                }
                None => None,
            }
        };
        if let Some(target) = target {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::References,
                source: source_qualified.to_string(),
                target,
                file_path: context.file_path.clone(),
                line,
                extra,
            });
        }
    }
}

fn rust_should_skip_value_reference(name: &str) -> bool {
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

fn rust_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = rust_call_signature(node, source).unwrap_or_else(|| call_name.to_string());
    let (relationship_role, bridge_kind) = rust_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match rust_first_string_arg(node, source) {
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
            "source_language": "rust",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn rust_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "arguments")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn rust_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "std::process::Command::new" | "Command::new" => Some(("invokes_binary", "subprocess")),
        "std::fs::read"
        | "std::fs::read_to_string"
        | "std::fs::File::open"
        | "fs::read"
        | "fs::read_to_string"
        | "File::open" => Some(("reads_file", "file_io")),
        "std::fs::write" | "std::fs::File::create" | "fs::write" | "File::create" => {
            Some(("writes_file", "file_io"))
        }
        "libloading::Library::new" | "Library::new" => Some(("loads_shared_library", "ffi")),
        _ => None,
    }
}

fn rust_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string_literal" | "raw_string_literal") {
            return Some(decode_rust_string_literal(child, source));
        }
        return None;
    }
    None
}

fn decode_rust_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_content" | "string_fragment") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('`')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::new_rust_parser;
    use super::parse_rust_with_parser;

    #[test]
    fn test_parse_rust_with_parser_emits_tested_by_for_cfg_test() {
        let source = br#"

fn production() {}

#[test]
fn test_production() {
    production();
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (nodes, edges) = parse_rust_with_parser("src/lib.rs", source, Some(&mut parser), None);

        assert!(
            nodes
                .iter()
                .any(|node| node.kind == "Test" && node.name == "test_production" && node.is_test)
        );
        assert!(edges.iter().any(|edge| {
            edge.kind == "TESTED_BY"
                && edge.source == "src/lib.rs::production"
                && edge.target == "src/lib.rs::test_production"
        }));
    }

    #[test]
    fn test_parse_rust_with_parser_marks_attribute_tests_without_name_prefix() {
        let source = br#"

#[test]
fn helpers_have_stable_contracts() {
    assert_eq!(1, 1);
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (nodes, _edges) =
            parse_rust_with_parser("src/tests.rs", source, Some(&mut parser), None);

        assert!(nodes.iter().any(|node| {
            node.kind == "Test" && node.name == "helpers_have_stable_contracts" && node.is_test
        }));
    }

    #[test]
    fn test_parse_rust_with_parser_emits_type_reference_edges() {
        let source = br#"

struct User {
    manager: Option<Box<User>>,
}

struct Repository {}

fn create_user(repo: Repository) -> User {
    User { manager: None }
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (_nodes, edges) = parse_rust_with_parser("src/lib.rs", source, Some(&mut parser), None);

        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "src/lib.rs::create_user"
                && edge.target == "src/lib.rs::Repository"
                && edge.extra["relationship_role"] == "type_reference"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "src/lib.rs::create_user"
                && edge.target == "src/lib.rs::User"
                && edge.extra["evidence_kind"] == "rust_type_identifier"
        }));
    }

    #[test]
    fn test_parse_rust_traits_and_impl_for_attach_methods() {
        let source = br#"
pub trait Repository {
    fn find(&self);
}

pub struct Repo;

impl Repo {
    pub fn new() -> Self { Repo }
}

impl Repository for Repo {
    fn find(&self) {}
}

type UserId = u64;

fn boot() {
    let repo = Repo::new();
    repo.find();
}
"#;
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let (nodes, edges) = parse_rust_with_parser("src/lib.rs", source, Some(&mut parser), None);

        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Repository"
                && node.extra["type_role"] == "trait"
                && node.extra["is_contract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Repo" && node.extra["type_role"] == "struct"
        }));
        assert!(
            !nodes.iter().any(|node| {
                node.kind == "Class" && node.extra["type_role"] == "implementation"
            })
        );
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "new"
                && node.parent_name.as_deref() == Some("Repo")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "find"
                && node.parent_name.as_deref() == Some("Repository")
                && node.extra["is_abstract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "find"
                && node.parent_name.as_deref() == Some("Repo")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Type" && node.name == "UserId" && node.extra["type_role"] == "alias"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPLEMENTS"
                && edge.source == "src/lib.rs::Repo"
                && edge.target == "Repository"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/lib.rs::boot"
                && edge.target == "src/lib.rs::Repo.new"
        }));
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CALLS"
                    && edge.source == "src/lib.rs::boot"
                    && edge.target == "src/lib.rs::Repo.find"
            }),
            "{edges:?}"
        );
        assert!(!edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/lib.rs::boot"
                && edge.target == "src/lib.rs::Repository.find"
        }));
        assert!(!edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "src/lib.rs::boot" && edge.target == "Repo::new"
        }));
    }
}
