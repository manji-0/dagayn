use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::json;

use super::member_calls::{CallOrigin, MemberCallBindings};
use super::stdlib::c::{c_function_header, c_header_package, c_header_satisfied_by};
use super::stdlib::cpp::{is_cpp_std_function, is_cpp_std_header, is_cpp_std_type};
use super::stdlib::objc::{objc_framework, objc_prefix_framework, objc_umbrella_members};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    is_test_file, line_count, node_text, resolve_import_path, strip_matching_quotes,
};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_c_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "c", parser, repo_root)
}

pub(super) fn parse_cpp_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "cpp", parser, repo_root)
}

pub(super) fn parse_objc_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "objc", parser, repo_root)
}

fn parse_c_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
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
        language: language.to_string(),
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
        let mut class_paths = HashSet::new();
        if language == "cpp" {
            c_collect_class_paths(tree.root_node(), source, None, &mut class_paths);
        }
        let stdlib = CStdlibScope::collect(tree.root_node(), source, language);
        let mut class_names = class_paths.clone();
        if language == "objc" {
            c_collect_objc_classes(tree.root_node(), source, &mut class_names);
        }
        let mut fields = HashMap::new();
        if language == "cpp" {
            c_collect_fields(tree.root_node(), source, None, &class_paths, &mut fields);
        }
        let context = CParseContext {
            source,
            file_path: file_path.clone(),
            language,
            repo_root,
            class_paths,
            stdlib,
            bindings: RefCell::new(MemberCallBindings::with_types(class_names.clone())),
            class_names,
            fields,
        };
        c_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        if language != "objc"
            && let Some(module) = record_foreign_registrations(tree.root_node(), source, &mut nodes)
        {
            nodes[0].extra["python_module"] = json!(module);
        }
        let mut edges = resolve_c_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct CParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    language: &'a str,
    repo_root: Option<&'a Path>,
    /// Dotted paths of C++ classes defined in this file (`Outer.Inner`).
    class_paths: HashSet<String>,
    stdlib: CStdlibScope,
    /// Variables of the function being walked and the class they hold
    /// (`Repo* r`, `auto r = new Repo()`, `Repo *r = [[Repo alloc] init]`)
    /// or the call they were returned by (`auto s = makeStore();`).
    bindings: RefCell<MemberCallBindings>,
    /// The classes of this file: C++ class paths and Objective-C classes.
    class_names: HashSet<String>,
    /// C++ fields by class path, with the class each holds
    /// (`Svc` -> `repo_` -> `Repo` for `class Svc { Repo* repo_; };`), bound
    /// in every method of the class, also out-of-line ones.
    fields: HashMap<String, HashMap<String, String>>,
}

impl CParseContext<'_> {
    /// Member calls exist in C++ and Objective-C; a C `s->fn()` calls a
    /// function pointer, which no receiver type names.
    fn tracks_receivers(&self) -> bool {
        self.language != "c"
    }
}

fn c_collect_class_paths(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    parent: Option<&str>,
    paths: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => continue,
            "struct_specifier" | "class_specifier" | "union_specifier" => {
                if let Some(name) = c_type_name(child, source) {
                    let path = c_scope_join(parent, &name);
                    c_collect_class_paths(child, source, Some(&path), paths);
                    paths.insert(path);
                    continue;
                }
            }
            _ => {}
        }
        c_collect_class_paths(child, source, parent, paths);
    }
}

fn c_scope_join(parent: Option<&str>, name: &str) -> String {
    match parent {
        Some(parent) => format!("{parent}.{name}"),
        None => name.to_string(),
    }
}

/// Maps an out-of-line scope such as `ns::Outer::Inner` or `V<T>` to the
/// dotted class path used for in-class definitions. The longest suffix that
/// names a class defined in this file wins; otherwise the innermost segment.
fn c_owner_from_scope(scope: &str, class_paths: &HashSet<String>) -> String {
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    let mut chars = scope.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && chars.peek() == Some(&':') => {
                chars.next();
                segments.push(std::mem::take(&mut current));
            }
            _ if depth == 0 && !ch.is_whitespace() => current.push(ch),
            _ => {}
        }
    }
    segments.push(current);
    segments.retain(|segment| !segment.is_empty());
    for start in 0..segments.len() {
        let candidate = segments[start..].join(".");
        if class_paths.contains(&candidate) {
            return candidate;
        }
    }
    segments.last().cloned().unwrap_or_default()
}

