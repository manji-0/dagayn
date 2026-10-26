use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde_json::json;

use super::member_calls::{BindingsSnapshot, CallOrigin, MemberCallBindings};
use super::stdlib::go::{go_default_import_name, is_go_builtin_function, is_go_std_import};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child_text, line_count, line_of, node_text, set_declared_namespaces,
    strip_matching_quotes,
};
use super::{add_tested_by_edges, qualify, resolve_rust_call_targets};

pub(super) fn parse_go_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "go")];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let root = tree.root_node();
        let module = repo_root.and_then(|root| go_module_path(root, &file_path));
        let namespace = go_package_namespace(root, source, &file_path, module.as_ref());
        let mut stdlib = GoStdlibScope::of_file(root, source, module.map(|(path, _)| path));
        stdlib.own_package = namespace
            .as_deref()
            .map(|path| path.strip_suffix("_test").unwrap_or(path).to_string());
        let context = GoContext {
            source,
            file_path: &file_path,
            stdlib: &stdlib,
            struct_fields: go_struct_fields(root, source, &stdlib),
            bindings: RefCell::new(MemberCallBindings::with_types(go_declared_types(
                root, source,
            ))),
            locals: RefCell::new(HashSet::new()),
        };
        go_walk_children(root, &context, None, &mut nodes, &mut edges);
        let test_file = go_is_test_file(&file_path);
        if test_file {
            go_mark_test_code(&mut nodes);
        }
        go_finish_typed_receivers(&nodes, &mut edges);
        go_apply_js_global_exports(root, source, &mut nodes);
        let libraries = go_cgo_libraries(root, source);
        if !libraries.is_empty() {
            nodes[0].extra["cgo_libraries"] = json!(libraries);
        }
        set_declared_namespaces(&mut nodes, namespace.into_iter().collect());
        let mut edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        if test_file {
            add_tested_by_edges(&nodes, &mut edges);
        }
        return (nodes, edges);
    }

    (nodes, edges)
}

/// The import path of the package a file belongs to, its namespace: the
/// module path and the directory below the `go.mod`
/// (`example.com/m/zstd`), with `_test` for an external test package
/// (`package zstd_test`), which is a package of its own. Without a module
/// the directory stands in for the path (the package name at the root). A
/// file the build ignores (`//go:build ignore`, a generator) belongs to no
/// package.
fn go_package_namespace(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    module: Option<&(String, String)>,
) -> Option<String> {
    let mut cursor = root.walk();
    let mut package = None;
    for child in root.children(&mut cursor) {
        match child.kind() {
            "comment" if go_is_build_ignore(&node_text(child, source)) => return None,
            "package_clause" => {
                package = direct_child_text(child, source, &["package_identifier"]);
                break;
            }
            _ => {}
        }
    }
    let package = package?;
    let directory = file_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut path = match module {
        Some((module, module_dir)) => {
            let below = if module_dir.is_empty() {
                Some(directory)
            } else if directory == module_dir {
                Some("")
            } else {
                directory
                    .strip_prefix(module_dir.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
            };
            match below? {
                "" => module.clone(),
                below => format!("{module}/{below}"),
            }
        }
        None if directory.is_empty() => package.clone(),
        None => directory.to_string(),
    };
    if package.ends_with("_test") {
        path.push_str("_test");
    }
    Some(path)
}

/// `//go:build ignore` or `// +build ignore` (a constraint no build meets).
fn go_is_build_ignore(comment: &str) -> bool {
    comment.lines().any(|line| {
        let line = line.trim();
        let constraint = line
            .strip_prefix("//go:build")
            .or_else(|| line.strip_prefix("// +build"))
            .or_else(|| line.strip_prefix("//+build"));
        constraint.is_some_and(|constraint| {
            constraint
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '!'))
                .any(|word| word == "ignore")
        })
    })
}

/// A file `go test` compiles only for testing: `name_test.go`, in the
/// package itself or in its external test package (`package name_test`).
fn go_is_test_file(file_path: &str) -> bool {
    file_path
        .rsplit('/')
        .next()
        .is_some_and(|name| name.ends_with("_test.go"))
}

/// A function `go test` runs: `TestX`, `BenchmarkX`, `FuzzX`, or `ExampleX`
/// at the top level of a test file, where the name is the prefix alone or
/// goes on with anything but a lower-case letter (`Testify` is not a test).
fn go_is_test_function_name(name: &str) -> bool {
    ["Test", "Benchmark", "Fuzz", "Example"]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.chars().next().is_some_and(char::is_lowercase))
        })
}

/// Everything a test file declares is test code; its top-level test
/// functions are `Test` nodes.
fn go_mark_test_code(nodes: &mut [ParsedNode]) {
    for node in nodes {
        node.is_test = true;
        if node.kind == crate::core::types::NodeKind::Function
            && node.parent_name.is_none()
            && go_is_test_function_name(&node.name)
        {
            node.kind = crate::core::types::NodeKind::Test;
        }
    }
}

/// What the walk of one file carries: the file, what it says about the
/// standard library, the fields of its structs, and the variables in scope
/// (their types, or the call they hold the result of).
struct GoContext<'a> {
    source: &'a [u8],
    file_path: &'a FilePath,
    stdlib: &'a GoStdlibScope,
    /// `struct` name -> field -> the field's type, if it names one
    /// (`cfg *Config`).
    struct_fields: HashMap<String, HashMap<String, Option<GoType>>>,
    bindings: RefCell<MemberCallBindings>,
    /// Names the enclosing function declares (parameters, receiver,
    /// `var`, `:=`, `range`): a receiver of these is a variable, never a
    /// package qualifier.
    locals: RefCell<HashSet<String>>,
}

fn go_walk_children(
    node: tree_sitter::Node<'_>,
    context: &GoContext<'_>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                go_emit_imports(child, source, file_path, context.stdlib, edges);
            }
            "type_declaration" => {
                // A type declared in a function body is local to it.
                go_emit_types(child, context, enclosing_func, nodes, edges);
            }
            "function_declaration" | "method_declaration" => {
                // `func _()` is the blank identifier: nothing can refer to it.
                if let Some((name, receiver)) =
                    go_function_name_and_receiver(child, source).filter(|(name, _)| name != "_")
                {
                    go_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        receiver.as_deref(),
                        nodes,
                        edges,
                    );
                    let scope = match receiver.as_deref() {
                        Some(receiver) => format!("{receiver}.{name}"),
                        None => name.clone(),
                    };
                    let saved = go_enter_function(child, context, false);
                    go_walk_children(child, context, Some(&scope), nodes, edges);
                    go_leave_function(context, saved);
                    continue;
                }
            }
            "func_literal" => {
                let saved = go_enter_function(child, context, true);
                go_walk_children(child, context, enclosing_func, nodes, edges);
                go_leave_function(context, saved);
                continue;
            }
            "short_var_declaration" | "assignment_statement" | "var_spec" => {
                go_walk_children(child, context, enclosing_func, nodes, edges);
                go_bind_declaration(child, context, enclosing_func.is_some());
                continue;
            }
            "range_clause" => {
                go_walk_children(child, context, enclosing_func, nodes, edges);
                go_bind_range(child, context);
                continue;
            }
            "call_expression" => {
                go_emit_call(child, context, enclosing_func, edges);
            }
            _ => {}
        }
        go_walk_children(child, context, enclosing_func, nodes, edges);
    }
}

