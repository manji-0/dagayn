//! OpenAPI generator configs and the packages that consume generated clients.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::command::strip_quotes;
use super::data::load_json;
use super::text::{contained_path, parent_dir, resolve_rel};
use super::{Bridge, Confidence, ManifestBridgeResult};

// Python's `$` also matches before a final newline.
static OPENAPI_GENERATOR_SCRIPT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)openapi-generator(?:-cli)?\s+generate\b(?P<args>.*)\n?\z").unwrap()
});
static CLI_INPUT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:--input-spec|-i)\s+(?P<path>(?:"[^"]+"|'[^']+'|[^\s]+))"#).unwrap()
});
static CLI_OUTPUT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:--output|-o)\s+(?P<path>(?:"[^"]+"|'[^']+'|[^\s]+))"#).unwrap()
});

pub(super) fn extract_openapitools_bridges(
    repo_root: &Path,
    config_rel: &str,
    result: &mut ManifestBridgeResult,
    generated_roots: &mut Vec<String>,
) {
    let Some(data) = load_json(&repo_root.join(config_rel)) else {
        return;
    };
    let Some(generators) = data
        .get_object("generator-cli")
        .and_then(|cli| cli.get_object("generators"))
    else {
        return;
    };
    let config_dir = parent_dir(config_rel);
    for (gen_name, gen_cfg) in generators.iter() {
        let Some(gen_cfg) = gen_cfg.as_object() else {
            continue;
        };
        let input_spec = match gen_cfg.get("inputSpec") {
            Some(value) if value.is_truthy() => Some(value),
            _ => gen_cfg.get("input"),
        };
        let Some(input_spec) = input_spec
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(output) = gen_cfg
            .get_str("output")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        // `None`: the path escapes the repository root.
        let (Some(schema_rel), Some(output_resolved)) = (
            resolve_rel(&config_dir, input_spec),
            resolve_rel(&config_dir, output),
        ) else {
            continue;
        };
        let output_rel = output_resolved.trim_end_matches('/').to_string();
        if !contained_path(repo_root, &schema_rel).is_some_and(|path| path.is_file()) {
            continue;
        }
        if !contained_path(repo_root, &output_rel).is_some_and(|path| path.exists()) {
            continue;
        }

        let package_rel = package_root_for_output(repo_root, &output_rel);
        result.ensure_file_node(config_rel, "json");
        result.ensure_file_node(&schema_rel, schema_language(&schema_rel));
        result.ensure_file_node(&package_rel, package_language(&package_rel));

        let mut extra = Bridge {
            relationship_role: "generates_code",
            bridge_kind: "generated_code",
            evidence_kind: "manifest",
            evidence_source: "openapitools.generator-cli.generators",
            source_language: schema_language(&schema_rel),
            target_language: package_language(&package_rel),
            confidence: Confidence::Exact,
        }
        .extra();
        let generator_name = match gen_cfg.get("generatorName") {
            Some(value) if value.is_truthy() => value.py_str(),
            _ => gen_name.to_string(),
        };
        extra.insert("manifest_kind".to_string(), Value::from("openapitools"));
        extra.insert("generator_name".to_string(), Value::from(generator_name));
        extra.insert("generator_key".to_string(), Value::from(gen_name));
        extra.insert("output_path".to_string(), Value::from(output_rel.clone()));
        result.push_edge(&schema_rel, &package_rel, config_rel, extra);
        add_generated_root(generated_roots, &package_rel);
    }
}

fn add_generated_root(generated_roots: &mut Vec<String>, package_rel: &str) {
    let root = package_rel
        .strip_suffix("/package.json")
        .unwrap_or(package_rel)
        .to_string();
    if !generated_roots.contains(&root) {
        generated_roots.push(root);
    }
}

pub(super) fn extract_package_json_generator_scripts(
    repo_root: &Path,
    package_rel: &str,
    result: &mut ManifestBridgeResult,
    generated_roots: &mut Vec<String>,
) {
    let Some(data) = load_json(&repo_root.join(package_rel)) else {
        return;
    };
    let Some(scripts) = data.get_object("scripts") else {
        return;
    };
    let package_dir = parent_dir(package_rel);
    for (script_name, script) in scripts.iter() {
        let Some(script) = script.as_str() else {
            continue;
        };
        let Some(found) = OPENAPI_GENERATOR_SCRIPT_RE.captures(script) else {
            continue;
        };
        let args = &found["args"];
        let (Some(input), Some(output)) =
            (CLI_INPUT_RE.captures(args), CLI_OUTPUT_RE.captures(args))
        else {
            continue;
        };
        let (Some(schema_rel), Some(output_resolved)) = (
            resolve_rel(&package_dir, strip_quotes(&input["path"])),
            resolve_rel(&package_dir, strip_quotes(&output["path"])),
        ) else {
            continue;
        };
        let output_rel = output_resolved.trim_end_matches('/').to_string();
        let schema_ok = contained_path(repo_root, &schema_rel).is_some_and(|path| path.is_file());
        let output_ok = contained_path(repo_root, &output_rel).is_some_and(|path| path.exists());
        if !schema_ok || !output_ok {
            continue;
        }

        let package_out = package_root_for_output(repo_root, &output_rel);
        // Avoid duplicating an equivalent openapitools edge.
        if result.edges.iter().any(|edge| {
            edge.source == schema_rel && edge.target == package_out && edge.kind == "CROSS_ARTIFACT"
        }) {
            continue;
        }

        result.ensure_file_node(package_rel, "json");
        result.ensure_file_node(&schema_rel, schema_language(&schema_rel));
        result.ensure_file_node(&package_out, package_language(&package_out));

        let evidence_source = format!("package.json.scripts.{script_name}");
        let mut extra = Bridge {
            relationship_role: "generates_code",
            bridge_kind: "generated_code",
            evidence_kind: "manifest",
            evidence_source: &evidence_source,
            source_language: schema_language(&schema_rel),
            target_language: package_language(&package_out),
            confidence: Confidence::Exact,
        }
        .extra();
        extra.insert(
            "manifest_kind".to_string(),
            Value::from("package_json_script"),
        );
        extra.insert("output_path".to_string(), Value::from(output_rel.clone()));
        result.push_edge(&schema_rel, &package_out, package_rel, extra);
        add_generated_root(generated_roots, &package_out);
    }
}