fn c_walk_children(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    // The body of a Catch2 `TEST_CASE("...") { }`, already walked with it.
    let mut skip = None;
    for child in node.children(&mut cursor) {
        if skip == Some(child.id()) {
            continue;
        }
        match child.kind() {
            "preproc_include" if enclosing_func.is_none() => {
                if let Some(mut target) = c_include_target(child, context) {
                    let mut extra = json!({});
                    // Unresolved (no repository file of that name) and
                    // written `<...>`: a system header.
                    if c_system_include(child, context.source).as_deref() == Some(target.as_str())
                        && let Some(package) = c_system_header_package(&target)
                    {
                        mark_stdlib_edge(&mut target, &mut extra, package, StdlibEvidence::Certain);
                    }
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra,
                    });
                    continue;
                }
            }
            "module_import" if enclosing_func.is_none() => {
                // Objective-C `@import Foundation;`.
                if let Some(path) = c_direct_child(child, &["identifier", "module_path"])
                    .or_else(|| child.child_by_field_name("path"))
                {
                    let mut target = node_text(path, context.source).trim().to_string();
                    let mut extra = json!({});
                    if let Some(framework) = objc_framework(&target, true) {
                        mark_stdlib_edge(
                            &mut target,
                            &mut extra,
                            framework,
                            StdlibEvidence::Certain,
                        );
                    }
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra,
                    });
                    continue;
                }
            }
            "type_definition" | "struct_specifier" | "class_specifier"
                if enclosing_func.is_none() =>
            {
                if let Some(name) = c_type_name(child, context.source) {
                    let parent = if context.language == "cpp" {
                        enclosing_class
                    } else {
                        None
                    };
                    c_emit_type(child, context, &name, parent, nodes, edges);
                    let path = c_scope_join(parent, &name);
                    c_emit_inheritance(child, context, &path, edges);
                    if context.language == "cpp" {
                        c_walk_children(child, context, Some(&path), enclosing_func, nodes, edges);
                    }
                    continue;
                }
            }
            "class_interface"
            | "class_implementation"
            | "category_interface"
            | "protocol_declaration"
                if context.language == "objc" && enclosing_func.is_none() =>
            {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_type(child, context, &name, None, nodes, edges);
                    if child.kind() == "class_implementation" {
                        c_walk_children(child, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "function_definition" if enclosing_func.is_none() && context.language == "cpp" => {
                if let Some(name) = c_test_macro_name(child, context.source) {
                    c_emit_function(child, context, &name, None, true, nodes, edges);
                    c_walk_function(child, context, None, &name, nodes, edges);
                    continue;
                }
                if let Some((name, scope)) = c_function_name(child, context.source) {
                    let scope = scope.map(|scope| c_owner_from_scope(&scope, &context.class_paths));
                    let owner = scope.as_deref().or(enclosing_class);
                    c_emit_function(child, context, &name, owner, false, nodes, edges);
                    c_walk_function(child, context, owner, &name, nodes, edges);
                    continue;
                }
            }
            "expression_statement" if enclosing_func.is_none() && context.language == "cpp" => {
                if let Some((name, body)) = c_catch2_test_case(child, context.source) {
                    c_emit_test_block(child, body, context, &name, nodes, edges);
                    c_walk_function(body, context, None, &name, nodes, edges);
                    skip = Some(body.id());
                    continue;
                }
            }
            "function_definition" => {
                if let Some((name, scope)) = c_function_name(child, context.source) {
                    // An out-of-line `Widget::draw` belongs to Widget, so it
                    // qualifies the same way an in-class definition would.
                    let scope = scope.map(|scope| c_owner_from_scope(&scope, &context.class_paths));
                    let owner = scope.as_deref().or(enclosing_class);
                    c_emit_function(child, context, &name, owner, false, nodes, edges);
                    c_walk_function(child, context, owner, &name, nodes, edges);
                    continue;
                }
            }
            "method_definition" if context.language == "objc" => {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_function(child, context, &name, enclosing_class, false, nodes, edges);
                    c_walk_function(child, context, enclosing_class, &name, nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "message_expression" if context.language == "objc" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        c_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
        // Bound once the initializer's own calls are walked: `auto s =
        // s.clone();` reads the outer `s`.
        if context.tracks_receivers() && enclosing_func.is_some() {
            match child.kind() {
                "declaration" => c_bind_declaration(child, context),
                "assignment_expression" => c_bind_assignment(child, context),
                _ => {}
            }
        }
    }
}

/// Walks a function body with the variables it declares bound for its
/// member calls: the fields of its class, its parameters, then each local
/// as it is declared. The bindings end with the function.
fn c_walk_function(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    owner: Option<&str>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    if !context.tracks_receivers() {
        c_walk_children(node, context, owner, Some(name), nodes, edges);
        return;
    }
    let snapshot = context.bindings.borrow().snapshot();
    if let Some(fields) = owner.and_then(|owner| context.fields.get(owner)) {
        let mut bindings = context.bindings.borrow_mut();
        for (field, type_name) in fields {
            bindings.bind_any(field.clone(), type_name.clone());
        }
    }
    c_bind_parameters(node, context);
    c_walk_children(node, context, owner, Some(name), nodes, edges);
    context.bindings.borrow_mut().restore(snapshot);
}

fn c_emit_type(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    parent: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, parent);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: parent.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: parent
            .map(|parent| qualify(&context.file_path, parent, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

/// googletest `TEST(Suite, Name)` family, named `Suite.Name` as
/// `--gtest_filter` spells it.
const GTEST_MACROS: &[&str] = &[
    "TEST",
    "TEST_F",
    "TEST_P",
    "TYPED_TEST",
    "TYPED_TEST_P",
    "GTEST_TEST",
];

/// Boost.Test cases, named by their first argument.
const BOOST_TEST_MACROS: &[&str] = &[
    "BOOST_AUTO_TEST_CASE",
    "BOOST_FIXTURE_TEST_CASE",
    "BOOST_DATA_TEST_CASE",
    "BOOST_DATA_TEST_CASE_F",
    "BOOST_AUTO_TEST_CASE_TEMPLATE",
    "BOOST_FIXTURE_TEST_CASE_TEMPLATE",
];

/// Catch2 / doctest cases, named by their string argument.
const CATCH2_TEST_MACROS: &[&str] = &[
    "TEST_CASE",
    "SCENARIO",
    "TEST_CASE_METHOD",
    "TEMPLATE_TEST_CASE",
    "TEMPLATE_PRODUCT_TEST_CASE",
];

/// The test a `TEST(Suite, Name) { }` / `BOOST_AUTO_TEST_CASE(name) { }`
/// definition declares. tree-sitter reads the macro as a function whose
/// parameters are the macro arguments; without this every case in a file
/// was one function named after the macro.
fn c_test_macro_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    if declarator.kind() != "function_declarator" {
        return None;
    }
    let macro_name = node_text(declarator.child_by_field_name("declarator")?, source);
    let parameters = declarator.child_by_field_name("parameters")?;
    let mut cursor = parameters.walk();
    let arguments: Vec<String> = parameters
        .named_children(&mut cursor)
        .map(|parameter| node_text(parameter, source).trim().to_string())
        .collect();
    if GTEST_MACROS.contains(&macro_name.as_str()) {
        match arguments.as_slice() {
            [suite, name, ..] => Some(format!("{suite}.{name}")),
            _ => None,
        }
    } else if BOOST_TEST_MACROS.contains(&macro_name.as_str()) {
        arguments.into_iter().next().filter(|name| !name.is_empty())
    } else {
        None
    }
}

/// `TEST_CASE("name", "[tag]") { ... }`: tree-sitter reads the macro as a
/// call statement followed by a block. Returns the case name and the block.
fn c_catch2_test_case<'tree>(
    node: tree_sitter::Node<'tree>,
    source: &[u8],
) -> Option<(String, tree_sitter::Node<'tree>)> {
    let call = node
        .named_child(0)
        .filter(|call| call.kind() == "call_expression")?;
    let function = call.child_by_field_name("function")?;
    if !CATCH2_TEST_MACROS.contains(&node_text(function, source).as_str()) {
        return None;
    }
    let body = node
        .next_named_sibling()
        .filter(|body| body.kind() == "compound_statement")?;
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let name = arguments
        .named_children(&mut cursor)
        .find(|argument| argument.kind() == "string_literal")
        .map(|literal| strip_matching_quotes(node_text(literal, source).trim()).to_string())?;
    (!name.is_empty()).then_some((name, body))
}

fn c_emit_test_block(
    head: tree_sitter::Node<'_>,
    body: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Test,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: head.start_position().row as i64 + 1,
        line_end: body.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.clone(),
        line: head.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn c_emit_function(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    test_macro: bool,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = test_macro || is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, enclosing_class);
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
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: if test_macro {
            None
        } else {
            c_return_type(node, context.source)
        },
        modifiers: None,
        is_test,
        extra: match c_function_ffi_export(node, context, name, enclosing_class) {
            Some(export) => json!({"ffi_export": export}),
            None => json!({}),
        },
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

/// The C symbol a function definition exports from a shared library.
///
/// A C (or Objective-C) free function has external linkage unless it is
/// `static` or hidden with `__attribute__((visibility("hidden")))`. C++
/// mangles every name except those declared inside `extern "C"`, so only
/// those are reachable by the plain name `dlsym` / `ctypes` look up.
fn c_function_ffi_export(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
) -> Option<serde_json::Value> {
    if node.kind() != "function_definition" || enclosing_class.is_some() {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "storage_class_specifier" if node_text(child, context.source).trim() == "static" => {
                return None;
            }
            "attribute_specifier" | "attribute_declaration"
                if node_text(child, context.source)
                    .replace(' ', "")
                    .contains("visibility(\"hidden\")") =>
            {
                return None;
            }
            _ => {}
        }
    }
    if context.language == "cpp" && !c_has_c_linkage(node, context.source) {
        return None;
    }
    Some(json!({"abi": "c", "kind": "function", "name": name}))
}

/// Functions and classes C / C++ code registers with another runtime,
/// recorded on the registered node as `ffi_exports`.
///
/// Node.js addons (`abi: "napi"`, the JavaScript name):
///
/// * `napi_create_function(env, "name", len, Fn, ...)`
/// * `NODE_SET_METHOD(exports, "name", Fn)`, `Nan::SetMethod(target, "name", Fn)`
/// * `DECLARE_NAPI_METHOD("name", Fn)`
/// * `exports.Set("name", Napi::Function::New(env, Fn))`, also with
///   `Napi::String::New(env, "name")` as the key
/// * `napi_property_descriptor` initializers `{ "name", NULL, Fn, ... }`
///
/// Python extension modules (`abi: "python"`, the module attribute):
///
/// * pybind11 / nanobind `m.def("name", &Fn)` and
///   `py::class_<T>(m, "Name")` / `nb::class_<T>(m, "Name")`
/// * CPython `PyMethodDef` entries `{ "name", Fn, METH_..., doc }`
///
/// Returns the Python module the file defines (`PYBIND11_MODULE(name, m)`,
/// `NB_MODULE(name, m)`, `PyInit_name`), if any.
fn record_foreign_registrations(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &mut [ParsedNode],
) -> Option<String> {
    let mut registrations = Vec::new();
    let mut python_module = None;
    c_collect_registrations(root, source, &mut registrations, &mut python_module);
    for registration in registrations {
        let wanted = if registration.kind == "class" {
            crate::core::types::NodeKind::Class
        } else {
            crate::core::types::NodeKind::Function
        };
        let mut matches = nodes
            .iter_mut()
            .filter(|node| node.kind == wanted && node.name == registration.target);
        let (Some(node), None) = (matches.next(), matches.next()) else {
            continue;
        };
        let entry = json!({
            "abi": registration.abi,
            "kind": registration.kind,
            "name": registration.name,
        });
        match node
            .extra
            .get_mut("ffi_exports")
            .and_then(|value| value.as_array_mut())
        {
            Some(list) if !list.contains(&entry) => list.push(entry),
            Some(_) => {}
            None => node.extra["ffi_exports"] = json!([entry]),
        }
    }
    python_module
}

struct Registration {
    abi: &'static str,
    kind: &'static str,
    /// The name the other runtime sees.
    name: String,
    /// The C / C++ function or class registered.
    target: String,
}

impl Registration {
    fn function(abi: &'static str, name: String, target: String) -> Self {
        Self {
            abi,
            kind: "function",
            name,
            target,
        }
    }
}

fn c_collect_registrations(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    found: &mut Vec<Registration>,
    python_module: &mut Option<String>,
) {
    match node.kind() {
        "function_definition" if python_module.is_none() => {
            *python_module = c_python_module(node, source);
        }
        "call_expression" => {
            if let Some((name, function)) = c_addon_registration_call(node, source) {
                found.push(Registration::function("napi", name, function));
            } else if let Some(registration) = c_python_registration_call(node, source) {
                found.push(registration);
            }
        }
        "initializer_list" => {
            let mut cursor = node.walk();
            let items: Vec<_> = node.named_children(&mut cursor).collect();
            if let [name, data, function, ..] = items.as_slice()
                && name.kind() == "string_literal"
                && (matches!(data.kind(), "null" | "nullptr")
                    || matches!(node_text(*data, source).trim(), "0" | "NULL" | "nullptr"))
                && let Some(function) = c_function_reference(*function, source)
            {
                found.push(Registration::function(
                    "napi",
                    c_string_text(*name, source),
                    function,
                ));
            } else if let [name, function, flags, ..] = items.as_slice()
                && name.kind() == "string_literal"
                && node_text(*flags, source).contains("METH_")
                && let Some(function) = c_function_reference(c_uncast(*function), source)
            {
                found.push(Registration::function(
                    "python",
                    c_string_text(*name, source),
                    function,
                ));
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        c_collect_registrations(child, source, found, python_module);
    }
}

/// The value inside a cast: `(PyCFunction)py_add` -> `py_add`.
fn c_uncast(node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    if node.kind() == "cast_expression"
        && let Some(value) = node.child_by_field_name("value")
    {
        return value;
    }
    node
}

/// `PYBIND11_MODULE(name, m) { ... }` / `NB_MODULE(name, m) { ... }` (which
/// parse as a function of that name) or `PyInit_name`.
fn c_python_module(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (name, _) = c_function_name(node, source)?;
    if let Some(module) = name.strip_prefix("PyInit_") {
        return (!module.is_empty()).then(|| module.to_string());
    }
    if !matches!(name.as_str(), "PYBIND11_MODULE" | "NB_MODULE") {
        return None;
    }
    let parameters = c_first_descendant(node, &["parameter_list"])?;
    let mut cursor = parameters.walk();
    let first = parameters.named_children(&mut cursor).next()?;
    let module = node_text(first, source).trim().to_string();
    (!module.is_empty()
        && module
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
    .then_some(module)
}

/// pybind11 / nanobind: `m.def("name", &Fn, ...)` on a module variable, and
/// `py::class_<T>(m, "Name")`.
fn c_python_registration_call(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Registration> {
    let callee = node.child_by_field_name("function")?;
    let callee_text = node_text(callee, source).replace(char::is_whitespace, "");
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let args: Vec<_> = arguments.named_children(&mut cursor).collect();
    if let Some(receiver) = callee_text.strip_suffix(".def")
        && receiver
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        let name = args.first().filter(|arg| arg.kind() == "string_literal")?;
        let function = c_function_reference(*args.get(1)?, source)?;
        return Some(Registration::function(
            "python",
            c_string_text(*name, source),
            function,
        ));
    }
    let class = [
        "py::class_<",
        "pybind11::class_<",
        "nb::class_<",
        "nanobind::class_<",
    ]
    .iter()
    .find_map(|prefix| callee_text.strip_prefix(prefix))?;
    let class = class.split([',', '>']).next()?;
    let class = class.rsplit("::").next().unwrap_or(class).to_string();
    let name = args.get(1).filter(|arg| arg.kind() == "string_literal")?;
    Some(Registration {
        abi: "python",
        kind: "class",
        name: c_string_text(*name, source),
        target: class,
    })
}

fn c_addon_registration_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, String)> {
    let callee = node.child_by_field_name("function")?;
    let callee_text = node_text(callee, source).replace(char::is_whitespace, "");
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let args: Vec<_> = arguments.named_children(&mut cursor).collect();
    let (name_index, function_index) = match callee_text.as_str() {
        "napi_create_function" => (1, 3),
        "NODE_SET_METHOD" | "Nan::SetMethod" => (1, 2),
        "DECLARE_NAPI_METHOD" => (0, 1),
        _ if callee_text.ends_with(".Set") || callee_text.ends_with("->Set") => {
            // `exports.Set(key, Napi::Function::New(env, Fn))`
            let key = args.first()?;
            let name = if key.kind() == "string_literal" {
                c_string_text(*key, source)
            } else {
                c_first_descendant(*key, &["string_literal"])
                    .map(|literal| c_string_text(literal, source))?
            };
            let value = args.get(1)?;
            if value.kind() != "call_expression" {
                return None;
            }
            let value_callee = value.child_by_field_name("function")?;
            if !node_text(value_callee, source)
                .replace(char::is_whitespace, "")
                .ends_with("Function::New")
            {
                return None;
            }
            let value_args = value.child_by_field_name("arguments")?;
            let mut value_cursor = value_args.walk();
            let function = value_args.named_children(&mut value_cursor).nth(1)?;
            return Some((name, c_function_reference(function, source)?));
        }
        _ => return None,
    };
    let name = args.get(name_index)?;
    if name.kind() != "string_literal" {
        return None;
    }
    let function = c_function_reference(*args.get(function_index)?, source)?;
    Some((c_string_text(*name, source), function))
}

/// `Fn`, `&Fn`, or `ns::Fn` as a function name (`Fn`).
fn c_function_reference(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let text = text.trim().trim_start_matches('&').trim();
    let name = text.rsplit("::").next().unwrap_or(text);
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
    .then(|| name.to_string())
}

/// True when *node* sits inside `extern "C" { ... }` or is `extern "C" f()`.
fn c_has_c_linkage(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind() == "linkage_specification"
            && ancestor
                .child_by_field_name("value")
                .is_some_and(|value| c_string_text(value, source) == "C")
        {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

fn c_emit_call(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    // A C++ lambda capture `[this]()` in an Objective-C++ file reads as a
    // message with no selector.
    if let Some(call_name) = c_call_name(node, context.source).filter(|name| !name.is_empty()) {
        let mut target = call_name;
        let mut extra = json!({});
        if let Some((package, symbol, evidence)) = c_stdlib_call(node, context) {
            target = symbol;
            mark_stdlib_edge(&mut target, &mut extra, package, evidence);
        } else if context.tracks_receivers() {
            c_mark_receiver(node, context, enclosing_class, &target, &mut extra);
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Calls,
            source: caller.clone(),
            target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra,
        });
    }
    if let Some(signature) = c_call_signature(node, context.source)
        && let Some(edge) = c_bridge_edge(node, context, &caller, &signature)
    {
        edges.push(edge);
    }
}

fn c_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if context.language != "cpp" {
        return;
    }
    let Some(base_clause) = c_direct_child(node, &["base_class_clause"]) else {
        return;
    };
    let mut cursor = base_clause.walk();
    for base in base_clause.named_children(&mut cursor) {
        // The last segment of `ns::Base<Args>`, read from the tree: the
        // template arguments may themselves contain `::` and line breaks.
        let mut base = base;
        while matches!(base.kind(), "qualified_identifier" | "template_type") {
            match base.child_by_field_name("name") {
                Some(name) => base = name,
                None => break,
            }
        }
        if base.kind() != "type_identifier" {
            continue;
        }
        let target = node_text(base, context.source).trim().to_string();
        if target.is_empty() {
            continue;
        }
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualify(&context.file_path, name, None),
            target,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_specifier",
            }),
        });
    }
}

fn c_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" | "qualified_identifier" => {
            Some(node_text(callee, source).replace(" :: ", "::"))
        }
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "argument_list")
}