fn go_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    stdlib: &GoStdlibScope,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_emit_imports(child, source, file_path, stdlib, edges);
        if child.kind() == "interpreted_string_literal" {
            let mut target = strip_matching_quotes(node_text(child, source).trim()).to_string();
            if !target.is_empty() {
                let mut extra = json!({});
                if stdlib.is_std_import(&target) {
                    let package = target.clone();
                    mark_stdlib_edge(&mut target, &mut extra, &package, StdlibEvidence::Certain);
                }
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.clone(),
                    line: child.start_position().row as i64 + 1,
                    extra,
                });
            }
        }
    }
}

fn go_emit_types(
    node: tree_sitter::Node<'_>,
    context: &GoContext<'_>,
    parent: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "type_spec" {
            continue;
        }
        let Some(name) =
            direct_child_text(child, source, &["type_identifier"]).filter(|name| name != "_")
        else {
            continue;
        };
        let qualified = qualify(file_path, &name, parent);
        let extra = go_type_extra(child, source, context.stdlib);
        nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Class,
            name,
            file_path: file_path.clone(),
            line_start: child.start_position().row as i64 + 1,
            line_end: child.end_position().row as i64 + 1,
            language: "go".to_string(),
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
                .map(|parent| qualify(file_path, parent, None))
                .unwrap_or_else(|| file_path.to_string()),
            qualified,
            file_path.clone(),
            line_of(child),
        ));
    }
}

fn go_type_extra(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    stdlib: &GoStdlibScope,
) -> serde_json::Value {
    let type_role = go_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "interface" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
        if type_role == "struct" {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
            if let Some(body) = node
                .child_by_field_name("type")
                .filter(|ty| ty.kind() == "struct_type")
            {
                let (fields, embedded) = go_struct_declaration(body, source, stdlib);
                map.insert("field_types".to_string(), json!(fields));
                if !embedded.is_empty() {
                    map.insert("embedded".to_string(), json!(embedded));
                }
            }
        }
    }
    extra
}

/// A struct's named fields and their types as resolution reads them
/// (`field_types`: `[]hcode` for `[]*hcode`, `[4]hcode`, or `map[K]hcode`;
/// another package's type by its import path, `example.com/m/huff0.Scratch`
/// for `*huff0.Scratch`; empty for a type with no methods to find), and the
/// types it embeds (`embedded`: `fastGen` for `fastGen` or `*fastGen`).
fn go_struct_declaration(
    body: tree_sitter::Node<'_>,
    source: &[u8],
    stdlib: &GoStdlibScope,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut fields = BTreeMap::new();
    let mut embedded = Vec::new();
    let mut declarations = Vec::new();
    go_field_declarations(body, &mut declarations);
    for declaration in declarations {
        let Some(ty) = declaration.child_by_field_name("type") else {
            continue;
        };
        let mut cursor = declaration.walk();
        let names = declaration
            .children_by_field_name("name", &mut cursor)
            .map(|name| node_text(name, source))
            .collect::<Vec<_>>();
        if names.is_empty() {
            let name = go_type_text(ty, source, stdlib);
            if !name.is_empty() && !name.starts_with("[]") {
                // An embedded field is named by its type (`e.fastGen`).
                let field = name.rsplit(['.', '/']).next().unwrap_or(&name).to_string();
                fields.insert(field, name.clone());
                embedded.push(name);
            }
        }
        for name in names {
            fields.insert(name, go_type_text(ty, source, stdlib));
        }
    }
    (fields, embedded)
}

fn go_field_declarations<'tree>(
    node: tree_sitter::Node<'tree>,
    out: &mut Vec<tree_sitter::Node<'tree>>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "field_declaration" {
            out.push(child);
        } else if child.kind() == "field_declaration_list" {
            go_field_declarations(child, out);
        }
    }
}

/// A type expression as `field_types` spells it (see
/// [`go_struct_declaration`]).
fn go_type_text(ty: tree_sitter::Node<'_>, source: &[u8], stdlib: &GoStdlibScope) -> String {
    let inner = |field: &str| {
        ty.child_by_field_name(field)
            .map(|inner| go_type_text(inner, source, stdlib))
            .unwrap_or_default()
    };
    match ty.kind() {
        "pointer_type" | "parenthesized_type" => {
            let mut cursor = ty.walk();
            ty.named_children(&mut cursor)
                .next()
                .map(|inner| go_type_text(inner, source, stdlib))
                .unwrap_or_default()
        }
        "generic_type" => inner("type"),
        "slice_type" | "array_type" | "map_type" => {
            let element = inner(if ty.kind() == "map_type" {
                "value"
            } else {
                "element"
            });
            if element.is_empty() {
                element
            } else {
                format!("[]{element}")
            }
        }
        "type_identifier" => {
            let name = node_text(ty, source);
            if is_go_predeclared_type(&name) {
                String::new()
            } else {
                name
            }
        }
        "qualified_type" => {
            let (Some(package), Some(name)) = (
                ty.child_by_field_name("package"),
                ty.child_by_field_name("name"),
            ) else {
                return String::new();
            };
            let package = node_text(package, source);
            let path = stdlib
                .imports
                .get(&package)
                .or_else(|| stdlib.packages.get(&package))
                .unwrap_or(&package);
            format!("{path}.{}", node_text(name, source))
        }
        _ => String::new(),
    }
}

fn go_type_role(node: tree_sitter::Node<'_>, _source: &[u8]) -> &'static str {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_type" => return "struct",
            "interface_type" => return "interface",
            _ => {}
        }
    }
    "class"
}

fn go_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    name: &str,
    receiver: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, receiver);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "go".to_string(),
        parent_name: receiver.map(str::to_string),
        params: go_first_parameter_list(node, source),
        // As written, `(*Store, error)` for several results: resolution takes
        // the first when the call was unwrapped (`s, err := Open(p)`).
        return_type: node
            .child_by_field_name("result")
            .map(|result| node_text(result, source)),
        modifiers: None,
        is_test: false,
        extra: go_directive_extra(node, source),
    });
    let container = receiver
        .map(|receiver| qualify(file_path, receiver, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        container,
        qualified,
        file_path.clone(),
        line_of(node),
    ));
}

