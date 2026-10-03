//! node-gyp `binding.gyp` targets.

use std::path::Path;

use super::super::text::{parent_dir, read_text, resolve_rel, splitlines};
use super::pyliteral::{PyLiteral, literal_eval};
use super::{NativeLibrary, existing_sources};

/// `binding.gyp`: a Python literal with `#` comments.
pub(super) fn gyp_libraries(repo_root: &Path, gyp_rel: &str) -> Vec<NativeLibrary> {
    let Some(content) = read_text(&repo_root.join(gyp_rel)) else {
        return Vec::new();
    };
    let stripped = splitlines(&content)
        .into_iter()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let Some(data) = literal_eval(&stripped) else {
        return Vec::new();
    };
    let Some(PyLiteral::List(targets)) = data.get("targets") else {
        return Vec::new();
    };
    let base = parent_dir(gyp_rel);
    let mut libraries = Vec::new();
    for target in targets {
        if !matches!(target, PyLiteral::Dict(_)) {
            continue;
        }
        let kind = target
            .get("type")
            .map_or(Some("loadable_module"), PyLiteral::as_str);
        if !matches!(kind, Some("loadable_module" | "shared_library")) {
            continue;
        }
        let (Some(name), Some(PyLiteral::List(sources))) = (
            target.get("target_name").and_then(PyLiteral::as_str),
            target.get("sources"),
        ) else {
            continue;
        };
        let items: Vec<String> = sources
            .iter()
            .filter_map(PyLiteral::as_str)
            .map(str::to_string)
            .collect();
        let found = existing_sources(repo_root, &base, &items);
        if found.is_empty() {
            continue;
        }
        let outputs = ["Release", "Debug"]
            .into_iter()
            .filter_map(|config| resolve_rel(&base, &format!("build/{config}/{name}.node")))
            .collect();
        let mut library = NativeLibrary::new(gyp_rel, "node-gyp", name.to_string(), found);
        library.outputs = outputs;
        libraries.push(library);
    }
    libraries
}
