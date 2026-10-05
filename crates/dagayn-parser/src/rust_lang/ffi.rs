//! Cross-language FFI facts for Rust items: exports (PyO3, wasm-bindgen, napi-rs, neon, UniFFI, cxx, WebAssembly components, C symbols), foreign-block imports, and WebAssembly host calls.

use super::*;

/// True for a file using generated WebAssembly component bindings.
pub(super) fn rust_uses_component_bindings(source: &[u8]) -> bool {
    let text = String::from_utf8_lossy(source);
    [
        "wit_bindgen::generate!",
        "component::bindgen!",
        "mod bindings",
        "bindings::exports::",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

/// The JavaScript name napi-rs and neon give a Rust function by default:
/// `fast_sum` -> `fastSum`. Leading and trailing underscores stay; a name
/// that is not plain lowercase `snake_case` is kept as written.
fn js_camel_case(name: &str) -> String {
    let core = name.trim_matches('_');
    if core.is_empty() || core.contains("__") || core.chars().any(|ch| ch.is_ascii_uppercase()) {
        return name.to_string();
    }
    let leading = &name[..name.len() - name.trim_start_matches('_').len()];
    let trailing = &name[name.trim_end_matches('_').len()..];
    let mut out = String::from(leading);
    let mut upper = false;
    for ch in core.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out.push_str(trailing);
    out
}

/// Methods a WebAssembly runtime looks an export up by name with
/// (wasmtime `get_typed_func` / `get_func` / `get_export`, wasmer
/// `get_function` / `get_typed_function`).
const RUST_WASM_EXPORT_LOOKUPS: &[&str] = &[
    "get_typed_func",
    "get_func",
    "get_export",
    "get_function",
    "get_typed_function",
];

/// Whether `source` holds the text `rust_wasm_host_edges` needs to emit an
/// edge: a `.wasm` string (in any case), an export lookup, or, with
/// component bindings, a `call_` method. Every edge of that pass comes from
/// one of these, so a file without them is skipped without walking its tree.
pub(super) fn rust_may_host_wasm(source: &[u8], component: bool) -> bool {
    memchr::memchr_iter(b'.', source).any(|dot| {
        source
            .get(dot + 1..dot + 5)
            .is_some_and(|ext| ext.eq_ignore_ascii_case(b"wasm"))
    }) || RUST_WASM_EXPORT_LOOKUPS
        .iter()
        .any(|lookup| contains_bytes(source, lookup.as_bytes()))
        || (component && contains_bytes(source, b"call_"))
}

pub(super) fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(haystack, needle).is_some()
}

/// A WebAssembly host embedding a module: any call or `include_bytes!` with
/// a string argument naming a `.wasm` file (`Module::from_file(&engine,
/// "guest.wasm")`) emits `loads_wasm_module`, and an export lookup by name
/// (`instance.get_typed_func::<_, _>(&mut store, "add")`) emits
/// `calls_wasm_export`.
pub(super) fn rust_wasm_host_edges(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    component: bool,
    impl_type: Option<&str>,
    func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "impl_item" => {
                let type_name = child
                    .child_by_field_name("type")
                    .map(|ty| node_text(ty, source));
                rust_wasm_host_edges(
                    child,
                    source,
                    file_path,
                    component,
                    type_name.as_deref(),
                    func,
                    edges,
                );
                continue;
            }
            "function_item" => {
                let name = rust_identifier_child(child, source);
                rust_wasm_host_edges(
                    child,
                    source,
                    file_path,
                    component,
                    impl_type,
                    name.as_deref(),
                    edges,
                );
                continue;
            }
            "call_expression" | "macro_invocation" => {
                let caller = func
                    .map(|func| qualify(file_path, func, impl_type))
                    .unwrap_or_else(|| file_path.to_string());
                let mut strings = Vec::new();
                rust_collect_string_literals(child, source, &mut strings);
                let line = child.start_position().row as i64 + 1;
                let field = if child.kind() == "call_expression" {
                    "function"
                } else {
                    "macro"
                };
                let callee = child
                    .child_by_field_name(field)
                    .map(|callee| node_text(callee, source))
                    .unwrap_or_default();
                let method = callee
                    .split("::<")
                    .next()
                    .unwrap_or_default()
                    .rsplit(['.', ':'])
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('!')
                    .to_string();
                let mut interface_hint = None;
                let (role, target) = if let Some(path) = strings
                    .iter()
                    .find(|value| value.to_ascii_lowercase().ends_with(".wasm"))
                {
                    ("loads_wasm_module", path.clone())
                } else if RUST_WASM_EXPORT_LOOKUPS.contains(&method.as_str())
                    && let Some(name) = strings.last()
                {
                    ("calls_wasm_export", name.clone())
                } else if component
                    && child.kind() == "call_expression"
                    && let Some(export) = method.strip_prefix("call_")
                {
                    // wasmtime component bindings: `bindings
                    // .example_calc_ops().call_add(&mut store, ...)` calls
                    // the guest's `add` in interface `example:calc/ops`.
                    interface_hint = callee
                        .rsplit_once('.')
                        .and_then(|(receiver, _)| receiver.strip_suffix("()"))
                        .and_then(|receiver| receiver.rsplit('.').next())
                        .map(str::to_string);
                    ("calls_component_export", export.to_string())
                } else {
                    rust_wasm_host_edges(
                        child, source, file_path, component, impl_type, func, edges,
                    );
                    continue;
                };
                let mut extra = json!({
                    "relationship_role": role,
                    "bridge_kind": "wasm",
                    "evidence_kind": "syntax",
                    "evidence_source": method,
                    "source_language": "rust",
                    "target_language": "unknown",
                    "confidence": 0.8,
                    "confidence_tier": "HIGH",
                });
                if let Some(hint) = interface_hint {
                    extra["interface_hint"] = json!(hint);
                }
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::CrossArtifact,
                    source: caller,
                    target,
                    file_path: file_path.clone(),
                    line,
                    extra,
                });
                continue;
            }
            _ => {}
        }
        rust_wasm_host_edges(child, source, file_path, component, impl_type, func, edges);
    }
}

