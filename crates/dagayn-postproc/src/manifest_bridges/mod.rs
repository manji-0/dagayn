//! Layer-2 manifest-backed CROSS_ARTIFACT bridge extraction.
//!
//! Parses common build and codegen manifests and emits explainable
//! `CROSS_ARTIFACT` edges with confidence/evidence in `extra`.
//!
//! Prefer exact manifest fields (`tool.maturin.manifest-path`,
//! `openapitools.json` generator `inputSpec`/`output`, package.json
//! dependency on a generated package name) over naming-only heuristics.

mod cargo;
mod command;
mod data;
mod native;
mod openapi;
mod python_ext;
mod text;
mod walk;
mod wasm;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;

use dagayn_graph::{EdgeInput, NodeInput};
use dagayn_parser::IgnoreRules;
use serde::Serialize;
use serde_json::{Map, Value, json};

pub use text::resolve_rel;

pub const EXTRACTOR_ID: &str = "manifest_bridges";

/// Nodes and edges discovered from manifests under a repository root.
#[derive(Debug, Default, Serialize)]
pub struct ManifestBridgeResult {
    pub nodes: Vec<NodeInput>,
    pub edges: Vec<EdgeInput>,
}

const MANIFEST_NAMES: &[&str] = &[
    "pyproject.toml",
    "Cargo.toml",
    "openapitools.json",
    "package.json",
    "asconfig.json",
    "Makefile",
    "makefile",
    "GNUmakefile",
    "justfile",
    "CMakeLists.txt",
    "meson.build",
    "binding.gyp",
    "build.rs",
    "setup.py",
    "build.zig",
];

/// Scan `repo_root` for supported manifests and build bridge edges, with
/// File node `line_end`s read from disk.
///
/// `scope` is the repo-relative indexable set (the VCS listing). When given,
/// manifests outside it are not read, and nodes and edges that name a file
/// outside it are dropped, including files a tracked manifest points at. A
/// gitignored file stored here is pruned as out of scope by the next
/// incremental update and re-added by the post-processing that update runs.
pub fn discover_manifest_bridges(
    repo_root: &Path,
    scope: Option<&HashSet<String>>,
) -> ManifestBridgeResult {
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let repo_root = repo_root.as_path();
    let mut result = ManifestBridgeResult::default();
    let ignore = IgnoreRules::load(repo_root);

    let mut found = walk::collect_named_files(repo_root, MANIFEST_NAMES, &ignore);
    if let Some(scope) = scope {
        for paths in found.values_mut() {
            paths.retain(|path| scope.contains(path));
        }
    }
    let files = |names: &[&str]| -> Vec<String> {
        let mut paths: Vec<String> = names.iter().flat_map(|name| found[*name].clone()).collect();
        paths.sort();
        paths
    };
    let pyprojects = files(&["pyproject.toml"]);
    let package_jsons = files(&["package.json"]);
    let build_scripts = files(&["Makefile", "makefile", "GNUmakefile", "justfile"]);

    // Generator output roots (repo-relative), later mapped to npm package names.
    let mut generated_roots: Vec<String> = Vec::new();

    // Cargo.toml -> Python module name, for crates maturin builds.
    let mut maturin_modules: Vec<(String, Option<String>)> = Vec::new();
    for rel_path in &pyprojects {
        if let Some((cargo_rel, module_name)) =
            python_ext::extract_maturin_bridges(repo_root, rel_path, &mut result)
        {
            match maturin_modules
                .iter_mut()
                .find(|(rel, _)| *rel == cargo_rel)
            {
                Some(slot) => slot.1 = module_name,
                None => maturin_modules.push((cargo_rel, module_name)),
            }
        }
    }
    // setuptools-rust names the module the same way maturin does.
    for rel_path in files(&["pyproject.toml", "setup.py"]) {
        for (cargo_rel, module_name) in
            python_ext::extract_setuptools_rust(repo_root, &rel_path, &mut result)
        {
            if !maturin_modules.iter().any(|(rel, _)| *rel == cargo_rel) {
                maturin_modules.push((cargo_rel, Some(module_name)));
            }
        }
    }

    let mut wasm_hints = cargo::collect_wasm_package_hints(repo_root, &package_jsons);
    wasm_hints.cargo_builds = cargo::cargo_wasm_builds(repo_root, &package_jsons, &build_scripts);
    for rel_path in &found["Cargo.toml"] {
        cargo::extract_cargo_crate_root(
            repo_root,
            rel_path,
            &maturin_modules,
            &wasm_hints,
            &mut result,
        );
    }

    wasm::extract_wasm_producers(
        repo_root,
        &package_jsons,
        &build_scripts,
        &found["asconfig.json"],
        &mut result,
    );

    let inputs = native::NativeBuildFiles {
        cmake_lists: found["CMakeLists.txt"].clone(),
        meson_builds: found["meson.build"].clone(),
        makefiles: files(&["Makefile", "makefile", "GNUmakefile"]),
        command_sources: wasm::build_commands(repo_root, &package_jsons, &found["justfile"]),
        binding_gyps: found["binding.gyp"].clone(),
        cargo_build_scripts: found["build.rs"].clone(),
        zig_builds: found["build.zig"].clone(),
    };
    native::extract_native_libraries(repo_root, &inputs, &mut result);

    for rel_path in &found["openapitools.json"] {
        openapi::extract_openapitools_bridges(
            repo_root,
            rel_path,
            &mut result,
            &mut generated_roots,
        );
    }
    for rel_path in &package_jsons {
        openapi::extract_package_json_generator_scripts(
            repo_root,
            rel_path,
            &mut result,
            &mut generated_roots,
        );
    }
    let generated_by_name = openapi::index_generated_package_names(repo_root, &generated_roots);
    for rel_path in &package_jsons {
        openapi::extract_generated_client_consumers(
            repo_root,
            rel_path,
            &generated_by_name,
            &mut result,
        );
    }

    if let Some(scope) = scope {
        restrict_to_scope(repo_root, &mut result, scope);
    }
    refine_node_line_ends(repo_root, &mut result.nodes);
    result
}