/// The included header, as a repo-relative file path when one exists.
///
/// Objective-C `#import` used to keep the whole directive as the target, so
/// `#import "Logger.h"` never matched the header it names. An include is also
/// written relative to the including file or to a compiler search path, so the
/// literal text alone (`util.h`) matches no file in the graph.
fn c_include_target(node: tree_sitter::Node<'_>, context: &CParseContext<'_>) -> Option<String> {
    let target = c_direct_child(node, &["system_lib_string", "string_literal"])?;
    let literal = strip_matching_quotes(
        node_text(target, context.source)
            .trim()
            .trim_matches(['<', '>'].as_ref()),
    )
    .trim()
    .to_string();
    if literal.is_empty() {
        return None;
    }
    Some(c_resolve_include(&literal, &context.file_path, context.repo_root).unwrap_or(literal))
}

/// Resolves an include against the including directory, then its ancestors,
/// standing in for the `-I` search paths the graph cannot know. A public
/// header usually sits in a parallel `include/` tree, so probe that too. A
/// system header such as `<vector>` matches nothing and keeps its literal name.
fn c_resolve_include(
    literal: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    resolve_import_path(literal, file_path, repo_root, &["include"], true)
}

fn c_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    c_direct_child_text(node, source, &["type_identifier"])
}