/// String literals among a call's direct arguments (or a macro's tokens).
fn rust_collect_string_literals(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    found: &mut Vec<String>,
) {
    let arguments = node.child_by_field_name("arguments").or_else(|| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == "token_tree")
    });
    let Some(arguments) = arguments else {
        return;
    };
    let mut cursor = arguments.walk();
    for argument in arguments.children(&mut cursor) {
        if argument.kind() == "string_literal" {
            found.push(node_text(argument, source).trim_matches('"').to_string());
        }
    }
}

/// `uniffi::setup_scaffolding!("ns")`: the namespace the foreign bindings
/// are generated under (the crate name when the macro has no argument).
pub(super) fn rust_uniffi_namespace(root: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = root.walk();
    root.children(&mut cursor)
        .filter(|child| matches!(child.kind(), "macro_invocation" | "expression_statement"))
        .find_map(|child| {
            let text = node_text(child, source);
            let rest = text.trim().strip_prefix("uniffi::setup_scaffolding!")?;
            let literal = rest.split('"').nth(1)?;
            (!literal.is_empty()).then(|| literal.to_string())
        })
}

/// neon's `cx.export_function("name", f)`: records the JavaScript name on
/// the Rust function `f` it registers (`ffi_exports`, `abi: "napi"`).
pub(super) fn record_neon_exported_functions(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    nodes: &mut [ParsedNode],
) {
    if !contains_bytes(source, b"export_function") {
        return;
    }
    let mut registrations = Vec::new();
    collect_neon_registrations(root, source, &mut registrations);
    for (js_name, function) in registrations {
        let mut matches = nodes.iter_mut().filter(|node| {
            node.kind == crate::core::types::NodeKind::Function && node.name == function
        });
        let (Some(node), None) = (matches.next(), matches.next()) else {
            continue;
        };
        let entry = json!({"abi": "napi", "kind": "function", "name": js_name});
        match node
            .extra
            .get_mut("ffi_exports")
            .and_then(|v| v.as_array_mut())
        {
            Some(list) => list.push(entry),
            None => node.extra["ffi_exports"] = json!([entry]),
        }
    }
}