/// Libraries the cgo preamble links: `-lNAME` in `#cgo ... LDFLAGS:` lines of
/// the comment right above `import "C"`.
fn go_cgo_libraries(root: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut libraries = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "import_declaration" || !node_text(child, source).contains("\"C\"") {
            continue;
        }
        let mut current = child.prev_sibling();
        while let Some(comment) = current.filter(|sibling| sibling.kind() == "comment") {
            for line in node_text(comment, source).lines() {
                let Some((directive, flags)) = line.split_once(':') else {
                    continue;
                };
                let directive = directive.trim().trim_start_matches("//").trim();
                if !(directive.starts_with("#cgo") && directive.ends_with("LDFLAGS")) {
                    continue;
                }
                for flag in flags.split_whitespace() {
                    if let Some(name) = flag.strip_prefix("-l")
                        && !name.is_empty()
                        && !libraries.iter().any(|known| known == name)
                    {
                        libraries.push(name.to_string());
                    }
                }
            }
            current = comment.prev_sibling();
        }
    }
    libraries
}

/// `ffi_export` / `ffi_import` from the directive above a function, if any.
fn go_directive_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> serde_json::Value {
    match go_directive(node, source) {
        Some((key, value)) => json!({ key: value }),
        None => json!({}),
    }
}

/// The FFI directive in the comments right above a function:
///
/// * `//go:wasmexport name` (a WebAssembly export) or `//export name` (cgo,
///   and TinyGo's WebAssembly export): `ffi_export`;
/// * `//go:wasmimport module name`: `ffi_import` of the host function
///   `name` from the import object's `module`.
fn go_directive(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(&'static str, serde_json::Value)> {
    let mut current = node.prev_sibling();
    let mut next_row = node.start_position().row;
    while let Some(comment) = current.filter(|sibling| sibling.kind() == "comment") {
        // Only the comment block attached to the declaration.
        if comment.end_position().row + 1 < next_row {
            break;
        }
        let text = node_text(comment, source);
        let text = text.trim();
        if let Some(name) = text.strip_prefix("//go:wasmexport ") {
            let export = json!({"abi": "wasm", "kind": "function", "name": name.trim()});
            return Some(("ffi_export", export));
        }
        if let Some(name) = text.strip_prefix("//export ") {
            let export = json!({"abi": "c", "kind": "function", "name": name.trim()});
            return Some(("ffi_export", export));
        }
        if let Some(rest) = text.strip_prefix("//go:wasmimport ")
            && let [module, name] = rest.split_whitespace().collect::<Vec<_>>().as_slice()
        {
            let import = json!({"abi": "wasmimport", "module": module, "name": name});
            return Some(("ffi_import", import));
        }
        next_row = comment.start_position().row;
        current = comment.prev_sibling();
    }
    None
}

/// `js.Global().Set("name", js.FuncOf(f))` exposes `f` to JavaScript as the
/// global `name`; a function literal exposes the function that registers it.
/// Recorded as `ffi_exports` entries with `abi: "js_global"`.
fn go_apply_js_global_exports(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &mut [ParsedNode],
) {
    let mut exports: Vec<(String, String)> = Vec::new();
    go_collect_js_global_exports(root, source, None, &mut exports);
    for (function, js_name) in exports {
        let Some(node) = nodes.iter_mut().find(|node| {
            node.kind == crate::core::types::NodeKind::Function
                && match node.parent_name.as_deref() {
                    Some(receiver) => format!("{receiver}.{}", node.name) == function,
                    None => node.name == function,
                }
        }) else {
            continue;
        };
        let entry = json!({"abi": "js_global", "kind": "function", "name": js_name});
        match node
            .extra
            .get_mut("ffi_exports")
            .and_then(|value| value.as_array_mut())
        {
            Some(list) => list.push(entry),
            None => node.extra["ffi_exports"] = json!([entry]),
        }
    }
}

fn go_collect_js_global_exports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    enclosing: Option<&str>,
    out: &mut Vec<(String, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function_declaration" | "method_declaration")
            && let Some((name, receiver)) = go_function_name_and_receiver(child, source)
        {
            let scope = match receiver {
                Some(receiver) => format!("{receiver}.{name}"),
                None => name,
            };
            go_collect_js_global_exports(child, source, Some(&scope), out);
            continue;
        }
        if child.kind() == "call_expression"
            && let Some((_, signature)) = go_call_name_and_signature(child, source)
            && signature.replace(char::is_whitespace, "") == "js.Global().Set"
            && let Some(export) = go_js_global_set(child, source, enclosing)
        {
            out.push(export);
        }
        go_collect_js_global_exports(child, source, enclosing, out);
    }
}

/// `(function, js_name)` for `js.Global().Set("js_name", js.FuncOf(function))`.
fn go_js_global_set(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    enclosing: Option<&str>,
) -> Option<(String, String)> {
    let js_name = go_first_string_arg(node, source)?;
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let value = arguments.named_children(&mut cursor).nth(1)?;
    if value.kind() != "call_expression" {
        return None;
    }
    let (_, signature) = go_call_name_and_signature(value, source)?;
    if signature != "js.FuncOf" {
        return None;
    }
    let inner = value.child_by_field_name("arguments")?;
    let mut inner_cursor = inner.walk();
    let wrapped = inner.named_children(&mut inner_cursor).next()?;
    let function = match wrapped.kind() {
        "identifier" => node_text(wrapped, source),
        "func_literal" => enclosing?.to_string(),
        _ => return None,
    };
    Some((function, js_name))
}

fn go_function_name_and_receiver(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    if node.kind() == "function_declaration" {
        return direct_child_text(node, source, &["identifier"]).map(|name| (name, None));
    }
    let name = direct_child_text(node, source, &["field_identifier"])?;
    let receiver = go_receiver_name(node, source);
    Some((name, receiver))
}

fn go_receiver_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let receiver_list = node
        .children(&mut cursor)
        .find(|child| child.kind() == "parameter_list")?;
    let mut params = receiver_list.walk();
    let declaration = receiver_list
        .named_children(&mut params)
        .find(|child| child.kind() == "parameter_declaration")?;
    let mut ty = declaration.child_by_field_name("type")?;
    loop {
        match ty.kind() {
            "pointer_type" | "parenthesized_type" => {
                let mut inner = ty.walk();
                ty = ty.named_children(&mut inner).next()?;
            }
            "generic_type" => ty = ty.child_by_field_name("type")?,
            "type_identifier" => return Some(node_text(ty, source)),
            _ => return go_last_named_descendant(receiver_list, source, &["type_identifier"]),
        }
    }
}

fn go_first_parameter_list(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() == "parameter_list")
        .map(|child| node_text(child, source))
}

