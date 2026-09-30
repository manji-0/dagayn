//! Cross-language bridges from Python calls: subprocess launches, file I/O, shared-library loads (ctypes, cffi), and WebAssembly hosts (wasmtime, wasmer).

use super::*;

pub(crate) fn python_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
    import_aliases: &HashMap<String, String>,
) -> Option<ParsedEdge> {
    let signature =
        python_canonical_signature(python_call_signature(node, source)?, import_aliases);
    let line = node.start_position().row as i64 + 1;
    if let Some((relationship_role, target)) = python_wasm_host_bridge(node, source, &signature) {
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
                "source_language": "python",
                "target_language": "unknown",
                "confidence": 0.8,
                "confidence_tier": "HIGH",
            }),
        });
    }
    let (relationship_role, bridge_kind) = python_bridge_pattern(&signature)?;
    let (target, confidence, confidence_tier) = match python_first_string_arg(node, source) {
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
            "source_language": "python",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

/// Spell the callee through the import that bound its head, so
/// `from ctypes import CDLL; CDLL(...)` and `import ctypes as ct;
/// ct.CDLL(...)` both read `ctypes.CDLL`.
fn python_canonical_signature(
    signature: String,
    import_aliases: &HashMap<String, String>,
) -> String {
    let (head, rest) = match signature.find('.') {
        Some(index) => signature.split_at(index),
        None => (signature.as_str(), ""),
    };
    let (name, call_suffix) = match head.strip_suffix("()") {
        Some(name) => (name, "()"),
        None => (head, ""),
    };
    match import_aliases.get(name) {
        Some(origin) if origin != name => format!("{origin}{call_suffix}{rest}"),
        _ => signature,
    }
}

fn python_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "argument_list")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A WebAssembly host (wasmtime / wasmer): a call with a string argument
/// naming a `.wasm` file (`Module.from_file(engine, "guest.wasm")`) loads
/// the module; `instance.exports(store).get("add")` and wasmer's
/// `instance.exports.add(...)` call its export.
fn python_wasm_host_bridge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    signature: &str,
) -> Option<(&'static str, String)> {
    let arguments = node
        .children(&mut node.walk())
        .find(|child| child.kind() == "argument_list")?;
    let strings: Vec<String> = arguments
        .children(&mut arguments.walk())
        .filter(|child| child.kind() == "string")
        .filter_map(|child| python_string_literal_text(child, source))
        .collect();
    if let Some(path) = strings
        .iter()
        .find(|value| value.to_ascii_lowercase().ends_with(".wasm"))
    {
        return Some(("loads_wasm_module", path.clone()));
    }
    if signature.ends_with(".get") && signature.contains(".exports(") {
        return Some(("calls_wasm_export", strings.first()?.clone()));
    }
    let (object, name) = signature.rsplit_once('.')?;
    (object.ends_with(".exports") && !name.is_empty())
        .then(|| ("calls_wasm_export", name.to_string()))
}

fn python_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "subprocess.run"
        | "subprocess.Popen"
        | "subprocess.call"
        | "subprocess.check_call"
        | "subprocess.check_output"
        | "os.system"
        | "os.popen"
        | "os.execv"
        | "os.execvp"
        | "os.execvpe"
        | "os.execve"
        | "os.execl"
        | "os.execlp"
        | "os.execlpe"
        | "os.execle"
        | "os.spawnv"
        | "os.spawnvp" => Some(("invokes_binary", "subprocess")),
        "ctypes.CDLL"
        | "ctypes.cdll.LoadLibrary"
        | "ctypes.WinDLL"
        | "ctypes.PyDLL"
        | "cffi.FFI().dlopen" => Some(("loads_shared_library", "ffi")),
        "open" | "io.open" => Some(("opens_file", "file_io")),
        // `ffi = cffi.FFI(); lib = ffi.dlopen("libfoo.so")`: the instance is
        // a variable, so the receiver cannot be spelled out.
        _ if signature.ends_with(".dlopen") => Some(("loads_shared_library", "ffi")),
        _ => None,
    }
}

fn python_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if child.kind() == "string" {
            return Some(decode_python_string_literal(child, source));
        }
        if matches!(child.kind(), "list" | "tuple") {
            return python_first_string_in_sequence(child, source);
        }
        return None;
    }
    None
}

fn python_first_string_in_sequence(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if child.kind() == "string" {
            return Some(decode_python_string_literal(child, source));
        }
    }
    None
}
