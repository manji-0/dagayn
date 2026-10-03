//! Cargo crates another language loads: the library root a `Cargo.toml`
//! builds, and what Python / JavaScript / UniFFI consumers know it by.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::command::{command_options, split_command, strip_quotes};
use super::data::{Json, load_json, load_toml, toml_str, toml_table};
use super::text::{contained_path, join_rel, parent_dir, read_text, resolve_rel};
use super::wasm::build_commands;
use super::{Bridge, Confidence, ManifestBridgeResult, strings};

static WASM_PACK_BUILD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)wasm-pack\s+build\b(?P<args>[^&|;]*)").unwrap());
static CARGO_BUILD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bcargo\s+(?:\+\S+\s+)?build\b(?P<args>[^;&|\n]*)").unwrap());
static UDL_NAMESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*namespace\s+(\w+)").unwrap());

/// A `wasm-pack build` script, repo-relative.
#[derive(Debug)]
pub(super) struct WasmPackBuild {
    crate_rel: String,
    out_dir: Option<String>,
    scope: Option<String>,
}

/// `cargo build --target wasm32-...` run from `cwd`, selecting a crate by
/// `-p` / `--package` or `--manifest-path` (or the crate in `cwd`).
#[derive(Debug)]
pub(super) struct CargoWasmBuild {
    cwd: String,
    target: String,
    package: Option<String>,
    manifest: Option<String>,
}

/// What package.json files say about wasm-pack output, repo-relative.
#[derive(Debug, Default)]
pub(super) struct WasmPackageHints {
    builds: Vec<WasmPackBuild>,
    /// (dependency name, path) for `"name": "file:../crate/pkg"` dependencies.
    file_dependencies: Vec<(String, String)>,
    /// `cargo build --target wasm32-*` commands, from any build file.
    pub cargo_builds: Vec<CargoWasmBuild>,
}

pub(super) fn cargo_wasm_builds(
    repo_root: &Path,
    package_jsons: &[String],
    build_scripts: &[String],
) -> Vec<CargoWasmBuild> {
    let valued = HashSet::from([
        "--target",
        "-p",
        "--package",
        "--manifest-path",
        "--profile",
    ]);
    let mut builds = Vec::new();
    for (command_file, commands) in build_commands(repo_root, package_jsons, build_scripts) {
        let cwd = parent_dir(&command_file);
        for command in &commands {
            for found in CARGO_BUILD_RE.captures_iter(command) {
                let parsed = command_options(&split_command(&found["args"]), &valued);
                let target = parsed.get("--target").unwrap_or("");
                if !target.starts_with("wasm32-") {
                    continue;
                }
                let manifest = parsed
                    .get("--manifest-path")
                    .filter(|manifest| !manifest.is_empty());
                builds.push(CargoWasmBuild {
                    cwd: cwd.clone(),
                    target: target.to_string(),
                    package: parsed
                        .first_non_empty(&["-p", "--package"])
                        .map(str::to_string),
                    manifest: manifest.and_then(|manifest| resolve_rel(&cwd, manifest)),
                });
            }
        }
    }
    builds
}

/// `[build] target = "wasm32-..."` in `.cargo/config.toml` of the crate
/// directory or one of its ancestors.
fn cargo_config_wasm_target(repo_root: &Path, crate_rel: &str) -> Option<String> {
    let parts: Vec<&str> = if crate_rel.is_empty() {
        Vec::new()
    } else {
        crate_rel.split('/').collect()
    };
    for depth in (0..=parts.len()).rev() {
        let mut directory = repo_root.to_path_buf();
        for part in &parts[..depth] {
            directory.push(part);
        }
        for name in ["config.toml", "config"] {
            let Some(data) = load_toml(&directory.join(".cargo").join(name)) else {
                continue;
            };
            let target = toml_table(&data, "build").and_then(|build| toml_str(build, "target"));
            if let Some(target) = target
                && target.starts_with("wasm32-")
            {
                return Some(target.to_string());
            }
        }
    }
    None
}

