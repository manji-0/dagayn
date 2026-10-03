//! WebAssembly modules Go / TinyGo / AssemblyScript builds produce, and the
//! command lines build files run.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::command::{command_options, split_command};
use super::data::{Json, load_json};
use super::text::{
    abs, is_file, join_rel, parent_dir, py_suffix, read_text, resolve_rel, splitlines,
};
use super::{Bridge, Confidence, ManifestBridgeResult, strings};

static GO_WASM_BUILD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?P<tool>tinygo|go)\s+build\b(?P<args>[^;&|\n]*)").unwrap()
});
static ASC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[\s;&|(])(?:npx\s+)?asc\s+(?P<args>[^;&|\n]*)").unwrap());
static GO_MAIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^func\s+main\s*\(").unwrap());
const WASM_TARGETS: &[&str] = &["wasm", "wasi", "wasip1", "wasip2", "wasm-unknown"];

/// A WebAssembly module some build command produces from repo sources.
struct WasmProducer {
    config_rel: String,
    /// "go" | "tinygo" | "assemblyscript"
    producer: String,
    root_rel: String,
    outputs: Vec<String>,
    export_dir: Option<String>,
    entry_files: Vec<String>,
}

/// (file, command lines) from package.json scripts, Makefiles, justfiles.
pub(super) fn build_commands(
    repo_root: &Path,
    package_jsons: &[String],
    build_scripts: &[String],
) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for rel in package_jsons {
        let data = load_json(&repo_root.join(rel));
        let scripts = data
            .as_ref()
            .filter(|data| !data.is_empty())
            .and_then(|data| data.get_object("scripts"));
        if let Some(scripts) = scripts {
            let commands = scripts
                .values()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect();
            out.push((rel.clone(), commands));
        }
    }
    for rel in build_scripts {
        let Some(content) = read_text(&repo_root.join(rel)) else {
            continue;
        };
        let content = content.replace("\\\n", " ");
        let lines = splitlines(&content)
            .into_iter()
            .map(str::to_string)
            .collect();
        out.push((rel.clone(), lines));
    }
    out
}

/// Emit build config -> source edges for Go / TinyGo / AssemblyScript
/// WebAssembly builds, carrying the `.wasm` outputs JavaScript loads and
/// where the exported functions live.
pub(super) fn extract_wasm_producers(
    repo_root: &Path,
    package_jsons: &[String],
    build_scripts: &[String],
    asconfigs: &[String],
    result: &mut ManifestBridgeResult,
) {
    let mut producers: Vec<WasmProducer> = Vec::new();
    let mut add = |producer: WasmProducer| {
        let existing = producers.iter_mut().find(|existing| {
            existing.config_rel == producer.config_rel && existing.root_rel == producer.root_rel
        });
        match existing {
            None => producers.push(producer),
            Some(existing) => {
                for output in producer.outputs {
                    if !existing.outputs.contains(&output) {
                        existing.outputs.push(output);
                    }
                }
            }
        }
    };
    for (command_file, commands) in build_commands(repo_root, package_jsons, build_scripts) {
        let cwd = parent_dir(&command_file);
        for command in &commands {
            for producer in wasm_producers_in_command(repo_root, &command_file, &cwd, command) {
                add(producer);
            }
        }
    }
    for config_rel in asconfigs {
        if let Some(producer) = assemblyscript_asconfig(repo_root, config_rel) {
            add(producer);
        }
    }

    for producer in producers {
        if producer.outputs.is_empty() {
            continue;
        }
        let language = if producer.config_rel.ends_with(".json") {
            "json"
        } else {
            "make"
        };
        result.ensure_file_node(&producer.config_rel, language);
        let evidence_source = format!("{} build", producer.producer);
        let mut extra = Bridge {
            relationship_role: "builds_from_source",
            bridge_kind: "build_config",
            evidence_kind: "config",
            evidence_source: &evidence_source,
            source_language: language,
            target_language: if matches!(producer.producer.as_str(), "go" | "tinygo") {
                "go"
            } else {
                "typescript"
            },
            confidence: Confidence::High,
        }
        .extra();
        extra.insert("manifest_kind".to_string(), Value::from("wasm_build"));
        extra.insert(
            "wasm_producer".to_string(),
            Value::from(producer.producer.clone()),
        );
        extra.insert("wasm_outputs".to_string(), strings(&producer.outputs));
        if let Some(export_dir) = &producer.export_dir {
            extra.insert("export_dir".to_string(), Value::from(export_dir.clone()));
        }
        if !producer.entry_files.is_empty() {
            extra.insert("entry_files".to_string(), strings(&producer.entry_files));
        }
        result.push_edge(
            &producer.config_rel,
            &producer.root_rel,
            &producer.config_rel,
            extra,
        );
    }
}