/// Resolves a `function_definition` name plus the class it was declared under.
///
/// Only free functions name themselves with a plain `identifier`. An in-class
/// member uses `field_identifier`, and an out-of-line definition
/// (`void Widget::draw() {}`) uses `qualified_identifier`, whose scope names
/// the owning class. Matching on `identifier` alone dropped every C++ method.
/// Keywords tree-sitter can mistake for a function name when it misreads a
/// construct it does not know, e.g. `export namespace std { }` under `#if`
/// parses as a function `namespace` returning `export`.
const C_KEYWORDS: &[&str] = &[
    "namespace",
    "inline",
    "export",
    "module",
    "import",
    "extern",
    "template",
    "typename",
    "using",
    "return",
    "if",
    "else",
    "while",
    "for",
    "do",
    "switch",
    "case",
    "sizeof",
    "decltype",
    "static_assert",
    "class",
    "struct",
    "enum",
    "union",
];

fn c_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<(String, Option<String>)> {
    let declarator = node
        .child_by_field_name("declarator")
        .or_else(|| c_first_descendant(node, &["function_declarator"]))?;
    c_declarator_name(declarator, source).filter(|(name, _)| !C_KEYWORDS.contains(&name.as_str()))
}

fn c_declarator_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
        | "operator_name" => {
            let name = node_text(node, source).trim().to_string();
            (!name.is_empty()).then_some((name, None))
        }
        "qualified_identifier" => {
            // `A::B::method` nests as `A :: (B :: method)`; keep the full
            // `A::B` scope so the owner can be matched against class paths.
            let scope = node
                .child_by_field_name("scope")
                .map(|scope| node_text(scope, source).trim().to_string());
            let (name, inner_scope) = c_declarator_name(node.child_by_field_name("name")?, source)?;
            let scope = match (scope, inner_scope) {
                (Some(outer), Some(inner)) => Some(format!("{outer}::{inner}")),
                (outer, inner) => inner.or(outer),
            };
            Some((name, scope))
        }
        "template_function" | "template_method" => {
            c_declarator_name(node.child_by_field_name("name")?, source)
        }
        "function_declarator"
        | "pointer_declarator"
        | "reference_declarator"
        | "parenthesized_declarator"
        | "array_declarator" => {
            let inner = node
                .child_by_field_name("declarator")
                .or_else(|| c_declarator_child(node))?;
            c_declarator_name(inner, source)
        }
        _ => None,
    }
}

/// `reference_declarator` carries no `declarator` field, so fall back to the
/// first child that can hold a name.
fn c_declarator_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    c_direct_child(
        node,
        &[
            "identifier",
            "field_identifier",
            "type_identifier",
            "destructor_name",
            "operator_name",
            "qualified_identifier",
            "template_function",
            "template_method",
            "function_declarator",
            "pointer_declarator",
            "reference_declarator",
            "parenthesized_declarator",
            "array_declarator",
        ],
    )
}

fn c_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        // `Factory::create()` must resolve to `create`, not to the scope.
        "qualified_identifier" | "template_function" => {
            c_declarator_name(callee, source).map(|(name, _)| name)
        }
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_message_selector(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut skipped_receiver = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "[" | "]" | ":") {
            continue;
        }
        if !skipped_receiver {
            skipped_receiver = true;
            continue;
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn c_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "popen" | "execvp" | "execv" | "execl" | "posix_spawn" => {
            ("invokes_binary", "subprocess")
        }
        "fopen" | "open" => ("opens_file", "file_io"),
        "fread" => ("reads_file", "file_io"),
        "fwrite" => ("writes_file", "file_io"),
        "dlopen" | "LoadLibrary" => ("loads_shared_library", "ffi"),
        "std::system" | "boost::process::child" => ("invokes_binary", "subprocess"),
        "std::ifstream" | "std::ofstream" | "std::fstream" => ("opens_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match c_first_string_arg(node, context.source) {
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
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn c_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = c_direct_child(node, &["argument_list"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(c_string_text(child, source));
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn c_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn c_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn c_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    c_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn c_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = c_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn c_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    found: &mut Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            *found = Some(node_text(child, source));
        }
        c_collect_descendant_texts(child, source, kinds, found);
    }
}

fn c_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    c_collect_descendant_texts(node, source, kinds, &mut found);
    found
}

/// Binds each call to a function of this file by name.
///
/// A member call on a receiver of a class of this file (`bound_owner`,
/// dropped here) takes that class's method first: `repo.save()` on `Repo
/// repo` is `Repo.save` even when another class declared `save` before it.
/// A call whose receiver is of unknown type (`receiver_unknown`) or of a
/// class of another file (`receiver_type`) never binds by name.
fn resolve_c_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    let mut symbols = HashMap::<String, String>::new();
    let mut members = HashMap::<(String, String), String>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        if let Some(owner) = &node.parent_name {
            members
                .entry((owner.clone(), node.name.clone()))
                .or_insert_with(|| qualified.clone());
        }
        symbols.entry(node.name.clone()).or_insert(qualified);
    }
    edges
        .into_iter()
        .map(|mut edge| {
            let owner = edge
                .extra
                .as_object_mut()
                .and_then(|extra| extra.remove("bound_owner"))
                .and_then(|owner| owner.as_str().map(str::to_string));
            if edge.kind != "CALLS"
                || edge.target.contains("::")
                || edge.extra["receiver_unknown"] == true
                || edge.extra.get("receiver_type").is_some()
            {
                return edge;
            }
            if let Some(target) = owner
                .and_then(|owner| members.get(&(owner, edge.target.clone())))
                .or_else(|| symbols.get(&edge.target))
            {
                edge.target = target.clone();
            }
            edge
        })
        .collect()
}

/// The standard-library package of a system header: `libc` (`stdio.h`),
/// `posix` (`unistd.h`, `sys/socket.h`), `std` (`vector`, `cstdio`), or an
/// Apple framework (`Foundation/Foundation.h`). Third-party headers
/// (`boost/...`, `gtest/gtest.h`) match none.
fn c_system_header_package(header: &str) -> Option<&'static str> {
    c_header_package(header)
        .or_else(|| is_cpp_std_header(header).then_some("std"))
        .or_else(|| objc_framework(header, false))
}

/// The header an `#include <...>` / `#import <...>` names, without the
/// brackets; `None` for a quoted (repository) include.
fn c_system_include(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let path = c_direct_child(node, &["system_lib_string"])?;
    let header = node_text(path, source)
        .trim()
        .trim_matches(['<', '>'].as_ref())
        .trim()
        .to_string();
    (!header.is_empty()).then_some(header)
}

/// A variable or field declared with a standard-library type
/// (`std::vector<int> xs;`, `NSString *name;`).
#[derive(Clone, PartialEq, Eq)]
struct CStdType {
    package: &'static str,
    /// `std::vector`, `NSString`.
    name: String,
    /// Declared through a pointer, so its members are reached with `->`.
    pointer: bool,
}

/// What a C / C++ / Objective-C file brings in from the standard library,
/// and what it declares itself, read before the walk so each call is marked
/// where it is emitted. A name the file declares (a function, a `#define`,
/// a type, a variable) is the file's, never the library's.
#[derive(Default)]
struct CStdlibScope {
    /// Headers included with angle brackets (`stdio.h`, `vector`).
    system_headers: HashSet<String>,
    /// Frameworks imported, with those an umbrella brings in (`Cocoa`
    /// imports `Foundation`).
    frameworks: HashSet<&'static str>,
    defined: HashSet<String>,
    /// `using namespace std;`
    using_namespace_std: bool,
    /// Names brought in by `using std::sort;`.
    std_usings: HashSet<String>,
    /// Variables and fields by name; `None` when a declaration of that name
    /// has another type, so a member call on it stays unmarked.
    typed_vars: HashMap<String, Option<CStdType>>,
}