/// `.wasm` files `cargo build --target wasm32-*` writes for the crate:
/// `<target dir>/<triple>/{release,debug}/<lib>.wasm` under the crate and
/// under the directory the build runs from (a workspace root).
fn cargo_wasm_outputs(
    repo_root: &Path,
    cargo_rel: &str,
    crate_rel: &str,
    package_name: Option<&str>,
    lib_name: &str,
    hints: &WasmPackageHints,
) -> Vec<String> {
    // (target triple, directory the build runs in)
    let mut selections: Vec<(String, String)> = Vec::new();
    for build in &hints.cargo_builds {
        let selected = if let Some(manifest) = &build.manifest {
            manifest == cargo_rel
        } else if let Some(package) = &build.package {
            Some(package.as_str()) == package_name
        } else {
            build.cwd == crate_rel
        };
        if selected {
            selections.push((build.target.clone(), build.cwd.clone()));
        }
    }
    if let Some(configured) = cargo_config_wasm_target(repo_root, crate_rel) {
        selections.push((configured, crate_rel.to_string()));
    }
    let mut outputs: Vec<String> = Vec::new();
    for (target, cwd) in &selections {
        let mut roots = vec![crate_rel];
        if cwd != crate_rel {
            roots.push(cwd);
        }
        for root in roots {
            for profile in ["release", "debug"] {
                let file = format!("{lib_name}.wasm");
                let rel = [root, "target", target, profile, &file]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("/");
                if !outputs.contains(&rel) {
                    outputs.push(rel);
                }
            }
        }
    }
    outputs
}

pub(super) fn collect_wasm_package_hints(
    repo_root: &Path,
    package_jsons: &[String],
) -> WasmPackageHints {
    let mut hints = WasmPackageHints::default();
    for package_rel in package_jsons {
        let Some(data) = load_json(&repo_root.join(package_rel)) else {
            continue;
        };
        let package_dir = parent_dir(package_rel);
        for section in ["dependencies", "devDependencies", "optionalDependencies"] {
            let Some(deps) = data.get_object(section) else {
                continue;
            };
            for (name, spec) in deps.iter() {
                let Some(spec) = spec.as_str() else {
                    continue;
                };
                for prefix in ["file:", "link:", "portal:"] {
                    if let Some(path) = spec.strip_prefix(prefix)
                        && let Some(target) = resolve_rel(&package_dir, path)
                    {
                        hints.file_dependencies.push((name.to_string(), target));
                    }
                }
            }
        }
        let Some(scripts) = data.get_object("scripts") else {
            continue;
        };
        for script in scripts.values().filter_map(Json::as_str) {
            for found in WASM_PACK_BUILD_RE.captures_iter(script) {
                hints
                    .builds
                    .push(parse_wasm_pack_args(&package_dir, &found["args"]));
            }
        }
    }
    hints
}

/// `wasm-pack build [path] --out-dir D --out-name N --scope S` -> parts.
///
/// The crate path is relative to where the script runs (the package.json
/// directory); `--out-dir` is relative to the crate.
fn parse_wasm_pack_args(package_dir: &str, args: &str) -> WasmPackBuild {
    const VALUED: &[&str] = &[
        "--out-dir",
        "-d",
        "--out-name",
        "--scope",
        "-s",
        "--target",
        "-t",
        "--mode",
        "-m",
    ];
    let tokens: Vec<&str> = args.split_whitespace().map(strip_quotes).collect();
    let mut crate_path = ".";
    let mut options: Vec<(&str, &str)> = Vec::new();
    let mut set = |key, value| match options.iter_mut().find(|(name, _)| *name == key) {
        Some(slot) => slot.1 = value,
        None => options.push((key, value)),
    };
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token.starts_with("--") && token.contains('=') {
            let (key, value) = token.split_once('=').unwrap_or((token, ""));
            set(key, value);
        } else if VALUED.contains(&token) {
            if index + 1 < tokens.len() {
                set(token, tokens[index + 1]);
            }
            index += 1;
        } else if !token.starts_with('-') {
            crate_path = token;
        }
        index += 1;
    }
    let get = |key: &str| {
        options
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .filter(|value| !value.is_empty())
    };
    let crate_rel = if crate_path == "." {
        None
    } else {
        resolve_rel(package_dir, crate_path)
    }
    .unwrap_or_else(|| package_dir.to_string());
    let out_dir = get("--out-dir").or_else(|| get("-d"));
    WasmPackBuild {
        out_dir: out_dir.and_then(|out_dir| resolve_rel(&crate_rel, out_dir)),
        scope: get("--scope").or_else(|| get("-s")).map(str::to_string),
        crate_rel,
    }
}