/// Drop nodes and edges that name a repository file outside `scope`.
fn restrict_to_scope(repo_root: &Path, result: &mut ManifestBridgeResult, scope: &HashSet<String>) {
    let out_of_scope = |reference: &str| {
        let path = reference.split("::").next().unwrap_or(reference);
        !scope.contains(path) && text::is_file(repo_root, path)
    };
    result.nodes.retain(|node| !out_of_scope(&node.file_path));
    result
        .edges
        .retain(|edge| !out_of_scope(&edge.source) && !out_of_scope(&edge.target));
}

/// Fill `line_end` for File nodes from on-disk content when available.
pub fn refine_node_line_ends(repo_root: &Path, nodes: &mut [NodeInput]) {
    for node in nodes {
        if node.kind != "File" {
            continue;
        }
        let Some(path) = text::contained_path(repo_root, &node.file_path) else {
            continue;
        };
        let Some(content) = text::read_text(&path) else {
            continue;
        };
        let lines = content.matches('\n').count() + usize::from(!content.ends_with('\n'));
        node.line_end = i64::try_from(lines.max(1)).unwrap_or(i64::MAX);
    }
}

impl ManifestBridgeResult {
    fn ensure_file_node(&mut self, rel_path: &str, language: &str) {
        if self
            .nodes
            .iter()
            .any(|node| node.kind == "File" && node.file_path == rel_path)
        {
            return;
        }
        self.nodes.push(NodeInput {
            kind: "File".to_string(),
            name: rel_path.to_string(),
            file_path: rel_path.to_string(),
            line_start: 1,
            line_end: 1,
            language: language.to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({
                "extractor": EXTRACTOR_ID,
                "node_role": "Artifact",
                "origin_file": rel_path,
            }),
        });
    }

    fn push_edge(
        &mut self,
        source: &str,
        target: &str,
        file_path: &str,
        extra: Map<String, Value>,
    ) {
        self.edges.push(EdgeInput {
            kind: "CROSS_ARTIFACT".to_string(),
            source: source.to_string(),
            target: target.to_string(),
            file_path: file_path.to_string(),
            line: 0,
            extra: Value::Object(extra),
        });
    }
}

/// Confidence contract for Layer-2 manifest bridges (see
/// docs/CROSS-ARTIFACT-EDGES-WIP.md).
#[derive(Clone, Copy)]
enum Confidence {
    Exact,
    High,
}

impl Confidence {
    fn value(self) -> f64 {
        match self {
            Confidence::Exact => 1.0,
            Confidence::High => 0.8,
        }
    }

    fn tier(self) -> &'static str {
        match self {
            Confidence::Exact => "EXACT",
            Confidence::High => "HIGH",
        }
    }
}

/// The metadata every bridge edge carries.
struct Bridge<'a> {
    relationship_role: &'a str,
    bridge_kind: &'a str,
    evidence_kind: &'a str,
    evidence_source: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    confidence: Confidence,
}

impl Bridge<'_> {
    fn extra(&self) -> Map<String, Value> {
        let mut extra = Map::new();
        for (key, value) in [
            ("relationship_role", self.relationship_role),
            ("bridge_kind", self.bridge_kind),
            ("evidence_kind", self.evidence_kind),
            ("evidence_source", self.evidence_source),
            ("source_language", self.source_language),
            ("target_language", self.target_language),
            ("confidence_tier", self.confidence.tier()),
            ("extractor", EXTRACTOR_ID),
        ] {
            extra.insert(key.to_string(), Value::from(value));
        }
        extra.insert(
            "confidence".to_string(),
            Value::from(self.confidence.value()),
        );
        extra
    }
}

fn strings(values: &[String]) -> Value {
    Value::from(values.to_vec())
}