/// npm package name -> generated package root. Roots are read in sorted
/// order, so a name two generated packages share maps to the later one.
pub(super) fn index_generated_package_names(
    repo_root: &Path,
    generated_roots: &[String],
) -> Vec<(String, String)> {
    let mut roots: Vec<&String> = generated_roots.iter().collect();
    roots.sort();
    let mut by_name: Vec<(String, String)> = Vec::new();
    for package_root in roots {
        let root = package_root.trim_end_matches('/');
        let Some(pkg_json) = contained_path(repo_root, &format!("{root}/package.json")) else {
            continue;
        };
        if !pkg_json.is_file() {
            continue;
        }
        let Some(data) = load_json(&pkg_json).filter(|data| !data.is_empty()) else {
            continue;
        };
        let Some(name) = data
            .get_str("name")
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        match by_name.iter_mut().find(|(known, _)| known == name) {
            Some(slot) => slot.1 = root.to_string(),
            None => by_name.push((name.to_string(), root.to_string())),
        }
    }
    by_name
}

pub(super) fn extract_generated_client_consumers(
    repo_root: &Path,
    package_rel: &str,
    generated_by_name: &[(String, String)],
    result: &mut ManifestBridgeResult,
) {
    let Some(data) = load_json(&repo_root.join(package_rel)) else {
        return;
    };
    let consumer_name = data.get_str("name").filter(|name| !name.is_empty());
    let has_dependency = |dep_name: &str| {
        [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ]
        .into_iter()
        .filter_map(|key| data.get_object(key))
        .any(|section| section.contains_key(dep_name))
    };
    let consumer_root = parent_dir(package_rel);

    for (dep_name, generated_root) in generated_by_name {
        if !has_dependency(dep_name) {
            continue;
        }
        let gen_root = generated_root.trim_end_matches('/');
        if consumer_root.trim_end_matches('/') == gen_root {
            continue;
        }
        if package_rel.starts_with(&format!("{gen_root}/")) {
            continue;
        }

        let gen_pkg = format!("{gen_root}/package.json");
        let generated_target =
            if contained_path(repo_root, &gen_pkg).is_some_and(|path| path.is_file()) {
                gen_pkg
            } else {
                gen_root.to_string()
            };

        result.ensure_file_node(package_rel, "json");
        result.ensure_file_node(&generated_target, "json");

        let mut extra = Bridge {
            relationship_role: "binds_generated_client",
            bridge_kind: "generated_code",
            evidence_kind: "manifest",
            evidence_source: "package.json.dependencies",
            source_language: "javascript",
            target_language: "javascript",
            confidence: Confidence::Exact,
        }
        .extra();
        extra.insert(
            "manifest_kind".to_string(),
            Value::from("generated_client_dependency"),
        );
        extra.insert("dependency_name".to_string(), Value::from(dep_name.clone()));
        if let Some(consumer_name) = consumer_name {
            extra.insert(
                "consumer_package_name".to_string(),
                Value::from(consumer_name),
            );
        }
        result.push_edge(package_rel, &generated_target, package_rel, extra);
    }
}

/// Prefer a package.json under the generator output when present.
fn package_root_for_output(repo_root: &Path, output_rel: &str) -> String {
    let output_rel = output_rel.trim_end_matches('/');
    let direct = format!("{output_rel}/package.json");
    if contained_path(repo_root, &direct).is_some_and(|path| path.is_file()) {
        return direct;
    }
    output_rel.to_string()
}

fn schema_language(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        "yaml"
    } else if lower.ends_with(".json") {
        "json"
    } else if lower.ends_with(".proto") {
        "protobuf"
    } else {
        "schema"
    }
}

fn package_language(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with("package.json") || lower.ends_with(".ts") || lower.ends_with(".js") {
        "javascript"
    } else if lower.ends_with(".py") || lower.contains("python") {
        "python"
    } else {
        "javascript"
    }
}