/// True when `crate_name` is a dependency in any `[dependencies]` table.
fn cargo_depends_on(data: &toml::Table, crate_name: &str) -> bool {
    let mut tables = vec![toml_table(data, "dependencies")];
    if let Some(target) = toml_table(data, "target") {
        tables.extend(
            target
                .values()
                .filter_map(toml::Value::as_table)
                .map(|spec| toml_table(spec, "dependencies")),
        );
    }
    tables
        .into_iter()
        .flatten()
        .any(|table| table.contains_key(crate_name))
}

/// JavaScript package names and output directories of a wasm-pack crate.
///
/// wasm-pack writes `<crate>/pkg` named after the crate unless a script
/// passes `--out-dir` / `--scope`; a `file:` dependency on one of those
/// directories adds the name the consumer imports it by.
fn wasm_package_facts(
    crate_rel: &str,
    package_name: &str,
    hints: &WasmPackageHints,
) -> (Vec<String>, Vec<String>) {
    let mut out_dirs = vec![join_rel(crate_rel, "pkg")];
    let mut names = vec![package_name.to_string()];
    for build in hints
        .builds
        .iter()
        .filter(|build| build.crate_rel == crate_rel)
    {
        if let Some(out_dir) = &build.out_dir
            && !out_dirs.contains(out_dir)
        {
            out_dirs.push(out_dir.clone());
        }
        if let Some(scope) = &build.scope {
            let scoped = format!("@{}/{package_name}", scope.trim_start_matches('@'));
            if !names.contains(&scoped) {
                names.push(scoped);
            }
        }
    }
    for (name, path) in &hints.file_dependencies {
        if out_dirs.contains(path) && !names.contains(name) {
            names.push(name.clone());
        }
    }
    (names, out_dirs)
}

/// The names UniFFI generates foreign bindings under: the UDL `namespace`
/// (else the library name; `setup_scaffolding!("ns")` is read from the
/// source later), and the Kotlin package / Swift module `uniffi.toml` may
/// override (`uniffi.<namespace>` / the namespace by default).
fn uniffi_facts(repo_root: &Path, crate_rel: &str, lib_name: &str) -> Value {
    let crate_dir = super::text::abs(repo_root, crate_rel);
    let src_dir = crate_dir.join("src");
    let mut udls: Vec<String> = std::fs::read_dir(&src_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".udl"))
        .collect();
    udls.sort();
    let mut namespace = None;
    for udl in &udls {
        let Some(content) = read_text(&src_dir.join(udl)) else {
            continue;
        };
        if let Some(found) = UDL_NAMESPACE_RE.captures(&content) {
            namespace = Some(found[1].to_string());
            break;
        }
    }
    let mut facts = Map::new();
    facts.insert(
        "namespace".to_string(),
        Value::from(namespace.unwrap_or_else(|| lib_name.to_string())),
    );
    let config = load_toml(&crate_dir.join("uniffi.toml")).unwrap_or_default();
    let bindings = toml_table(&config, "bindings");
    for (language, key, fact) in [
        ("kotlin", "package_name", "kotlin_package"),
        ("swift", "module_name", "swift_module"),
    ] {
        let value = bindings
            .and_then(|bindings| toml_table(bindings, language))
            .and_then(|section| toml_str(section, key))
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(value) = value {
            facts.insert(fact.to_string(), Value::from(value));
        }
    }
    Value::Object(facts)
}