fn go_emit_call(
    node: tree_sitter::Node<'_>,
    context: &GoContext<'_>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (source, file_path) = (context.source, context.file_path);
    let Some((mut call_name, signature)) = go_call_name_and_signature(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, None))
        .unwrap_or_else(|| file_path.to_string());
    // `C.fast_sum(...)` calls C through cgo.
    let mut extra = if signature.starts_with("C.") {
        json!({"receiver": "C"})
    } else {
        json!({})
    };
    if let Some((package, symbol, evidence)) = context.stdlib.package_of_call(&signature) {
        call_name = symbol;
        mark_stdlib_edge(&mut call_name, &mut extra, package, evidence);
    } else if let Some(operand) = node
        .child_by_field_name("function")
        .filter(|function| function.kind() == "selector_expression")
        .and_then(|function| function.child_by_field_name("operand"))
    {
        match go_receiver(operand, &call_name, context) {
            GoReceiver::Typed(GoType::Std(path, type_name)) => {
                // `req.Header.Get` on `req *http.Request`: the standard
                // library's `net/http.Request.Get`.
                call_name = format!("{path}.{type_name}.{call_name}");
                mark_stdlib_edge(&mut call_name, &mut extra, &path, StdlibEvidence::Certain);
            }
            GoReceiver::Typed(GoType::Named(type_name)) => {
                extra[GO_TYPED_RECEIVER_KEY] = json!(type_name);
            }
            GoReceiver::Typed(GoType::Field { owner, path }) => {
                // Never bound by name: only the field's type, which another
                // file declares, says which method this is.
                extra["receiver_unknown"] = json!(true);
                let (field, element) = match path.strip_suffix("[]") {
                    Some(field) => (field, true),
                    None => (path.as_str(), false),
                };
                extra["receiver_field"] = json!({"owner": owner, "field": field});
                if element {
                    extra["receiver_field"]["element"] = json!(true);
                }
            }
            GoReceiver::Typed(GoType::Package(path, type_name)) => {
                // Bound in that package only (`receiver_package`), never to
                // the caller package's type of the name.
                extra["receiver_type"] = json!(type_name);
                extra["receiver_package"] = json!(path);
            }
            GoReceiver::Typed(GoType::Elements(_) | GoType::Channel(_)) => {
                extra["receiver_unknown"] = json!(true);
            }
            GoReceiver::Unknown(origin) => {
                extra["receiver_unknown"] = json!(true);
                if let Some(origin) = origin {
                    extra["receiver_from"] = origin.to_json();
                }
            }
            GoReceiver::Known => {
                // `huff0.Compress()`, `defaults.Load()`: no plain call, so
                // never a function of the caller's file or package.
                let receiver = node_text(operand, source);
                if operand.kind() == "identifier"
                    && let Some(path) = context.stdlib.packages.get(&receiver)
                {
                    extra["import_path"] = json!(path);
                }
                extra["receiver"] = json!(receiver);
            }
        }
    } else if let Some(callee) = node
        .child_by_field_name("function")
        .filter(|function| function.kind() == "identifier")
        && context.locals.borrow().contains(&node_text(callee, source))
    {
        // `f()` on a parameter or variable holding a function.
        extra["callee_local"] = json!(true);
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
    if let Some(edge) = go_bridge_edge(node, source, file_path, &caller, &signature) {
        edges.push(edge);
    }
}

/// Scratch key on a `CALLS` edge: the type its receiver is declared with
/// (`Store` for `s.Save()` after `s := &Store{}`), consumed by
/// [`go_finish_typed_receivers`] once every method of the file is known.
const GO_TYPED_RECEIVER_KEY: &str = "go_typed_receiver";

/// A type a value is declared with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GoType {
    /// A type of the file's package (`Store`, `*Store`, `Store[T]`).
    Named(String),
    /// A type of the standard library: import path, name (`net/http`,
    /// `Request` for `*http.Request`).
    Std(String, String),
    /// A type of another package of the repository or of a module the
    /// file imports: import path, name (`example.com/m/zstd`, `Decoder` for
    /// `*zstd.Decoder`).
    Package(String, String),
    /// A slice, array, or map of the type (`[]T`, `[4]*T`, `map[K]T`),
    /// whose elements an index or a `range` gives.
    Elements(Box<GoType>),
    /// A channel of the type (`chan *T`), whose values a receive (`<-ch`)
    /// or a `range` gives.
    Channel(Box<GoType>),
    /// A field of a struct another file declares, by its path from the
    /// struct (`br` for `s.br` on `s *Scratch`, `state.jobs` for
    /// `e.state.jobs`, `nodes[]` for an element `s.nodes[i]`): resolution
    /// reads the fields' types from the structs' `field_types`.
    Field { owner: String, path: String },
}

impl GoType {
    /// What an index (`x[i]`) or a `range` gives of a value of this type.
    fn element(self) -> Option<GoType> {
        match self {
            GoType::Elements(inner) => Some(*inner),
            GoType::Field { owner, path } if !path.ends_with("[]") => Some(GoType::Field {
                owner,
                path: format!("{path}[]"),
            }),
            _ => None,
        }
    }

    /// How [`MemberCallBindings`] remembers a variable of this type: `Name`,
    /// `path:Name` for the standard library (no Go import path or identifier
    /// has a `:`), `%path Name` for another package's, `[]T` for elements,
    /// `<-T` for a channel, `@Owner.path` for a field of another file's
    /// struct.
    fn key(&self) -> String {
        match self {
            GoType::Named(name) => name.clone(),
            GoType::Std(path, name) => format!("{path}:{name}"),
            GoType::Package(path, name) => format!("%{path} {name}"),
            GoType::Elements(inner) => format!("[]{}", inner.key()),
            GoType::Channel(inner) => format!("<-{}", inner.key()),
            GoType::Field { owner, path } => format!("@{owner}.{path}"),
        }
    }

    fn from_key(key: &str) -> GoType {
        if let Some((path, name)) = key.strip_prefix('%').and_then(|rest| rest.split_once(' ')) {
            return GoType::Package(path.to_string(), name.to_string());
        }
        if let Some(inner) = key.strip_prefix("[]") {
            return GoType::Elements(Box::new(GoType::from_key(inner)));
        }
        if let Some(inner) = key.strip_prefix("<-") {
            return GoType::Channel(Box::new(GoType::from_key(inner)));
        }
        if let Some(field) = key.strip_prefix('@') {
            let (owner, path) = field.split_once('.').unwrap_or((field, ""));
            return GoType::Field {
                owner: owner.to_string(),
                path: path.to_string(),
            };
        }
        match key.split_once(':') {
            Some((path, name)) => GoType::Std(path.to_string(), name.to_string()),
            None => GoType::Named(key.to_string()),
        }
    }
}

/// What a member call's receiver says about the method it calls.
#[derive(Debug)]
enum GoReceiver {
    /// Declared with a type (`s *Store`, `s := Store{}`, `x.(Store)`).
    Typed(GoType),
    /// A variable or value of a type the file does not say, with the call
    /// it is the result of, if any (`NewStore().Save()`,
    /// `s, err := Open(p); s.Save()`).
    Unknown(Option<CallOrigin>),
    /// A package qualifier, a type, or a variable of the package: left to
    /// resolution by name.
    Known,
}