fn collect_neon_registrations(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    found: &mut Vec<(String, String)>,
) {
    if node.kind() == "call_expression"
        && let Some(function) = node.child_by_field_name("function")
        && function.kind() == "field_expression"
        && function
            .child_by_field_name("field")
            .is_some_and(|field| node_text(field, source) == "export_function")
        && let Some(arguments) = node.child_by_field_name("arguments")
    {
        let mut cursor = arguments.walk();
        let args: Vec<_> = arguments.named_children(&mut cursor).collect();
        if let [name, target, ..] = args.as_slice()
            && name.kind() == "string_literal"
            && matches!(target.kind(), "identifier" | "scoped_identifier")
        {
            let js_name = node_text(*name, source).trim_matches('"').to_string();
            let target = node_text(*target, source);
            let function = target.rsplit("::").next().unwrap_or(&target).to_string();
            found.push((js_name, function));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_neon_registrations(child, source, found);
    }
}

/// The `extern "C" { ... }` block directly containing a declaration.
pub(super) fn rust_enclosing_foreign_block(
    node: tree_sitter::Node<'_>,
) -> Option<tree_sitter::Node<'_>> {
    node.parent()
        .filter(|parent| parent.kind() == "declaration_list")
        .and_then(|list| list.parent())
        .filter(|item| item.kind() == "foreign_mod_item")
}

/// What a foreign declaration binds to on the other side.
///
/// `#[wasm_bindgen(module = "/js/util.js")] extern "C" { fn f(); }` imports
/// the JavaScript function `f` (or `js_name`) from that module, a path from
/// the crate root. Methods, constructors, accessors, and namespaced or
/// global imports (`js_namespace = console`) name no module function and
/// are not recorded.
pub(super) fn rust_foreign_ffi_import(
    node: tree_sitter::Node<'_>,
    block: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
) -> Option<serde_json::Value> {
    let block_attrs = rust_leading_attribute_texts(block, source);
    let Some(block_attr) = block_attrs
        .iter()
        .find(|attr| rust_attr_is(attr, "wasm_bindgen"))
    else {
        return rust_c_ffi_import(node, block, &block_attrs, source, name);
    };
    let attrs = rust_leading_attribute_texts(node, source);
    let own_attr = attrs.iter().find(|attr| rust_attr_is(attr, "wasm_bindgen"));
    let module = rust_attr_string_arg(block_attr, "module")?;
    let flags = [
        "method",
        "constructor",
        "getter",
        "setter",
        "structural",
        "indexing_getter",
        "indexing_setter",
        "indexing_deleter",
    ];
    for attr in [Some(block_attr), own_attr].into_iter().flatten() {
        if flags.iter().any(|flag| rust_attr_has_flag(attr, flag))
            || rust_attr_string_arg(attr, "js_namespace").is_some()
            || rust_attr_string_arg(attr, "static_method_of").is_some()
            || attr.contains("js_namespace=")
        {
            return None;
        }
    }
    let js_name = own_attr
        .and_then(|attr| rust_attr_string_arg(attr, "js_name"))
        .unwrap_or_else(|| name.to_string());
    Some(json!({"abi": "wasm", "module": module, "name": js_name}))
}