/// How JavaScript reaches a napi-rs / neon crate, from its package.json.
///
/// The package sits in the crate directory or the one above it. JavaScript
/// imports the addon by the package name (or a `file:` dependency on the
/// package directory), through the generated glue (`main` / `types`,
/// `index.js` / `index.d.ts` by default for napi-rs), or by requiring
/// the built `.node` file: neon's `main` (`index.node`), or napi-rs's
/// `<binaryName>.<platform>.node`.
fn node_addon_facts(
    repo_root: &Path,
    crate_rel: &str,
    addon: &str,
    hints: &WasmPackageHints,
) -> Option<Map<String, Value>> {
    let candidates = if crate_rel.is_empty() {
        vec![String::new()]
    } else {
        vec![crate_rel.to_string(), parent_dir(crate_rel)]
    };
    let (package_dir, data) = candidates.into_iter().find_map(|package_dir| {
        let data = load_json(&repo_root.join(join_rel(&package_dir, "package.json")))?;
        Some((package_dir, data))
    })?;
    let mut names: Vec<String> = Vec::new();
    if let Some(name) = data.get_str("name").map(str::trim)
        && !name.is_empty()
    {
        names.push(name.to_string());
    }
    for (dep_name, path) in &hints.file_dependencies {
        if *path == package_dir && !names.contains(dep_name) {
            names.push(dep_name.clone());
        }
    }
    let mut fields: Vec<Option<&str>> = ["main", "types", "typings"]
        .into_iter()
        .map(|key| data.get_str(key))
        .collect();
    if addon == "napi" {
        fields.extend([Some("index.js"), Some("index.d.ts")]);
    } else {
        fields.push(Some("index.node"));
    }
    let mut entries: Vec<String> = Vec::new();
    let mut outputs: Vec<String> = Vec::new();
    for value in fields.into_iter().flatten().map(str::trim) {
        if value.is_empty() {
            continue;
        }
        let Some(rel) = resolve_rel(&package_dir, value) else {
            continue;
        };
        let bucket = if rel.ends_with(".node") {
            &mut outputs
        } else {
            &mut entries
        };
        if !bucket.contains(&rel) {
            bucket.push(rel);
        }
    }
    let mut binary_names: Vec<String> = Vec::new();
    if addon == "napi" {
        // `napi.binaryName` (napi-rs 3) or `napi.name` (2), else `index`.
        let napi = data.get_object("napi");
        let configured = napi.and_then(|napi| match napi.get("binaryName") {
            Some(value) if value.is_truthy() => Some(value),
            _ => napi.get("name"),
        });
        let configured = configured
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        binary_names.push(configured.unwrap_or("index").to_string());
    }
    let mut facts = Map::new();
    facts.insert("js_packages".to_string(), strings(&names));
    facts.insert("js_entry_files".to_string(), strings(&entries));
    facts.insert("node_outputs".to_string(), strings(&outputs));
    facts.insert("node_binary_names".to_string(), strings(&binary_names));
    Some(facts)
}