/// Rewrites typed member calls once every method of the file is known:
/// `s.Save()` on a `Store` whose `Save` the file declares becomes
/// `Store::Save` (same-file resolution binds it to `Store.Save`); any
/// other keeps the bare method with `receiver_type: "Store"`, which
/// resolution across files matches (the method may live in another file of
/// the package).
fn go_finish_typed_receivers(nodes: &[ParsedNode], edges: &mut [ParsedEdge]) {
    let methods = nodes
        .iter()
        .filter(|node| node.kind == crate::core::types::NodeKind::Function)
        .filter_map(|node| Some((node.parent_name.clone()?, node.name.clone())))
        .collect::<HashSet<_>>();
    for edge in edges.iter_mut() {
        let Some(type_name) = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove(GO_TYPED_RECEIVER_KEY))
            .and_then(|value| value.as_str().map(str::to_string))
        else {
            continue;
        };
        if methods.contains(&(type_name.clone(), edge.target.clone())) {
            edge.target = format!("{type_name}::{}", edge.target);
        } else {
            edge.extra["receiver_type"] = json!(type_name);
        }
    }
}

/// Every type the file declares (`type Store struct {}`), at any depth.
fn go_declared_types(root: tree_sitter::Node<'_>, source: &[u8]) -> HashSet<String> {
    fn collect(node: tree_sitter::Node<'_>, source: &[u8], out: &mut HashSet<String>) {
        if matches!(node.kind(), "type_spec" | "type_alias")
            && let Some(name) = node.child_by_field_name("name")
        {
            out.insert(node_text(name, source));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect(child, source, out);
        }
    }
    let mut out = HashSet::new();
    collect(root, source, &mut out);
    out
}

/// The fields of the file's top-level structs and their types: `type Store
/// struct { cfg *Config }` gives `Store` -> `cfg` -> `Config`, which types
/// `s.cfg.Get()`. An embedded field is named by its type.
fn go_struct_fields(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    stdlib: &GoStdlibScope,
) -> HashMap<String, HashMap<String, Option<GoType>>> {
    let mut out = HashMap::new();
    let mut cursor = root.walk();
    for declaration in root.children(&mut cursor) {
        if declaration.kind() != "type_declaration" {
            continue;
        }
        let mut specs = declaration.walk();
        for spec in declaration.children(&mut specs) {
            let (Some(name), Some(ty)) = (
                spec.child_by_field_name("name"),
                spec.child_by_field_name("type"),
            ) else {
                continue;
            };
            if spec.kind() != "type_spec" || ty.kind() != "struct_type" {
                continue;
            }
            let mut declarations = Vec::new();
            go_field_declarations(ty, &mut declarations);
            let mut fields = HashMap::new();
            for declaration in declarations {
                let Some(field_type) = declaration.child_by_field_name("type") else {
                    continue;
                };
                let mut names = declaration.walk();
                let names = declaration
                    .children_by_field_name("name", &mut names)
                    .map(|field| node_text(field, source))
                    .collect::<Vec<_>>();
                let ty = go_type_of(field_type, source, stdlib);
                if names.is_empty()
                    && let Some(GoType::Named(name) | GoType::Package(_, name)) = &ty
                {
                    fields.insert(name.clone(), ty.clone());
                }
                for field in names {
                    fields.insert(field, ty.clone());
                }
            }
            out.insert(node_text(name, source), fields);
        }
    }
    out
}

/// The bindings and locals of the enclosing scope, restored by
/// [`go_leave_function`].
struct GoSavedScope {
    bindings: BindingsSnapshot,
    locals: HashSet<String>,
}

/// Enters a function, method, or function literal: binds its receiver and
/// parameters to their types (`func (s *Store) Save(r Repo)`), and records
/// every name it declares as a local. A function literal also sees the
/// variables of the function around it (`nested`).
fn go_enter_function(
    node: tree_sitter::Node<'_>,
    context: &GoContext<'_>,
    nested: bool,
) -> GoSavedScope {
    let saved = GoSavedScope {
        bindings: context.bindings.borrow().snapshot(),
        locals: context.locals.borrow().clone(),
    };
    let mut locals = HashSet::new();
    go_collect_locals(node, context.source, true, &mut locals);
    {
        let mut current = context.locals.borrow_mut();
        if !nested {
            current.clear();
        }
        // A declaration shadows a variable of the same name around it.
        for name in &locals {
            current.insert(name.clone());
            context.bindings.borrow_mut().forget_foreign(name);
        }
    }
    for field in ["receiver", "parameters"] {
        let Some(list) = node.child_by_field_name(field) else {
            continue;
        };
        let mut cursor = list.walk();
        for declaration in list.named_children(&mut cursor) {
            let Some(ty) = declaration.child_by_field_name("type") else {
                continue;
            };
            let mut names = declaration.walk();
            for name in declaration.children_by_field_name("name", &mut names) {
                go_bind_type(&node_text(name, context.source), ty, context);
            }
        }
    }
    saved
}

fn go_leave_function(context: &GoContext<'_>, saved: GoSavedScope) {
    context.bindings.borrow_mut().restore(saved.bindings);
    *context.locals.borrow_mut() = saved.locals;
}

