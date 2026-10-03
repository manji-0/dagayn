//! Python extension modules built from Rust: maturin and setuptools-rust.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::data::{load_toml, toml_str, toml_table};
use super::text::{contained_path, is_file, lossy_slice, parent_dir, read_text, resolve_rel};
use super::{Bridge, Confidence, ManifestBridgeResult};

static RUST_EXTENSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bRustExtension\s*\(").unwrap());
// One pattern per quote character: the value may hold neither quote.
static PY_DOUBLE_QUOTED_ARG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\A\s*(?:(?P<key>\w+)\s*=\s*)?"(?P<value>[^"']*)"\s*\z"#).unwrap()
});
static PY_SINGLE_QUOTED_ARG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\A\s*(?:(?P<key>\w+)\s*=\s*)?'(?P<value>[^"']*)'\s*\z"#).unwrap()
});

/// Emit pyproject.toml -> Cargo.toml; return (Cargo.toml, module-name).
pub(super) fn extract_maturin_bridges(
    repo_root: &Path,
    pyproject_rel: &str,
    result: &mut ManifestBridgeResult,
) -> Option<(String, Option<String>)> {
    let data = load_toml(&repo_root.join(pyproject_rel))?;
    let maturin = toml_table(toml_table(&data, "tool")?, "maturin")?;

    let pyproject_dir = parent_dir(pyproject_rel);
    let manifest_path = toml_str(maturin, "manifest-path").map(str::trim);
    let module_name = toml_str(maturin, "module-name")
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let (cargo_rel, evidence_source, confidence) = match manifest_path {
        Some(path) if !path.is_empty() => (
            resolve_rel(&pyproject_dir, path),
            "tool.maturin.manifest-path",
            Confidence::Exact,
        ),
        // Default maturin layout: Cargo.toml beside pyproject.toml.
        _ => (
            resolve_rel(&pyproject_dir, "Cargo.toml"),
            "tool.maturin",
            Confidence::High,
        ),
    };
    // `None`: manifest-path escapes the repository root.
    let cargo_rel = cargo_rel?;
    if !contained_path(repo_root, &cargo_rel).is_some_and(|path| path.is_file()) {
        return None;
    }

    result.ensure_file_node(pyproject_rel, "toml");
    result.ensure_file_node(&cargo_rel, "toml");
    let mut extra = Bridge {
        relationship_role: "builds_artifact",
        bridge_kind: "extension_module",
        evidence_kind: "manifest",
        evidence_source,
        source_language: "python",
        target_language: "rust",
        confidence,
    }
    .extra();
    if let Some(module_name) = module_name {
        extra.insert("module_name".to_string(), Value::from(module_name));
    }
    extra.insert("manifest_kind".to_string(), Value::from("maturin"));
    result.push_edge(pyproject_rel, &cargo_rel, pyproject_rel, extra);
    Some((cargo_rel, module_name.map(str::to_string)))
}

/// String-literal arguments of the call whose `(` ends at `start`:
/// positional values in order, and `key="value"` keywords. Other
/// arguments are skipped.
fn python_call_string_args(text: &str, start: usize) -> (Vec<String>, HashMap<String, String>) {
    let bytes = text.as_bytes();
    let mut args = Vec::new();
    let (mut depth, mut current, mut index) = (1_i32, Vec::new(), start);
    while index < bytes.len() && depth != 0 {
        let byte = bytes[index];
        if b"([{".contains(&byte) {
            depth += 1;
        } else if b")]}".contains(&byte) {
            depth -= 1;
        }
        if depth == 0 || (byte == b',' && depth == 1) {
            args.push(lossy_slice(&current, 0, current.len()));
            current.clear();
        } else {
            current.push(byte);
        }
        index += 1;
    }
    let mut positional = Vec::new();
    let mut keywords = HashMap::new();
    for arg in &args {
        let Some(found) = PY_DOUBLE_QUOTED_ARG_RE
            .captures(arg)
            .or_else(|| PY_SINGLE_QUOTED_ARG_RE.captures(arg))
        else {
            continue;
        };
        let value = found["value"].to_string();
        match found.name("key") {
            Some(key) => {
                keywords.insert(key.as_str().to_string(), value);
            }
            None => positional.push(value),
        }
    }
    (positional, keywords)
}

/// setuptools-rust extensions: `RustExtension("pkg._core", "Cargo.toml")`
/// in `setup.py`, or `[[tool.setuptools-rust.ext-modules]]` with
/// `target` / `path` in `pyproject.toml`. Emits config -> Cargo.toml
/// and returns `(Cargo.toml, module)` pairs.
pub(super) fn extract_setuptools_rust(
    repo_root: &Path,
    rel_path: &str,
    result: &mut ManifestBridgeResult,
) -> Vec<(String, String)> {
    let base = parent_dir(rel_path);
    let is_toml = rel_path.ends_with(".toml");
    // (module, Cargo.toml as written)
    let mut declared: Vec<(String, String)> = Vec::new();
    if is_toml {
        let data = load_toml(&repo_root.join(rel_path)).unwrap_or_default();
        let modules = toml_table(&data, "tool")
            .and_then(|tool| toml_table(tool, "setuptools-rust"))
            .and_then(|section| section.get("ext-modules"))
            .and_then(toml::Value::as_array);
        for module in modules.into_iter().flatten() {
            let Some(module) = module.as_table() else {
                continue;
            };
            if let Some(target) = toml_str(module, "target") {
                let path = toml_str(module, "path").unwrap_or("Cargo.toml");
                declared.push((target.to_string(), path.to_string()));
            }
        }
    } else {
        let Some(text) = read_text(&repo_root.join(rel_path)) else {
            return Vec::new();
        };
        for found in RUST_EXTENSION_RE.find_iter(&text) {
            let (positional, keywords) = python_call_string_args(&text, found.end());
            let target = match keywords.get("target") {
                Some(target) if !target.is_empty() => Some(target.clone()),
                _ => positional.first().cloned(),
            };
            let Some(target) = target else {
                continue;
            };
            let rest = if keywords.contains_key("target") {
                &positional[..]
            } else {
                &positional[1..]
            };
            let path = match keywords.get("path") {
                Some(path) if !path.is_empty() => path.clone(),
                _ => rest
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Cargo.toml".to_string()),
            };
            declared.push((target, path));
        }
    }
    let mut found = Vec::new();
    for (module, path) in declared {
        let Some(cargo_rel) = resolve_rel(&base, &path) else {
            continue;
        };
        if !is_file(repo_root, &cargo_rel) {
            continue;
        }
        result.ensure_file_node(rel_path, if is_toml { "toml" } else { "python" });
        result.ensure_file_node(&cargo_rel, "toml");
        let mut extra = Bridge {
            relationship_role: "builds_artifact",
            bridge_kind: "extension_module",
            evidence_kind: "manifest",
            evidence_source: "setuptools-rust",
            source_language: "python",
            target_language: "rust",
            confidence: Confidence::Exact,
        }
        .extra();
        extra.insert("module_name".to_string(), Value::from(module.clone()));
        extra.insert("manifest_kind".to_string(), Value::from("setuptools-rust"));
        result.push_edge(rel_path, &cargo_rel, rel_path, extra);
        found.push((cargo_rel, module));
    }
    found
}