/// A declaration in a C-ABI `extern` block (`extern "C"`, `"system"`, or no
/// ABI string): the C symbol it links against (`#[link_name]`, else its
/// name), and the library `#[link(name = "...")]` names, if any.
fn rust_c_ffi_import(
    node: tree_sitter::Node<'_>,
    block: tree_sitter::Node<'_>,
    block_attrs: &[String],
    source: &[u8],
    name: &str,
) -> Option<serde_json::Value> {
    if node.kind() != "function_signature_item" {
        return None;
    }
    let abi = rust_extern_abi(block, source);
    if abi.as_deref() == Some("C++") {
        return rust_cxx_import(node, block, source, name);
    }
    if !matches!(
        abi.as_deref(),
        None | Some("C" | "C-unwind" | "system" | "system-unwind" | "cdecl" | "stdcall")
    ) {
        return None;
    }
    let symbol = rust_leading_attribute_texts(node, source)
        .iter()
        .filter(|attr| rust_attr_is(attr, "link_name"))
        .find_map(|attr| rust_attr_string_arg(attr, "link_name"))
        .unwrap_or_else(|| name.to_string());
    let mut import = json!({"abi": "c", "name": symbol});
    if let Some(library) = block_attrs
        .iter()
        .filter(|attr| rust_attr_is(attr, "link"))
        .find_map(|attr| rust_attr_string_arg(attr, "name"))
    {
        import["library"] = json!(library);
    }
    Some(import)
}

/// True when the `extern` block sits in a `#[cxx::bridge] mod`.
fn rust_in_cxx_bridge(block: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    block
        .parent()
        .filter(|parent| parent.kind() == "declaration_list")
        .and_then(|list| list.parent())
        .filter(|item| item.kind() == "mod_item")
        .is_some_and(|module| {
            rust_leading_attribute_texts(module, source)
                .iter()
                .any(|attr| rust_attr_is(attr, "bridge") && attr.contains("cxx::bridge"))
        })
}

/// `unsafe extern "C++" { fn f(); fn m(self: Pin<&mut T>); }` in a
/// `#[cxx::bridge]`: the C++ function `f`, or the method `T::m`
/// (`abi: "cxx"`, with `class` for a method).
fn rust_cxx_import(
    node: tree_sitter::Node<'_>,
    block: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
) -> Option<serde_json::Value> {
    if !rust_in_cxx_bridge(block, source) {
        return None;
    }
    let mut import = json!({"abi": "cxx", "name": name});
    let params = direct_child_text(node, source, &["parameters"]).unwrap_or_default();
    if let Some(receiver) = CXX_RECEIVER_RE.captures(&params) {
        import["class"] = json!(receiver[1].to_string());
    }
    Some(import)
}

static CXX_RECEIVER_RE: LazyLock<Regex> = LazyLock::new(|| {
    // `self: &T`, `self: &mut T`, `self: Pin<&mut T>`
    Regex::new(r"^\(\s*self\s*:\s*(?:Pin\s*<\s*)?&\s*(?:mut\s+)?(\w+)").expect("valid regex")
});

/// A declaration in a cxx `extern "Rust"` block: the Rust function of that
/// name is exported to C++ (`abi: "cxx"`).
pub(super) fn rust_cxx_rust_export(
    block: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
) -> Option<serde_json::Value> {
    (rust_extern_abi(block, source).as_deref() == Some("Rust") && rust_in_cxx_bridge(block, source))
        .then(|| json!({"abi": "cxx", "kind": "function", "name": name}))
}

/// The ABI string of `extern "C" { ... }`; `None` for a bare `extern`.
fn rust_extern_abi(block: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = block.walk();
    let abi = block
        .children(&mut cursor)
        .find(|child| child.kind() == "extern_modifier")?;
    let mut inner = abi.walk();
    let literal = abi
        .children(&mut inner)
        .find(|child| child.kind() == "string_literal")?;
    Some(node_text(literal, source).trim_matches('"').to_string())
}