/// The names a function declares: its receiver and parameters, `var`,
/// `:=`, and `range` variables, but not those of a function literal in it
/// (`top` is the function itself).
fn go_collect_locals(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    top: bool,
    out: &mut HashSet<String>,
) {
    match node.kind() {
        "func_literal" if !top => return,
        "parameter_declaration" | "variadic_parameter_declaration" | "var_spec" => {
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                out.insert(node_text(name, source));
            }
        }
        "short_var_declaration" | "range_clause" => {
            if let Some(left) = node.child_by_field_name("left") {
                let mut cursor = left.walk();
                for name in left.named_children(&mut cursor) {
                    if name.kind() == "identifier" {
                        out.insert(node_text(name, source));
                    }
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_collect_locals(child, source, false, out);
    }
}

/// The type a type expression declares a value with: `Store`, `*Store`,
/// `Store[T]` name a type of the package, and so does `models.Store` in the
/// external test package of `models`; `*zstd.Decoder` names one of the
/// imported package, `*http.Request` one of the standard library; `[]T`,
/// `[4]T`, and `map[K]T` the elements' type, `chan T` its values'.
/// Predeclared types (`error`, `string`) and functions name none whose
/// methods resolution could find.
fn go_type_of(ty: tree_sitter::Node<'_>, source: &[u8], stdlib: &GoStdlibScope) -> Option<GoType> {
    match ty.kind() {
        "pointer_type" | "parenthesized_type" => {
            let mut cursor = ty.walk();
            let inner = ty.named_children(&mut cursor).next()?;
            go_type_of(inner, source, stdlib)
        }
        "generic_type" => go_type_of(ty.child_by_field_name("type")?, source, stdlib),
        "slice_type" | "array_type" => Some(GoType::Elements(Box::new(go_type_of(
            ty.child_by_field_name("element")?,
            source,
            stdlib,
        )?))),
        "map_type" => Some(GoType::Elements(Box::new(go_type_of(
            ty.child_by_field_name("value")?,
            source,
            stdlib,
        )?))),
        "channel_type" => Some(GoType::Channel(Box::new(go_type_of(
            ty.child_by_field_name("value")?,
            source,
            stdlib,
        )?))),
        "type_identifier" => {
            let name = node_text(ty, source);
            (!is_go_predeclared_type(&name)).then_some(GoType::Named(name))
        }
        "qualified_type" => {
            let package = node_text(ty.child_by_field_name("package")?, source);
            let name = node_text(ty.child_by_field_name("name")?, source);
            if let Some(path) = stdlib.imports.get(&package) {
                return Some(GoType::Std(path.clone(), name));
            }
            let path = stdlib.packages.get(&package)?;
            Some(if stdlib.own_package.as_ref() == Some(path) {
                GoType::Named(name)
            } else {
                GoType::Package(path.clone(), name)
            })
        }
        _ => None,
    }
}

fn is_go_predeclared_type(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "bool"
            | "byte"
            | "comparable"
            | "complex64"
            | "complex128"
            | "error"
            | "float32"
            | "float64"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "rune"
            | "string"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
    )
}

fn go_bind(var: &str, ty: GoType, context: &GoContext<'_>) {
    context.bindings.borrow_mut().bind_any(var, ty.key());
}

fn go_bind_type(var: &str, ty: tree_sitter::Node<'_>, context: &GoContext<'_>) {
    match go_type_of(ty, context.source, context.stdlib) {
        Some(ty) => go_bind(var, ty, context),
        None => context.bindings.borrow_mut().forget_foreign(var),
    }
}

/// The type a variable is bound to (see [`go_bind`]).
fn go_bound_type(var: &str, context: &GoContext<'_>) -> Option<GoType> {
    let bindings = context.bindings.borrow();
    let bound = bindings
        .bound_type(var)
        .or_else(|| bindings.foreign_type(var))?;
    Some(GoType::from_key(bound))
}

/// Binds the value variable of a `range` over a value of a declared type
/// (`for _, tok := range in` on `in []token`, `for b := range ch` on `ch
/// chan *blockDec`) to the elements' type; every variable it declares drops
/// what it was bound to.
fn go_bind_range(node: tree_sitter::Node<'_>, context: &GoContext<'_>) {
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    let mut cursor = left.walk();
    let names = left
        .named_children(&mut cursor)
        .filter(|name| name.kind() == "identifier")
        .map(|name| node_text(name, context.source))
        .collect::<Vec<_>>();
    for name in &names {
        context.bindings.borrow_mut().forget_foreign(name);
    }
    let ranged = node
        .child_by_field_name("right")
        .and_then(|right| go_value_type(right, context));
    match (names.as_slice(), ranged) {
        ([value], Some(GoType::Channel(inner))) => go_bind(value, *inner, context),
        ([_, value], Some(ranged)) => {
            if let Some(element) = ranged.element() {
                go_bind(value, element, context);
            }
        }
        _ => {}
    }
}

/// Binds the variables a declaration or assignment gives a value:
/// `var s Store`, `s := Store{}` / `&Store{}` / `new(Store)`, `s :=
/// x.(Store)` to the type; `s := NewStore()` and `s, err := Open(p)` to the
/// call (the first of several results is unwrapped); anything else drops
/// what the variable was bound to. A call is recorded only in a function
/// (`in_function`), where the member call on the variable is made from the
/// same function as the call.
fn go_bind_declaration(node: tree_sitter::Node<'_>, context: &GoContext<'_>, in_function: bool) {
    let source = context.source;
    let named = |field: &str| -> Vec<tree_sitter::Node<'_>> {
        match node.child_by_field_name(field) {
            Some(list) if list.kind() == "expression_list" => {
                let mut cursor = list.walk();
                list.named_children(&mut cursor).collect()
            }
            Some(single) => vec![single],
            None => Vec::new(),
        }
    };
    let (names, values) = if node.kind() == "var_spec" {
        let mut cursor = node.walk();
        let names = node
            .children_by_field_name("name", &mut cursor)
            .collect::<Vec<_>>();
        if let Some(ty) = node.child_by_field_name("type") {
            for name in names {
                go_bind_type(&node_text(name, source), ty, context);
            }
            return;
        }
        (names, named("value"))
    } else {
        (named("left"), named("right"))
    };
    for (index, name) in names.iter().enumerate() {
        if name.kind() != "identifier" {
            continue;
        }
        let var = node_text(*name, source);
        let value = if values.len() == names.len() {
            values.get(index).copied()
        } else {
            // `s, err := Open(p)`: the first result of the one call.
            (index == 0 && values.len() == 1).then(|| values[0])
        };
        let several = values.len() == 1 && names.len() > 1;
        // `br := &br[i]`: the value is read before the name is rebound.
        let ty = value.and_then(|value| go_value_type(value, context));
        context.bindings.borrow_mut().forget_foreign(&var);
        let Some(value) = value else {
            continue;
        };
        if let Some(ty) = ty {
            go_bind(&var, ty, context);
        } else if in_function
            && value.kind() == "call_expression"
            && let Some(mut origin) = go_call_origin(value, None, context)
        {
            origin.unwrap |= several;
            context.bindings.borrow_mut().bind_returned(var, origin);
        } else if in_function && value.kind() == "identifier" {
            // Read before binding: the borrow must end before `borrow_mut`.
            let origin = context
                .bindings
                .borrow()
                .returned_by(&node_text(value, source))
                .cloned();
            if let Some(origin) = origin {
                context.bindings.borrow_mut().bind_returned(var, origin);
            }
        }
    }
}

/// The type a value is written with: `Store{}`, `&Store{}`, `new(Store)`,
/// `make([]Store, n)`, `x.(Store)`, a conversion to a type of the file
/// (`Store(x)`), a variable bound to one, or a field, element, or received
/// value of one.
fn go_value_type(value: tree_sitter::Node<'_>, context: &GoContext<'_>) -> Option<GoType> {
    match value.kind() {
        "composite_literal" => go_type_of(
            value.child_by_field_name("type")?,
            context.source,
            context.stdlib,
        ),
        "unary_expression" => {
            let operator = node_text(value.child_by_field_name("operator")?, context.source);
            let operand = go_value_type(value.child_by_field_name("operand")?, context);
            match (operator.as_str(), operand) {
                ("&", operand) => operand,
                // `<-ch` on `ch chan *T`.
                ("<-", Some(GoType::Channel(inner))) => Some(*inner),
                _ => None,
            }
        }
        "parenthesized_expression" => {
            let mut cursor = value.walk();
            let inner = value.named_children(&mut cursor).next()?;
            go_value_type(inner, context)
        }
        "type_assertion_expression" => go_type_of(
            value.child_by_field_name("type")?,
            context.source,
            context.stdlib,
        ),
        // `s.br`: the field's type when the file declares the struct, else
        // the field for resolution to type (see [`GoType::Field`]).
        "selector_expression" => {
            let field = node_text(value.child_by_field_name("field")?, context.source);
            match go_value_type(value.child_by_field_name("operand")?, context)? {
                GoType::Named(owner) => match context
                    .struct_fields
                    .get(&owner)
                    .and_then(|fields| fields.get(&field))
                {
                    Some(declared) => declared.clone(),
                    None => Some(GoType::Field { owner, path: field }),
                },
                GoType::Field { owner, path } => Some(GoType::Field {
                    owner,
                    path: format!("{path}.{field}"),
                }),
                _ => None,
            }
        }
        "index_expression" => {
            go_value_type(value.child_by_field_name("operand")?, context)?.element()
        }
        "slice_expression" => {
            match go_value_type(value.child_by_field_name("operand")?, context)? {
                GoType::Field { path, .. } if path.ends_with("[]") => None,
                ty @ (GoType::Elements(_) | GoType::Field { .. }) => Some(ty),
                _ => None,
            }
        }
        "call_expression" => {
            let function = value.child_by_field_name("function")?;
            let arguments = value.child_by_field_name("arguments")?;
            let name = node_text(function, context.source);
            // `new(T)`, and `make([]T, n)` / `make(chan *T)`, whose type
            // is the result's.
            if function.kind() == "identifier"
                && matches!(name.as_str(), "new" | "make")
                && !context.stdlib.declared.contains(&name)
            {
                let mut cursor = arguments.walk();
                let ty = arguments.named_children(&mut cursor).next()?;
                return go_type_of(ty, context.source, context.stdlib);
            }
            (function.kind() == "identifier"
                && context.bindings.borrow().constructor_type(&name).is_some())
            .then_some(GoType::Named(name))
        }
        "identifier" => go_bound_type(&node_text(value, context.source), context),
        _ => None,
    }
}