impl CStdlibScope {
    fn collect(root: tree_sitter::Node<'_>, source: &[u8], language: &str) -> Self {
        let mut scope = Self::default();
        let mut stack = vec![root];
        let mut declarations = Vec::new();
        while let Some(node) = stack.pop() {
            scope.visit(node, source, &mut declarations);
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
        // Typed after the whole file is read: `using namespace std;` and a
        // class the file defines decide what a bare type name means.
        for declaration in declarations {
            let std_type = declaration
                .child_by_field_name("type")
                .and_then(|ty| scope.std_type(ty, source, language));
            let mut cursor = declaration.walk();
            for declarator in declaration.children_by_field_name("declarator", &mut cursor) {
                let Some((name, pointer)) = c_declared_name(declarator, source) else {
                    continue;
                };
                let entry = std_type.clone().map(|(package, name)| CStdType {
                    package,
                    name,
                    pointer,
                });
                match scope.typed_vars.get_mut(&name) {
                    Some(existing) if *existing != entry => *existing = None,
                    Some(_) => {}
                    None => {
                        scope.typed_vars.insert(name, entry);
                    }
                }
            }
        }
        scope
    }

    fn visit<'tree>(
        &mut self,
        node: tree_sitter::Node<'tree>,
        source: &[u8],
        declarations: &mut Vec<tree_sitter::Node<'tree>>,
    ) {
        match node.kind() {
            "preproc_include" => {
                if let Some(header) = c_system_include(node, source) {
                    if let Some(framework) = objc_framework(&header, false) {
                        self.import_framework(framework);
                    }
                    self.system_headers.insert(header);
                }
            }
            "module_import" => {
                if let Some(framework) = c_direct_child(node, &["identifier", "module_path"])
                    .or_else(|| node.child_by_field_name("path"))
                    .and_then(|path| objc_framework(node_text(path, source).trim(), true))
                {
                    self.import_framework(framework);
                }
            }
            "using_declaration" => {
                if c_direct_child(node, &["namespace"]).is_some() {
                    if c_direct_child_text(node, source, &["identifier"]).as_deref() == Some("std")
                    {
                        self.using_namespace_std = true;
                    }
                } else if let Some(path) = c_direct_child(node, &["qualified_identifier"])
                    .map(|path| c_scoped_path(&node_text(path, source)))
                    && path.len() == 2
                    && path[0] == "std"
                {
                    self.std_usings.insert(path[1].clone());
                }
            }
            "function_definition" => {
                if let Some((name, _)) = c_function_name(node, source) {
                    self.defined.insert(name);
                }
            }
            "declaration" | "field_declaration" | "parameter_declaration" => {
                let mut cursor = node.walk();
                for declarator in node.children_by_field_name("declarator", &mut cursor) {
                    if let Some((name, _)) = c_declared_name(declarator, source) {
                        self.defined.insert(name);
                    }
                }
                declarations.push(node);
            }
            "type_definition" => {
                let mut cursor = node.walk();
                for declarator in node.children_by_field_name("declarator", &mut cursor) {
                    if let Some((name, _)) = c_declared_name(declarator, source) {
                        self.defined.insert(name);
                    }
                }
            }
            "preproc_def" | "preproc_function_def" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.defined
                        .insert(node_text(name, source).trim().to_string());
                }
            }
            "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier" => {
                if let Some(name) = c_type_name(node, source) {
                    self.defined.insert(name);
                }
            }
            "class_interface"
            | "class_implementation"
            | "category_interface"
            | "protocol_declaration" => {
                if let Some(name) = c_direct_child_text(node, source, &["identifier"]) {
                    self.defined.insert(name);
                }
            }
            _ => {}
        }
    }

    fn import_framework(&mut self, framework: &'static str) {
        self.frameworks.insert(framework);
        self.frameworks
            .extend(objc_umbrella_members(framework).iter().copied());
    }

    /// Certain when the framework is imported, likely on the prefix alone.
    fn framework_evidence(&self, framework: &str) -> StdlibEvidence {
        if self.frameworks.contains(framework) {
            StdlibEvidence::Certain
        } else {
            StdlibEvidence::Likely
        }
    }

    /// The standard-library type a declaration's `type` names:
    /// `std::vector<int>` -> `std::vector`, a bare `vector<int>` under
    /// `using namespace std;`, or an Objective-C framework class
    /// (`NSString`).
    fn std_type(
        &self,
        ty: tree_sitter::Node<'_>,
        source: &[u8],
        language: &str,
    ) -> Option<(&'static str, String)> {
        match ty.kind() {
            "qualified_identifier" => {
                let path = c_scoped_path(&node_text(ty, source));
                (path.first().map(String::as_str) == Some("std")).then(|| ("std", path.join("::")))
            }
            "template_type" | "type_identifier" => {
                let name = match ty.child_by_field_name("name") {
                    Some(name) => node_text(name, source),
                    None => node_text(ty, source),
                };
                let name = name.trim();
                if self.defined.contains(name) {
                    return None;
                }
                if is_cpp_std_type(name)
                    && (self.using_namespace_std || self.std_usings.contains(name))
                {
                    return Some(("std", format!("std::{name}")));
                }
                if language == "objc" {
                    return objc_prefix_framework(name)
                        .map(|framework| (framework, name.to_string()));
                }
                None
            }
            _ => None,
        }
    }
}

/// The name a declarator declares and whether it is a pointer:
/// `*p = malloc(1)` -> (`p`, true), `(*cb)(int)` -> (`cb`, true).
fn c_declared_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<(String, bool)> {
    let mut node = node;
    let mut pointer = false;
    loop {
        match node.kind() {
            "init_declarator" => node = node.child_by_field_name("declarator")?,
            "pointer_declarator" => {
                pointer = true;
                node = node
                    .child_by_field_name("declarator")
                    .or_else(|| c_declarator_child(node))?;
            }
            _ => break,
        }
    }
    let (name, _) = c_declarator_name(node, source)?;
    Some((name, pointer))
}