/// How a function is reachable from another language, if at all.
///
/// * `#[pyfunction]`, or a method in a `#[pymethods]` impl: PyO3 exposes it
///   to Python under its own name or `#[pyo3(name = "...")]`.
/// * `#[no_mangle]` / `#[export_name = "..."]`: exported as a C symbol that
///   `ctypes` / `cffi` / `dlopen` can look up.
/// * `#[wasm_bindgen]`, or a `pub` method in a `#[wasm_bindgen] impl`:
///   wasm-bindgen exposes it to JavaScript under its own name,
///   `js_name = ...`, or `constructor`.
pub(super) fn rust_function_ffi_export(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
) -> Option<serde_json::Value> {
    let attrs = rust_leading_attribute_texts(node, source);
    let renamed = || {
        attrs
            .iter()
            .filter(|attr| rust_attr_is(attr, "pyo3") || rust_attr_is(attr, "pyfunction"))
            .find_map(|attr| rust_attr_string_arg(attr, "name"))
    };
    if attrs.iter().any(|attr| rust_attr_is(attr, "pyfunction")) {
        let python_name = renamed().unwrap_or_else(|| name.to_string());
        return Some(json!({"abi": "pyo3", "kind": "function", "name": python_name}));
    }
    if let Some(export) = rust_node_addon_export(node, source, name, &attrs) {
        return Some(export);
    }
    if let Some(export) = rust_uniffi_export(node, source, name, &attrs) {
        return Some(export);
    }
    let wasm_attr = attrs.iter().find(|attr| rust_attr_is(attr, "wasm_bindgen"));
    let wasm_impl = rust_enclosing_impl(node).filter(|item| {
        rust_leading_attribute_texts(*item, source)
            .iter()
            .any(|attr| rust_attr_is(attr, "wasm_bindgen"))
    });
    if wasm_impl.is_some() {
        // wasm-bindgen exports the `pub` methods of a `#[wasm_bindgen] impl`.
        if !rust_has_pub_visibility(node, source) {
            return None;
        }
        let js_name = if wasm_attr.is_some_and(|attr| attr.contains("constructor")) {
            "constructor".to_string()
        } else {
            wasm_attr
                .and_then(|attr| rust_attr_string_arg(attr, "js_name"))
                .unwrap_or_else(|| name.to_string())
        };
        return Some(json!({"abi": "wasm", "kind": "method", "name": js_name}));
    }
    if let Some(attr) = wasm_attr {
        let js_name = rust_attr_string_arg(attr, "js_name").unwrap_or_else(|| name.to_string());
        return Some(json!({"abi": "wasm", "kind": "function", "name": js_name}));
    }
    let in_pymethods = rust_enclosing_impl(node).is_some_and(|item| {
        rust_leading_attribute_texts(item, source)
            .iter()
            .any(|attr| rust_attr_is(attr, "pymethods"))
    });
    if in_pymethods {
        let python_name = if attrs.iter().any(|attr| rust_attr_is(attr, "new")) {
            "__new__".to_string()
        } else {
            renamed().unwrap_or_else(|| name.to_string())
        };
        return Some(json!({"abi": "pyo3", "kind": "method", "name": python_name}));
    }
    if let Some(symbol) = attrs
        .iter()
        .filter(|attr| rust_attr_is(attr, "export_name"))
        .find_map(|attr| rust_attr_string_arg(attr, "export_name"))
    {
        return Some(json!({"abi": "c", "kind": "function", "name": symbol}));
    }
    if attrs.iter().any(|attr| rust_attr_is(attr, "no_mangle")) {
        return Some(json!({"abi": "c", "kind": "function", "name": name}));
    }
    None
}