/// The call `value` is the result of, or of which a variable holds the
/// result. A chain repeating `method` (`b.X(1).X(2)`) points past the
/// repeats, as the calls of one line to one method are one edge.
fn go_call_origin(
    value: tree_sitter::Node<'_>,
    method: Option<&str>,
    context: &GoContext<'_>,
) -> Option<CallOrigin> {
    match value.kind() {
        "call_expression" => {
            let (name, _) = go_call_name_and_signature(value, context.source)?;
            if method == Some(name.as_str())
                && let Some(receiver) = value
                    .child_by_field_name("function")
                    .filter(|function| function.kind() == "selector_expression")
                    .and_then(|function| function.child_by_field_name("operand"))
            {
                return go_call_origin(receiver, method, context);
            }
            Some(CallOrigin {
                name,
                line: value.start_position().row as i64 + 1,
                unwrap: false,
                element: false,
            })
        }
        "parenthesized_expression" => {
            let mut cursor = value.walk();
            let inner = value.named_children(&mut cursor).next()?;
            go_call_origin(inner, method, context)
        }
        "identifier" => context
            .bindings
            .borrow()
            .returned_by(&node_text(value, context.source))
            .cloned(),
        _ => None,
    }
}

/// Classifies the receiver of a call to `method` (see [`GoReceiver`]). An
/// identifier the function does not declare may be a package
/// (`models.Open()`), a type, or a variable of the package, so it is
/// known; a value of a call or a local variable of no type the file says
/// is unknown.
fn go_receiver(
    operand: tree_sitter::Node<'_>,
    method: &str,
    context: &GoContext<'_>,
) -> GoReceiver {
    let source = context.source;
    if let Some(ty) = go_value_type(operand, context) {
        return GoReceiver::Typed(ty);
    }
    match operand.kind() {
        "identifier" => {
            let name = node_text(operand, source);
            if let Some(origin) = context.bindings.borrow().returned_by(&name) {
                return GoReceiver::Unknown(Some(origin.clone()));
            }
            if context.locals.borrow().contains(&name) {
                GoReceiver::Unknown(None)
            } else {
                GoReceiver::Known
            }
        }
        "call_expression" | "parenthesized_expression" => {
            GoReceiver::Unknown(go_call_origin(operand, Some(method), context))
        }
        "selector_expression" => {
            let Some(object) = operand.child_by_field_name("operand") else {
                return GoReceiver::Known;
            };
            if go_is_local_value(object, context) {
                GoReceiver::Unknown(None)
            } else {
                GoReceiver::Known
            }
        }
        "index_expression" | "type_assertion_expression" | "slice_expression" => {
            if go_is_local_value(operand, context) {
                GoReceiver::Unknown(None)
            } else {
                GoReceiver::Known
            }
        }
        _ => GoReceiver::Known,
    }
}

/// Whether an expression is rooted at a value of the function: a local
/// variable (`s.cfg`, `items[0]`) or a call's result.
fn go_is_local_value(node: tree_sitter::Node<'_>, context: &GoContext<'_>) -> bool {
    match node.kind() {
        "identifier" => context
            .locals
            .borrow()
            .contains(&node_text(node, context.source)),
        "call_expression" => true,
        "selector_expression" => node
            .child_by_field_name("operand")
            .is_some_and(|operand| go_is_local_value(operand, context)),
        _ => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .next()
                .is_some_and(|first| go_is_local_value(first, context))
        }
    }
}

/// What a file says about the standard library: the names its imports of
/// standard packages bind (`fmt`, `http` for `net/http`, `str` for
/// `str "strings"`), and every name it declares, which shadows a
/// predeclared function of the same name (`func min(...)`, `len := 3`).
struct GoStdlibScope {
    imports: HashMap<String, String>,
    /// The names imports of other packages bind (`huff0`, `h` for
    /// `h "example.com/m/huff0"`) -> import path.
    packages: HashMap<String, String>,
    declared: HashSet<String>,
    /// The module this file belongs to (`go.mod`'s `module` line): its
    /// packages are never the standard library, even under a name like
    /// `crypto/...` that has no dot.
    module: Option<String>,
    /// The import path of the package the file is of, or tests from outside
    /// (`package zip_test` is `zip`'s): `zip.Writer` there is `zip`'s own.
    own_package: Option<String>,
}

impl GoStdlibScope {
    fn of_file(root: tree_sitter::Node<'_>, source: &[u8], module: Option<String>) -> Self {
        let mut scope = Self {
            imports: HashMap::new(),
            packages: HashMap::new(),
            declared: HashSet::new(),
            module,
            own_package: None,
        };
        scope.collect(root, source);
        scope
    }