fn wasm_producers_in_command(
    repo_root: &Path,
    command_file: &str,
    cwd: &str,
    command: &str,
) -> Vec<WasmProducer> {
    let mut producers = Vec::new();
    let go_valued = HashSet::from([
        "-o",
        "-target",
        "-tags",
        "-ldflags",
        "-gcflags",
        "-scheduler",
        "-gc",
        "-opt",
    ]);
    for found in GO_WASM_BUILD_RE.captures_iter(command) {
        let tool = found["tool"].to_lowercase();
        let parsed = command_options(&split_command(&found["args"]), &go_valued);
        if tool == "go" && !command.replace(' ', "").contains("GOARCH=wasm") {
            continue;
        }
        if tool == "tinygo"
            && !WASM_TARGETS.contains(&parsed.get("-target").unwrap_or("").to_lowercase().as_str())
        {
            continue;
        }
        let Some(output) = parsed
            .get("-o")
            .filter(|output| !output.is_empty() && output.ends_with(".wasm"))
        else {
            continue;
        };
        let package = parsed.positional.last().map_or(".", String::as_str);
        if !package.starts_with('.') {
            continue; // a module import path, not a directory
        }
        let package_rel = if package == "." {
            Some(cwd.to_string())
        } else {
            resolve_rel(cwd, package)
        };
        let (Some(package_rel), Some(output_rel)) = (package_rel, resolve_rel(cwd, output)) else {
            continue;
        };
        let Some(root) = go_main_file(repo_root, &package_rel) else {
            continue;
        };
        producers.push(WasmProducer {
            config_rel: command_file.to_string(),
            producer: tool,
            root_rel: root,
            outputs: vec![output_rel],
            export_dir: Some(package_rel),
            entry_files: Vec::new(),
        });
    }
    let asc_valued = HashSet::from(["--outFile", "-o", "--target", "--config"]);
    for found in ASC_RE.captures_iter(command) {
        let parsed = command_options(&split_command(&found["args"]), &asc_valued);
        let output = parsed.first_non_empty(&["--outFile", "-o"]);
        let entries: Vec<String> = parsed
            .positional
            .iter()
            .filter(|token| token.ends_with(".ts"))
            .filter_map(|token| resolve_rel(cwd, token))
            .filter(|rel| is_file(repo_root, rel))
            .collect();
        // `asc --target release` reads asconfig.json instead.
        let Some(output) = output.filter(|output| !output.is_empty()) else {
            continue;
        };
        if entries.is_empty() {
            continue;
        }
        let Some(output_rel) = resolve_rel(cwd, output) else {
            continue;
        };
        producers.push(WasmProducer {
            config_rel: command_file.to_string(),
            producer: "assemblyscript".to_string(),
            root_rel: entries[0].clone(),
            outputs: vec![output_rel],
            export_dir: None,
            entry_files: entries,
        });
    }
    producers
}

/// The file of a Go package directory that declares `func main`, else the
/// first `.go` file (tests excluded).
fn go_main_file(repo_root: &Path, package_rel: &str) -> Option<String> {
    let directory = abs(repo_root, package_rel);
    if !directory.is_dir() {
        return None;
    }
    let mut files: Vec<String> = std::fs::read_dir(&directory)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| py_suffix(name) == ".go" && !name.ends_with("_test.go"))
        .filter(|name| directory.join(name).is_file())
        .collect();
    files.sort();
    for name in &files {
        let Some(content) = read_text(&directory.join(name)) else {
            continue;
        };
        if GO_MAIN_RE.is_match(&content) {
            return Some(join_rel(package_rel, name));
        }
    }
    files.first().map(|name| join_rel(package_rel, name))
}

/// `asconfig.json`: `entries` and every target's `outFile`.
fn assemblyscript_asconfig(repo_root: &Path, config_rel: &str) -> Option<WasmProducer> {
    let data = load_json(&repo_root.join(config_rel))?;
    let base = parent_dir(config_rel);
    let entries: Vec<String> = data
        .get("entries")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Json::as_str)
        .filter_map(|entry| resolve_rel(&base, entry))
        .filter(|rel| is_file(repo_root, rel))
        .collect();
    let mut outputs: Vec<String> = Vec::new();
    if let Some(targets) = data.get_object("targets") {
        for target in targets.values() {
            let out_file = target
                .as_object()
                .and_then(|target| target.get_str("outFile"));
            if let Some(rel) = out_file.and_then(|out_file| resolve_rel(&base, out_file))
                && !outputs.contains(&rel)
            {
                outputs.push(rel);
            }
        }
    }
    if entries.is_empty() || outputs.is_empty() {
        return None;
    }
    Some(WasmProducer {
        config_rel: config_rel.to_string(),
        producer: "assemblyscript".to_string(),
        root_rel: entries[0].clone(),
        outputs,
        export_dir: None,
        entry_files: entries,
    })
}