/// The segments of a scoped name, template arguments and whitespace
/// dropped: `std::vector<std::string>` -> [`std`, `vector`], and a leading
/// `::` (the global namespace) ignored.
fn c_scoped_path(text: &str) -> Vec<String> {
    let mut plain = String::new();
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 && !ch.is_whitespace() => plain.push(ch),
            _ => {}
        }
    }
    plain
        .split("::")
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// The standard-library package a call reaches, the symbol as written or
/// resolved, and how sure that is:
///
/// * `std::sort(...)`, `std::chrono::steady_clock::now()` -> `std`, certain;
///   `sort(...)` after `using std::sort;` too.
/// * `malloc(n)` -> `libc`, `fork()` -> `posix`: certain when the header
///   declaring it is included, likely otherwise.
/// * `sort(...)` under `using namespace std;` -> `std`, likely.
/// * `xs.push_back(1)` on a variable declared `std::vector<int> xs;` ->
///   `std`, likely; on a temporary (`std::string(s).size()`) certain.
/// * `NSLog(...)`, `[NSString stringWithFormat:...]`,
///   `[[NSMutableArray alloc] init]` -> `Foundation`, certain when the
///   framework is imported; `[name length]` on `NSString *name` likely.
///
/// A name the file declares itself is never the library's, nor is a member
/// of a receiver whose type is unknown.
fn c_stdlib_call(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
) -> Option<(&'static str, String, StdlibEvidence)> {
    let scope = &context.stdlib;
    let source = context.source;
    if node.kind() == "message_expression" {
        let selector = c_message_selector(node, source)?;
        let receiver = node.child_by_field_name("receiver")?;
        let class = match receiver.kind() {
            "identifier" => node_text(receiver, source).trim().to_string(),
            // `[[NSMutableArray alloc] init]`: a value the class just made.
            "message_expression"
                if matches!(
                    c_message_selector(receiver, source).as_deref(),
                    Some("alloc" | "new")
                ) =>
            {
                let inner = receiver.child_by_field_name("receiver")?;
                (inner.kind() == "identifier")
                    .then(|| node_text(inner, source).trim().to_string())?
            }
            _ => return None,
        };
        if let Some(Some(var)) = scope.typed_vars.get(&class) {
            return (var.package != "std").then(|| {
                (
                    var.package,
                    format!("{}.{selector}", var.name),
                    StdlibEvidence::Likely,
                )
            });
        }
        if scope.defined.contains(&class) {
            return None;
        }
        let framework = objc_prefix_framework(&class)?;
        return Some((
            framework,
            format!("{class}.{selector}"),
            scope.framework_evidence(framework),
        ));
    }
    let callee = node.child_by_field_name("function")?;
    match callee.kind() {
        "identifier" => c_stdlib_bare_call(node_text(callee, source).trim(), context),
        "qualified_identifier" | "template_function" => {
            let path = c_scoped_path(&node_text(callee, source));
            match path.as_slice() {
                [first, ..] if first == "std" && path.len() > 1 => {
                    Some(("std", path.join("::"), StdlibEvidence::Certain))
                }
                // `::printf(...)`, `make_shared<T>(...)`.
                [name] => c_stdlib_bare_call(name, context),
                _ => None,
            }
        }
        "field_expression" => {
            let method = node_text(callee.child_by_field_name("field")?, source);
            let method = method.trim();
            let receiver = callee.child_by_field_name("argument")?;
            let arrow = callee
                .child_by_field_name("operator")
                .is_some_and(|operator| node_text(operator, source).trim() == "->");
            let variable = match receiver.kind() {
                "identifier" => Some(node_text(receiver, source)),
                // `this->items.push_back(x)`
                "field_expression"
                    if receiver
                        .child_by_field_name("argument")
                        .is_some_and(|object| object.kind() == "this") =>
                {
                    receiver
                        .child_by_field_name("field")
                        .map(|field| node_text(field, source))
                }
                // `std::string(s).size()`
                "call_expression" => {
                    let path = c_scoped_path(&node_text(
                        receiver.child_by_field_name("function")?,
                        source,
                    ));
                    return (!arrow
                        && path.len() == 2
                        && path[0] == "std"
                        && is_cpp_std_type(&path[1]))
                    .then(|| {
                        (
                            "std",
                            format!("{}::{method}", path.join("::")),
                            StdlibEvidence::Certain,
                        )
                    });
                }
                _ => None,
            }?;
            let var = scope.typed_vars.get(variable.trim())?.as_ref()?;
            // A smart pointer's `->` reaches the pointee, not the `std` type.
            (var.package == "std" && var.pointer == arrow).then(|| {
                (
                    "std",
                    format!("{}::{method}", var.name),
                    StdlibEvidence::Likely,
                )
            })
        }
        _ => None,
    }
}

/// An unqualified call `name(...)`: a libc / POSIX function, an Apple
/// framework function (`NSLog`, `CGRectMake`), or a `std` function brought
/// in by `using`.
fn c_stdlib_bare_call(
    name: &str,
    context: &CParseContext<'_>,
) -> Option<(&'static str, String, StdlibEvidence)> {
    let scope = &context.stdlib;
    if name.is_empty() || scope.defined.contains(name) {
        return None;
    }
    let cpp = context.language == "cpp";
    if cpp && scope.std_usings.contains(name) {
        return Some(("std", format!("std::{name}"), StdlibEvidence::Certain));
    }
    if let Some((header, package)) = c_function_header(name) {
        let evidence = if scope
            .system_headers
            .iter()
            .any(|included| c_header_satisfied_by(header, included))
        {
            StdlibEvidence::Certain
        } else {
            StdlibEvidence::Likely
        };
        return Some((package, name.to_string(), evidence));
    }
    if let Some(framework) = objc_prefix_framework(name)
        && (context.language == "objc" || scope.frameworks.contains(framework))
    {
        return Some((
            framework,
            name.to_string(),
            scope.framework_evidence(framework),
        ));
    }
    (cpp && scope.using_namespace_std && is_cpp_std_function(name))
        .then(|| ("std", format!("std::{name}"), StdlibEvidence::Likely))
}

/// The declared return type of a function or method, as written, for
/// resolution across files to type what a call returns: `Store*` for
/// `Store* make()`, `const Repo&` for `const Repo& Svc::get()`,
/// `std::unique_ptr<Store>`, `Repo*` for `auto f() -> Repo*`, and
/// `(NSString *)` for an Objective-C `- (NSString *)name`. Constructors and
/// destructors declare none.
fn c_return_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "method_definition" {
        return c_direct_child_text(node, source, &["method_type"])
            .map(|text| text.trim().to_string());
    }
    let ty = node.child_by_field_name("type")?;
    let declarator = node.child_by_field_name("declarator");
    // `auto f() -> Repo*`: the type follows the parameters.
    if ty.kind() == "placeholder_type_specifier"
        && let Some(trailing) = declarator
            .and_then(|declarator| c_first_descendant(declarator, &["trailing_return_type"]))
        && let Some(written) = c_direct_child(trailing, &["type_descriptor"])
    {
        return Some(node_text(written, source).trim().to_string());
    }
    let mut written = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.id() == ty.id() {
            break;
        }
        if child.kind() == "type_qualifier" {
            written.push_str(node_text(child, source).trim());
            written.push(' ');
        }
    }
    written.push_str(node_text(ty, source).trim());
    // `Store* make()` / `const Repo& get()`: the pointer or reference wraps
    // the function declarator.
    let mut declarator = declarator;
    while let Some(current) = declarator {
        match current.kind() {
            "pointer_declarator" => written.push('*'),
            "reference_declarator" => {
                if let Some(operator) = current.child(0) {
                    written.push_str(node_text(operator, source).trim());
                }
            }
            _ => break,
        }
        declarator = current
            .child_by_field_name("declarator")
            .or_else(|| c_declarator_child(current));
    }
    Some(written)
}

/// Names of the Objective-C classes a file declares or implements.
fn c_collect_objc_classes(node: tree_sitter::Node<'_>, source: &[u8], names: &mut HashSet<String>) {
    if matches!(
        node.kind(),
        "class_interface" | "class_implementation" | "category_interface"
    ) && let Some(name) = c_direct_child_text(node, source, &["identifier"])
    {
        names.insert(name.trim().to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        c_collect_objc_classes(child, source, names);
    }
}

/// The class-typed fields of each C++ class of the file (`Svc` -> `repo_`
/// -> `Repo` for `class Svc { Repo* repo_; };`).
fn c_collect_fields(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    parent: Option<&str>,
    class_paths: &HashSet<String>,
    fields: &mut HashMap<String, HashMap<String, String>>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => continue,
            "struct_specifier" | "class_specifier" | "union_specifier" => {
                if let Some(name) = c_type_name(child, source) {
                    let path = c_scope_join(parent, &name);
                    if let Some(body) = child.child_by_field_name("body") {
                        let mut members = body.walk();
                        for member in body.children(&mut members) {
                            if member.kind() != "field_declaration" {
                                continue;
                            }
                            let Some(CDeclType::Class(type_name)) = member
                                .child_by_field_name("type")
                                .map(|ty| c_decl_type(ty, source, class_paths, "cpp"))
                            else {
                                continue;
                            };
                            let mut declarators = member.walk();
                            for declarator in
                                member.children_by_field_name("declarator", &mut declarators)
                            {
                                if declarator.kind() == "function_declarator" {
                                    continue;
                                }
                                if let Some((field, _)) = c_declared_name(declarator, source) {
                                    fields
                                        .entry(path.clone())
                                        .or_default()
                                        .insert(field, type_name.clone());
                                }
                            }
                        }
                    }
                    c_collect_fields(child, source, Some(&path), class_paths, fields);
                    continue;
                }
            }
            _ => {}
        }
        c_collect_fields(child, source, parent, class_paths, fields);
    }
}

/// What a declaration's type says about the members reached through the
/// variable it declares.
enum CDeclType {
    /// A class, as the dotted path of a class of this file (`Outer.Inner`)
    /// or the bare name of another (`Repo` for `ns::Repo*`). A smart
    /// pointer's `->` reaches its pointee, so `std::unique_ptr<Repo>` is
    /// `Repo`.
    Class(String),
    /// `auto` / Objective-C `id`: the type comes from the initializer.
    Infer,
    /// A primitive, library or otherwise unusable type.
    Other,
}

const C_SMART_POINTERS: &[&str] = &["unique_ptr", "shared_ptr", "weak_ptr"];