    fn is_std_import(&self, path: &str) -> bool {
        let own = self.module.as_deref().is_some_and(|module| {
            path == module
                || path
                    .strip_prefix(module)
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        !own && is_go_std_import(path)
    }

    fn collect(&mut self, node: tree_sitter::Node<'_>, source: &[u8]) {
        match node.kind() {
            "import_spec" => {
                let path = node
                    .child_by_field_name("path")
                    .map(|path| strip_matching_quotes(node_text(path, source).trim()).to_string());
                // A dot import (`. "fmt"`) binds no name; `_` binds none usable.
                let name = node
                    .child_by_field_name("name")
                    .map(|name| node_text(name, source));
                if let Some(path) = path.as_ref().filter(|path| !self.is_std_import(path)) {
                    match name.as_deref() {
                        None => {
                            let name = go_package_name_guess(path).to_string();
                            self.packages.insert(name, path.clone());
                        }
                        Some("." | "_") => {}
                        Some(name) => {
                            self.packages.insert(name.to_string(), path.clone());
                        }
                    }
                }
                if let Some(path) = path.filter(|path| self.is_std_import(path)) {
                    match name.as_deref() {
                        None => {
                            let name = go_default_import_name(&path).to_string();
                            self.imports.insert(name, path);
                        }
                        Some("." | "_") => {}
                        Some(name) => {
                            self.imports.insert(name.to_string(), path);
                        }
                    }
                }
                return;
            }
            "function_declaration"
            | "var_spec"
            | "const_spec"
            | "parameter_declaration"
            | "variadic_parameter_declaration"
            | "type_spec"
            | "type_alias" => {
                let mut cursor = node.walk();
                for name in node.children_by_field_name("name", &mut cursor) {
                    self.declared.insert(node_text(name, source));
                }
            }
            "short_var_declaration" | "range_clause" => {
                if let Some(left) = node.child_by_field_name("left") {
                    let mut cursor = left.walk();
                    for name in left.named_children(&mut cursor) {
                        if name.kind() == "identifier" {
                            self.declared.insert(node_text(name, source));
                        }
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect(child, source);
        }
    }

    /// `(package, symbol, evidence)` for a call into the standard library:
    /// through an import of it (`fmt.Println`, `str.ToUpper` after
    /// `str "strings"`, `strings.NewReader(s).Read`) certainly, or to a
    /// predeclared function the file does not shadow (`len`) likely. The
    /// symbol is spelled from the import path, without arguments
    /// (`net/http.Get`, `strings.NewReader.Read`).
    fn package_of_call(&self, signature: &str) -> Option<(&str, String, StdlibEvidence)> {
        let signature = go_strip_arguments(signature);
        match signature.split_once('.') {
            Some((head, rest)) => {
                let path = self.imports.get(head)?;
                Some((path, format!("{path}.{rest}"), StdlibEvidence::Certain))
            }
            None => (is_go_builtin_function(&signature) && !self.declared.contains(&signature))
                .then(|| ("builtin", signature.clone(), StdlibEvidence::Likely)),
        }
    }
}

/// The name an import of a package binds when it gives none: the package's
/// name, which by convention is the path's last element without a major
/// version (`v2`), a `gopkg.in` version (`yaml.v3`), or a `go-` prefix or
/// `-go` suffix (`go-isatty`). A package declaring another name is missed.
fn go_package_name_guess(path: &str) -> &str {
    let name = go_default_import_name(path);
    let name = match name.rsplit_once(".v") {
        Some((stem, version))
            if !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()) =>
        {
            stem
        }
        _ => name,
    };
    let name = name.strip_prefix("go-").unwrap_or(name);
    name.strip_suffix("-go").unwrap_or(name)
}

/// A callee without whitespace or the arguments and type arguments of the
/// calls in it: `strings.NewReader(s).Read` is `strings.NewReader.Read`.
fn go_strip_arguments(signature: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::with_capacity(signature.len());
    for c in signature.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            c if depth == 0 && !c.is_whitespace() => out.push(c),
            _ => {}
        }
    }
    out
}

fn go_call_name_and_signature(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, String)> {
    let mut cursor = node.walk();
    let callee = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")?;
    if callee.kind() == "identifier" {
        let name = node_text(callee, source);
        return Some((name.clone(), name));
    }
    if callee.kind() == "selector_expression" {
        let signature = node_text(callee, source);
        let name = go_last_named_descendant(callee, source, &["field_identifier", "identifier"])?;
        return Some((name, signature));
    }
    None
}

fn go_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let line = node.start_position().row as i64 + 1;
    if let Some((relationship_role, target)) = go_wasm_host_bridge(node, source, signature) {
        return Some(ParsedEdge {
            kind: crate::core::types::EdgeKind::CrossArtifact,
            source: caller.to_string(),
            target,
            file_path: file_path.clone(),
            line,
            extra: json!({
                "relationship_role": relationship_role,
                "bridge_kind": "wasm",
                "evidence_kind": "syntax",
                "evidence_source": signature,
                "source_language": "go",
                "target_language": "unknown",
                "confidence": 0.8,
                "confidence_tier": "HIGH",
            }),
        });
    }
    let (relationship_role, bridge_kind) = match signature {
        "exec.Command" => ("invokes_binary", "subprocess"),
        "os.ReadFile" | "os.Open" => ("reads_file", "file_io"),
        "os.WriteFile" => ("writes_file", "file_io"),
        "plugin.Open" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let (target, confidence, confidence_tier) = match go_first_string_arg(node, source) {
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
            "source_language": "go",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

/// A WebAssembly host (wazero, wasmtime-go, wasmer-go): a call with a
/// string argument naming a `.wasm` file (`os.ReadFile("guest.wasm")`)
/// loads the module; `mod.ExportedFunction("add")`,
/// `instance.GetFunc(store, "add")`, `instance.GetExport(store, "add")`, and
/// `instance.Exports.GetFunction("add")` call its export.
fn go_wasm_host_bridge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    signature: &str,
) -> Option<(&'static str, String)> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    let strings: Vec<String> = arguments
        .children(&mut arg_cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "interpreted_string_literal" | "raw_string_literal"
            )
        })
        .map(|child| strip_matching_quotes(node_text(child, source).trim()).to_string())
        .collect();
    if let Some(path) = strings
        .iter()
        .find(|value| value.to_ascii_lowercase().ends_with(".wasm"))
    {
        return Some(("loads_wasm_module", path.clone()));
    }
    let method = signature.rsplit('.').next().unwrap_or(signature);
    matches!(
        method,
        "ExportedFunction" | "GetFunc" | "GetExport" | "GetFunction"
    )
    .then(|| strings.last().cloned())
    .flatten()
    .map(|name| ("calls_wasm_export", name))
}

fn go_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpreted_string_literal" | "raw_string_literal"
        ) {
            return Some(strip_matching_quotes(node_text(child, source).trim()).to_string());
        }
        return None;
    }
    None
}

fn go_last_named_descendant(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(name) = go_last_named_descendant(child, source, kinds) {
            found = Some(name);
        }
    }
    found
}

/// The module path of the `go.mod` nearest above `file_path` within
/// `repo_root` (`module example/app` -> `example/app`), and that `go.mod`'s
/// repository-relative directory (`""` at the root).
fn go_module_path(repo_root: &Path, file_path: &str) -> Option<(String, String)> {
    let mut dir = Path::new(file_path).parent();
    while let Some(current) = dir {
        if let Ok(text) = std::fs::read_to_string(repo_root.join(current).join("go.mod")) {
            let module = text.lines().find_map(|line| {
                let module = line.trim().strip_prefix("module")?;
                module
                    .starts_with(char::is_whitespace)
                    .then(|| strip_matching_quotes(module.trim()).to_string())
            })?;
            return Some((module, current.to_string_lossy().replace('\\', "/")));
        }
        dir = current.parent();
    }
    None
}
