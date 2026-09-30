use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_go_with_parser(
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
        language: "go".to_string(),
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
        go_walk_children(root, source, &file_path, None, &mut nodes, &mut edges);
        go_apply_js_global_exports(root, source, &mut nodes);
        let libraries = go_cgo_libraries(root, source);
        if !libraries.is_empty() {
            nodes[0].extra["cgo_libraries"] = json!(libraries);
        }
        let edges = resolve_rust_call_targets(&nodes, edges, &file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

fn go_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                go_emit_imports(child, source, file_path, edges);
            }
            "type_declaration" => {
                go_emit_types(child, source, file_path, nodes, edges);
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
                    go_walk_children(child, source, file_path, Some(&scope), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                go_emit_call(child, source, file_path, enclosing_func, edges);
            }
            _ => {}
        }
        go_walk_children(child, source, file_path, enclosing_func, nodes, edges);
    }
}

fn go_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_emit_imports(child, source, file_path, edges);
        if child.kind() == "interpreted_string_literal" {
            let target = strip_matching_quotes(node_text(child, source).trim()).to_string();
            if !target.is_empty() {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.clone(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn go_emit_types(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "type_spec" {
            continue;
        }
        let Some(name) =
            go_direct_child_text(child, source, "type_identifier").filter(|name| name != "_")
        else {
            continue;
        };
        let qualified = qualify(file_path, &name, None);
        let extra = go_type_extra(child, source);
        nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Class,
            name,
            file_path: file_path.clone(),
            line_start: child.start_position().row as i64 + 1,
            line_end: child.end_position().row as i64 + 1,
            language: "go".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        });
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: file_path.to_string(),
            target: qualified,
            file_path: file_path.clone(),
            line: child.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn go_type_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> serde_json::Value {
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
        }
    }
    extra
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
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: go_directive_extra(node, source),
    });
    let container = receiver
        .map(|receiver| qualify(file_path, receiver, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
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
        return go_direct_child_text(node, source, "identifier").map(|name| (name, None));
    }
    let name = go_direct_child_text(node, source, "field_identifier")?;
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
    source: &[u8],
    file_path: &FilePath,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some((call_name, signature)) = go_call_name_and_signature(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name,
        file_path: file_path.clone(),
        line: node.start_position().row as i64 + 1,
        // `C.fast_sum(...)` calls C through cgo.
        extra: if signature.starts_with("C.") {
            json!({"receiver": "C"})
        } else {
            json!({})
        },
    });
    if let Some(edge) = go_bridge_edge(node, source, file_path, &caller, &signature) {
        edges.push(edge);
    }
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

fn go_direct_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
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