/// `class_names` are the classes of this file (C++ class paths, Objective-C
/// classes).
fn c_decl_type(
    ty: tree_sitter::Node<'_>,
    source: &[u8],
    class_names: &HashSet<String>,
    language: &str,
) -> CDeclType {
    let class = |written: &str| {
        let path = c_owner_from_scope(written, class_names);
        let local = class_names.contains(&path);
        // Library types are marked by the standard-library pass, never
        // resolved across files.
        if path.is_empty()
            || (!local && language == "cpp" && is_cpp_std_type(&path))
            || (!local && language == "objc" && objc_prefix_framework(&path).is_some())
        {
            CDeclType::Other
        } else {
            CDeclType::Class(path)
        }
    };
    let smart_pointee = |template: tree_sitter::Node<'_>| {
        let arguments = template.child_by_field_name("arguments")?;
        let argument = c_direct_child(arguments, &["type_descriptor"])?;
        Some(c_decl_type(
            argument.child_by_field_name("type")?,
            source,
            class_names,
            language,
        ))
    };
    match ty.kind() {
        "placeholder_type_specifier" => CDeclType::Infer,
        "typedefed_specifier" | "type_identifier"
            if matches!(node_text(ty, source).trim(), "id" | "instancetype") =>
        {
            CDeclType::Infer
        }
        "type_identifier" => class(node_text(ty, source).trim()),
        "struct_specifier" | "class_specifier" | "union_specifier" => {
            c_type_name(ty, source).map_or(CDeclType::Other, |name| class(&name))
        }
        "template_type" => {
            let Some(name) = ty.child_by_field_name("name") else {
                return CDeclType::Other;
            };
            let name = node_text(name, source);
            if C_SMART_POINTERS.contains(&name.trim()) {
                return smart_pointee(ty).unwrap_or(CDeclType::Other);
            }
            class(name.trim())
        }
        "qualified_identifier" => {
            let path = c_scoped_path(&node_text(ty, source));
            if path.first().map(String::as_str) != Some("std") {
                return class(&node_text(ty, source));
            }
            // `std::unique_ptr<Repo>`
            let mut name = ty;
            while name.kind() == "qualified_identifier" {
                match name.child_by_field_name("name") {
                    Some(inner) => name = inner,
                    None => return CDeclType::Other,
                }
            }
            if name.kind() == "template_type"
                && name.child_by_field_name("name").is_some_and(|inner| {
                    C_SMART_POINTERS.contains(&node_text(inner, source).trim())
                })
            {
                return smart_pointee(name).unwrap_or(CDeclType::Other);
            }
            CDeclType::Other
        }
        _ => CDeclType::Other,
    }
}

/// What a value assigned to a variable holds: an instance of a class
/// (`new Repo(1)`, `Repo{1}`, `std::make_unique<Repo>()`, `Repo(1)` for a
/// class of this file, `[[Repo alloc] init]`), or the result of a call
/// (`makeStore()`, `[factory store]`).
enum CValue {
    Class(String),
    Returned(CallOrigin),
}

fn c_value_binding(value: tree_sitter::Node<'_>, context: &CParseContext<'_>) -> Option<CValue> {
    let source = context.source;
    let class = |ty: tree_sitter::Node<'_>| match c_decl_type(
        ty,
        source,
        &context.class_names,
        context.language,
    ) {
        CDeclType::Class(name) => Some(CValue::Class(name)),
        _ => None,
    };
    match value.kind() {
        "parenthesized_expression" => c_value_binding(value.named_child(0)?, context),
        "new_expression" | "compound_literal_expression" => {
            class(value.child_by_field_name("type")?)
        }
        "call_expression" => {
            let mut callee = value.child_by_field_name("function")?;
            while callee.kind() == "qualified_identifier" {
                callee = callee.child_by_field_name("name")?;
            }
            // `std::make_unique<Repo>(...)` / `make_shared<Repo>(...)`
            if callee.kind() == "template_function"
                && callee.child_by_field_name("name").is_some_and(|name| {
                    matches!(
                        node_text(name, source).trim(),
                        "make_unique" | "make_shared"
                    )
                })
            {
                let arguments = callee.child_by_field_name("arguments")?;
                let argument = c_direct_child(arguments, &["type_descriptor"])?;
                return class(argument.child_by_field_name("type")?);
            }
            // `Repo(1)` constructs a class of this file.
            if callee.kind() == "identifier" {
                let name = node_text(callee, source).trim().to_string();
                if context.class_names.contains(&name) {
                    return Some(CValue::Class(name));
                }
            }
            c_call_origin(value, context).map(CValue::Returned)
        }
        "message_expression" => match c_objc_created_class(value, context) {
            Some(name) => Some(CValue::Class(name)),
            None => c_call_origin(value, context).map(CValue::Returned),
        },
        "identifier" => {
            let name = node_text(value, source);
            let bindings = context.bindings.borrow();
            let name = name.trim();
            if let Some(origin) = bindings.returned_by(name) {
                return Some(CValue::Returned(origin.clone()));
            }
            bindings
                .bound_type(name)
                .or_else(|| bindings.foreign_type(name))
                .map(|name| CValue::Class(name.to_string()))
        }
        _ => None,
    }
}

/// `[[Repo alloc] init]` / `[Repo new]`: the class an Objective-C message
/// creates an instance of.
fn c_objc_created_class(
    message: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
) -> Option<String> {
    if message.kind() != "message_expression" {
        return None;
    }
    let selector = c_message_selector(message, context.source)?;
    let receiver = message.child_by_field_name("receiver")?;
    match selector.as_str() {
        "alloc" | "new" if receiver.kind() == "identifier" => {
            let name = node_text(receiver, context.source).trim().to_string();
            c_objc_is_class(&name, context).then_some(name)
        }
        _ if selector.starts_with("init") => c_objc_created_class(receiver, context).filter(|_| {
            matches!(
                c_message_selector(receiver, context.source).as_deref(),
                Some("alloc")
            )
        }),
        _ => None,
    }
}

/// An Objective-C message receiver naming a class rather than a variable:
/// a class of this file, or a capitalized name the file declares no
/// variable of (`[Repo shared]`).
fn c_objc_is_class(name: &str, context: &CParseContext<'_>) -> bool {
    let bindings = context.bindings.borrow();
    if bindings.bound_type(name).is_some()
        || bindings.foreign_type(name).is_some()
        || bindings.returned_by(name).is_some()
    {
        return false;
    }
    context.class_names.contains(name)
        || (name.starts_with(|ch: char| ch.is_ascii_uppercase())
            && !context.stdlib.defined.contains(name))
}

/// Binds `var` by its declared type, or, for `auto` / `id`, by what its
/// initializer holds. Any other type drops what the name held before.
fn c_bind_declared(
    var: String,
    declared: CDeclType,
    value: Option<tree_sitter::Node<'_>>,
    context: &CParseContext<'_>,
) {
    let value = match declared {
        CDeclType::Class(type_name) => Some(CValue::Class(type_name)),
        CDeclType::Infer => value.and_then(|value| c_value_binding(value, context)),
        CDeclType::Other => None,
    };
    let mut bindings = context.bindings.borrow_mut();
    match value {
        Some(CValue::Class(type_name)) => bindings.bind_any(var, type_name),
        Some(CValue::Returned(origin)) => bindings.bind_returned(var, origin),
        None => bindings.forget_foreign(&var),
    }
}

/// The parameters of a C / C++ function (`void run(Repo& repo)`) or an
/// Objective-C method (`- (void)run:(Repo *)repo`).
fn c_bind_parameters(node: tree_sitter::Node<'_>, context: &CParseContext<'_>) {
    let source = context.source;
    if node.kind() == "method_definition" {
        let mut cursor = node.walk();
        for parameter in node.children(&mut cursor) {
            if parameter.kind() != "method_parameter" {
                continue;
            }
            let (Some(name), Some(ty)) = (
                c_direct_child_text(parameter, source, &["identifier"]),
                c_direct_child(parameter, &["method_type"])
                    .and_then(|ty| c_direct_child(ty, &["type_name"]))
                    .and_then(|ty| ty.named_child(0)),
            ) else {
                continue;
            };
            let declared = c_decl_type(ty, source, &context.class_names, context.language);
            c_bind_declared(name.trim().to_string(), declared, None, context);
        }
        return;
    }
    if node.kind() != "function_definition" {
        return;
    }
    let Some(parameters) = node
        .child_by_field_name("declarator")
        .and_then(|declarator| {
            if declarator.kind() == "function_declarator" {
                Some(declarator)
            } else {
                c_first_descendant(declarator, &["function_declarator"])
            }
        })
        .and_then(|declarator| declarator.child_by_field_name("parameters"))
    else {
        return;
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !matches!(
            parameter.kind(),
            "parameter_declaration" | "optional_parameter_declaration"
        ) {
            continue;
        }
        let (Some(ty), Some(declarator)) = (
            parameter.child_by_field_name("type"),
            parameter.child_by_field_name("declarator"),
        ) else {
            continue;
        };
        if let Some((name, _)) = c_declared_name(declarator, source) {
            let declared = c_decl_type(ty, source, &context.class_names, context.language);
            c_bind_declared(name, declared, None, context);
        }
    }
}