/// Emit Cargo.toml -> library root for crates another language loads.
///
/// Only crates maturin builds or that declare a `cdylib` count: those are
/// the ones Python imports as an extension module or loads with `ctypes`,
/// and JavaScript imports as a wasm-bindgen package. The edge carries what
/// native-binding resolution matches against -- the library name
/// (`libNAME.so`), the crate directory, the Python module name maturin
/// installs, and for wasm-bindgen crates the JavaScript package names and
/// wasm-pack output directories.
pub(super) fn extract_cargo_crate_root(
    repo_root: &Path,
    cargo_rel: &str,
    maturin_modules: &[(String, Option<String>)],
    wasm_hints: &WasmPackageHints,
    result: &mut ManifestBridgeResult,
) {
    let Some(data) = load_toml(&repo_root.join(cargo_rel)) else {
        return;
    };
    let package = toml_table(&data, "package");
    let empty = toml::Table::new();
    let lib = toml_table(&data, "lib").unwrap_or(&empty);
    let crate_types: Vec<String> = lib
        .get("crate-type")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_string)
        .collect();
    let maturin_module = maturin_modules
        .iter()
        .find(|(rel, _)| rel == cargo_rel)
        .map(|(_, module)| module);
    let is_cdylib = crate_types.iter().any(|kind| kind == "cdylib");
    if maturin_module.is_none() && !is_cdylib {
        return;
    }

    let package_name = package.and_then(|package| package.get("name"));
    let lib_name = match toml_str(lib, "name").filter(|name| !name.trim().is_empty()) {
        Some(name) => name,
        None => match package_name
            .and_then(toml::Value::as_str)
            .filter(|name| !name.trim().is_empty())
        {
            Some(name) => name,
            None => return,
        },
    };
    let package_name = package_name.and_then(toml::Value::as_str);
    let lib_name = lib_name.trim().replace('-', "_");

    let crate_dir = parent_dir(cargo_rel);
    let declared_path = toml_str(lib, "path")
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let (root_rel, evidence_source, confidence) = match declared_path {
        Some(path) => (resolve_rel(&crate_dir, path), "lib.path", Confidence::Exact),
        // Cargo's default library root.
        None => (
            resolve_rel(&crate_dir, "src/lib.rs"),
            "cargo default src/lib.rs",
            Confidence::High,
        ),
    };
    let Some(root_rel) = root_rel else {
        return;
    };
    if !contained_path(repo_root, &root_rel).is_some_and(|path| path.is_file()) {
        return;
    }

    result.ensure_file_node(cargo_rel, "toml");
    let mut extra = Bridge {
        relationship_role: "builds_from_source",
        bridge_kind: "build_config",
        evidence_kind: "manifest",
        evidence_source,
        source_language: "toml",
        target_language: "rust",
        confidence,
    }
    .extra();
    extra.insert("manifest_kind".to_string(), Value::from("cargo"));
    extra.insert("lib_name".to_string(), Value::from(lib_name.clone()));
    extra.insert("crate_types".to_string(), strings(&crate_types));
    extra.insert("crate_dir".to_string(), Value::from(crate_dir.clone()));
    if let Some(module) = maturin_module {
        // maturin installs the extension as `module-name`, or the library name.
        let module = module
            .as_deref()
            .filter(|module| !module.is_empty())
            .unwrap_or(&lib_name);
        extra.insert("python_module".to_string(), Value::from(module));
    }
    let mut wasm_bindgen = false;
    if cargo_depends_on(&data, "wasm-bindgen")
        && let Some(package_name) = package_name
    {
        let (js_packages, out_dirs) =
            wasm_package_facts(&crate_dir, package_name.trim(), wasm_hints);
        wasm_bindgen = true;
        extra.insert("wasm_bindgen".to_string(), Value::from(true));
        extra.insert("js_packages".to_string(), strings(&js_packages));
        extra.insert("wasm_out_dirs".to_string(), strings(&out_dirs));
    }
    if is_cdylib && !wasm_bindgen {
        let wasm_outputs = cargo_wasm_outputs(
            repo_root,
            cargo_rel,
            &crate_dir,
            package_name.map(str::trim),
            &lib_name,
            wasm_hints,
        );
        if !wasm_outputs.is_empty() {
            extra.insert("wasm_outputs".to_string(), strings(&wasm_outputs));
        }
    }
    if cargo_depends_on(&data, "uniffi") {
        extra.insert(
            "uniffi".to_string(),
            uniffi_facts(repo_root, &crate_dir, &lib_name),
        );
    }
    let addon = ["napi", "neon"]
        .into_iter()
        .find(|kind| cargo_depends_on(&data, kind));
    if let Some(addon) = addon
        && let Some(facts) = node_addon_facts(repo_root, &crate_dir, addon, wasm_hints)
    {
        extra.insert("node_addon".to_string(), Value::from(addon));
        extra.extend(facts);
    }
    result.push_edge(cargo_rel, &root_rel, cargo_rel, extra);
}