/// WebAssembly component bindings, from the trait an impl block implements:
///
/// * `impl exports::example::calc::ops::Guest for C { fn add }` (wit-bindgen
///   guest): the export `add` of interface `example::calc::ops`
///   (`abi: "wit"`); a bare `Guest` is a world-level export (`interface: ""`);
/// * `impl example::calc::logging::Host for S { fn log }` (wasmtime host):
///   the import `log` of interface `example::calc::logging` a guest calls
///   (`abi: "wit_host"`); `impl <World>Imports for S` implements world-level
///   imports.
pub(super) fn rust_component_export(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
) -> Option<serde_json::Value> {
    let item = rust_enclosing_impl(node)?;
    let trait_path = node_text(item.child_by_field_name("trait")?, source);
    let trait_path = trait_path.split('<').next().unwrap_or(&trait_path).trim();
    let mut segments: Vec<&str> = trait_path.split("::").map(str::trim).collect();
    while matches!(segments.first(), Some(&("crate" | "self" | "bindings"))) {
        segments.remove(0);
    }
    let last = segments.pop()?;
    match last {
        "Guest" => {
            let interface = match segments.iter().position(|segment| *segment == "exports") {
                Some(index) => segments[index + 1..].join("::"),
                None if segments.is_empty() => String::new(),
                None => return None,
            };
            Some(json!({"abi": "wit", "kind": "function", "interface": interface, "name": name}))
        }
        "Host" if !segments.is_empty() => Some(json!({
            "abi": "wit_host",
            "kind": "function",
            "interface": segments.join("::"),
            "name": name,
        })),
        imports if segments.is_empty() && imports.ends_with("Imports") => Some(json!({
            "abi": "wit_host",
            "kind": "function",
            "interface": "",
            "name": name,
        })),
        _ => None,
    }
}

/// UniFFI: `#[uniffi::export]` functions, and methods of a
/// `#[uniffi::export] impl` (`abi: "uniffi"`, under the Rust name; each
/// foreign language renames it by its own convention).
fn rust_uniffi_export(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
    attrs: &[String],
) -> Option<serde_json::Value> {
    let is_export = |attr: &String| attr.starts_with("#[uniffi::export");
    let in_export_impl = rust_enclosing_impl(node).is_some_and(|item| {
        rust_leading_attribute_texts(item, source)
            .iter()
            .any(is_export)
    });
    if in_export_impl {
        return Some(json!({"abi": "uniffi", "kind": "method", "name": name}));
    }
    attrs
        .iter()
        .any(is_export)
        .then(|| json!({"abi": "uniffi", "kind": "function", "name": name}))
}

/// Node.js addon exports: `#[napi]` functions, methods of a `#[napi] impl`
/// (`#[napi(constructor)]` as `constructor`), and `#[neon::export]`
/// functions, under the camelCase name both expose unless `js_name` /
/// `name` says otherwise.
fn rust_node_addon_export(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    name: &str,
    attrs: &[String],
) -> Option<serde_json::Value> {
    let napi_attr = attrs.iter().find(|attr| rust_attr_is(attr, "napi"));
    let napi_impl = rust_enclosing_impl(node).is_some_and(|item| {
        rust_leading_attribute_texts(item, source)
            .iter()
            .any(|attr| rust_attr_is(attr, "napi"))
    });
    if napi_impl {
        let attr = napi_attr?;
        let js_name = if rust_attr_has_flag(attr, "constructor") {
            "constructor".to_string()
        } else {
            rust_attr_string_arg(attr, "js_name").unwrap_or_else(|| js_camel_case(name))
        };
        return Some(json!({"abi": "napi", "kind": "method", "name": js_name}));
    }
    if let Some(attr) = napi_attr {
        let js_name = rust_attr_string_arg(attr, "js_name").unwrap_or_else(|| js_camel_case(name));
        return Some(json!({"abi": "napi", "kind": "function", "name": js_name}));
    }
    let neon_attr = attrs.iter().find(|attr| {
        attr.strip_prefix("#[")
            .is_some_and(|inner| inner.starts_with("neon::export"))
    })?;
    let js_name = rust_attr_string_arg(neon_attr, "name").unwrap_or_else(|| js_camel_case(name));
    Some(json!({"abi": "napi", "kind": "function", "name": js_name}))
}