/// `Repo repo;`, `Repo* r = make();`, `auto r = new Repo();`, `auto s =
/// makeStore();`, `Repo *r = [[Repo alloc] init];`.
fn c_bind_declaration(node: tree_sitter::Node<'_>, context: &CParseContext<'_>) {
    let Some(ty) = node.child_by_field_name("type") else {
        return;
    };
    let mut cursor = node.walk();
    for declarator in node.children_by_field_name("declarator", &mut cursor) {
        if declarator.kind() == "function_declarator" {
            continue;
        }
        let Some((name, _)) = c_declared_name(declarator, context.source) else {
            continue;
        };
        let value = (declarator.kind() == "init_declarator")
            .then(|| declarator.child_by_field_name("value"))
            .flatten();
        let declared = c_decl_type(ty, context.source, &context.class_names, context.language);
        c_bind_declared(name, declared, value, context);
    }
}

/// `s = makeStore();` / `r = new Repo();` rebinds a variable.
fn c_bind_assignment(node: tree_sitter::Node<'_>, context: &CParseContext<'_>) {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return;
    };
    if left.kind() != "identifier" {
        return;
    }
    let name = node_text(left, context.source).trim().to_string();
    c_bind_declared(name, CDeclType::Infer, Some(right), context);
}

/// What a member call's receiver is.
enum CReceiver {
    /// A class of this file: the call binds to its method.
    Owner(String),
    /// A class of another file, resolved across files.
    Foreign(String),
    /// `super`, or a class of another file messaged directly (`[Repo
    /// shared]`): left as it is.
    Known,
    /// Of unknown type, maybe the result of a call.
    Unknown(Option<CallOrigin>),
}

/// Records what a C++ / Objective-C member call's receiver says:
///
/// * `repo.save()` on `Repo repo` of this file: `bound_owner` (read and
///   dropped by [`resolve_c_call_targets`]), as for `this->save()`,
///   `[self save]` and `[Repo shared]`.
/// * on a class of another file: `receiver_type: "Repo"`.
/// * `factory.create()->save()`, `auto s = makeStore(); s.save()`,
///   `[[factory store] save]`: `receiver_unknown` and `receiver_from` the
///   call it came from; any other receiver of unknown type,
///   `receiver_unknown` alone.
///
/// A plain call `save()` has no receiver.
fn c_mark_receiver(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    method: &str,
    extra: &mut serde_json::Value,
) {
    let receiver = match node.kind() {
        "message_expression" => node.child_by_field_name("receiver"),
        _ => node
            .child_by_field_name("function")
            .filter(|callee| callee.kind() == "field_expression")
            .and_then(|callee| callee.child_by_field_name("argument")),
    };
    let Some(receiver) = receiver else {
        return;
    };
    match c_receiver(receiver, context, enclosing_class, method) {
        CReceiver::Owner(owner) => extra["bound_owner"] = json!(owner),
        CReceiver::Foreign(type_name) => extra["receiver_type"] = json!(type_name),
        CReceiver::Known => {}
        CReceiver::Unknown(origin) => {
            extra["receiver_unknown"] = json!(true);
            if let Some(origin) = origin {
                extra["receiver_from"] = origin.to_json();
            }
        }
    }
}

fn c_receiver(
    receiver: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    method: &str,
) -> CReceiver {
    let source = context.source;
    let class = |name: &str| {
        if context.class_names.contains(name) {
            CReceiver::Owner(name.to_string())
        } else {
            CReceiver::Foreign(name.to_string())
        }
    };
    let own_class = || match enclosing_class {
        Some(class) => CReceiver::Owner(class.to_string()),
        None => CReceiver::Known,
    };
    match receiver.kind() {
        "parenthesized_expression" => match receiver.named_child(0) {
            Some(inner) => c_receiver(inner, context, enclosing_class, method),
            None => CReceiver::Unknown(None),
        },
        "this" | "self" => own_class(),
        "super" => CReceiver::Known,
        "identifier" => {
            let name = node_text(receiver, source);
            let name = name.trim();
            if context.language == "objc" {
                match name {
                    "self" => return own_class(),
                    "super" => return CReceiver::Known,
                    _ => {}
                }
            }
            {
                let bindings = context.bindings.borrow();
                if let Some(bound) = bindings.bound_type(name) {
                    return CReceiver::Owner(bound.to_string());
                }
                if let Some(foreign) = bindings.foreign_type(name) {
                    return CReceiver::Foreign(foreign.to_string());
                }
                if let Some(origin) = bindings.returned_by(name) {
                    return CReceiver::Unknown(Some(origin.clone()));
                }
            }
            // `[Repo shared]` messages the class itself.
            if context.language == "objc" && c_objc_is_class(name, context) {
                return if context.class_names.contains(name) {
                    CReceiver::Owner(name.to_string())
                } else {
                    CReceiver::Known
                };
            }
            CReceiver::Unknown(None)
        }
        // `this->repo_->save()`
        "field_expression"
            if receiver
                .child_by_field_name("argument")
                .is_some_and(|object| object.kind() == "this") =>
        {
            receiver
                .child_by_field_name("field")
                .and_then(|field| {
                    let field = node_text(field, source);
                    context
                        .fields
                        .get(enclosing_class?)?
                        .get(field.trim())
                        .map(|type_name| class(type_name))
                })
                .unwrap_or(CReceiver::Unknown(None))
        }
        "new_expression" => match c_value_binding(receiver, context) {
            Some(CValue::Class(name)) => class(&name),
            _ => CReceiver::Unknown(None),
        },
        "call_expression" | "message_expression" => {
            if let Some(name) = c_objc_created_class(receiver, context) {
                return class(&name);
            }
            let base = c_past_repeats(receiver, method, context.source);
            if base.id() != receiver.id() {
                return c_receiver(base, context, enclosing_class, method);
            }
            CReceiver::Unknown(c_call_origin(receiver, context))
        }
        _ => CReceiver::Unknown(None),
    }
}

/// The receiver of a chain past calls of the same method: a line holds a
/// single edge per target, so in `b.flag(1).flag(2).build()` the edge of
/// `flag` stands for both and its receiver is `b`.
fn c_past_repeats<'tree>(
    mut receiver: tree_sitter::Node<'tree>,
    method: &str,
    source: &[u8],
) -> tree_sitter::Node<'tree> {
    loop {
        let inner = match receiver.kind() {
            "call_expression" => receiver
                .child_by_field_name("function")
                .filter(|callee| callee.kind() == "field_expression")
                .filter(|callee| {
                    callee
                        .child_by_field_name("field")
                        .is_some_and(|field| node_text(field, source).trim() == method)
                })
                .and_then(|callee| callee.child_by_field_name("argument")),
            "message_expression"
                if c_message_selector(receiver, source).as_deref() == Some(method) =>
            {
                receiver.child_by_field_name("receiver")
            }
            _ => None,
        };
        match inner {
            Some(inner) => receiver = inner,
            None => return receiver,
        }
    }
}

/// The call an expression is the result of: `makeStore()`,
/// `factory.create()`, `[factory store]` (the called name as written, the
/// rightmost identifier, and the line of its CALLS edge), or a variable
/// bound to one (`auto s = makeStore();`).
fn c_call_origin(
    expression: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
) -> Option<CallOrigin> {
    match expression.kind() {
        "parenthesized_expression" => c_call_origin(expression.named_child(0)?, context),
        "call_expression" | "message_expression" => Some(CallOrigin {
            name: c_call_name(expression, context.source)?.trim().to_string(),
            line: expression.start_position().row as i64 + 1,
            unwrap: false,
        }),
        "identifier" => context
            .bindings
            .borrow()
            .returned_by(node_text(expression, context.source).trim())
            .cloned(),
        _ => None,
    }
}
