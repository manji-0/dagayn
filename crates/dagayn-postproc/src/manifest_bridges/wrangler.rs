//! `wrangler.toml` / `wrangler.json` / `wrangler.jsonc` -> the Worker's
//! entry module (`main`), which `SELF.fetch` of `cloudflare:test` runs.

use std::path::Path;

use super::data::{load_toml, toml_str};
use super::text::{contained_path, read_text_strict, resolve_rel};
use super::{Bridge, Confidence, ManifestBridgeResult};

pub(super) const WRANGLER_NAMES: &[&str] = &["wrangler.toml", "wrangler.json", "wrangler.jsonc"];

pub(super) fn extract_worker_entry(
    repo_root: &Path,
    config_rel: &str,
    result: &mut ManifestBridgeResult,
) {
    let Some(path) = contained_path(repo_root, config_rel) else {
        return;
    };
    let main = if config_rel.ends_with(".toml") {
        load_toml(&path).and_then(|table| toml_str(&table, "main").map(str::to_string))
    } else {
        read_text_strict(&path)
            .and_then(|text| {
                serde_json::from_str::<serde_json::Value>(&dagayn_parser::strip_jsonc_comments(
                    &text,
                ))
                .ok()
            })
            .and_then(|config| config.get("main")?.as_str().map(str::to_string))
    };
    let config_dir = config_rel.rsplit_once('/').map_or("", |(dir, _)| dir);
    let Some(main_rel) = main.and_then(|main| resolve_rel(config_dir, &main)) else {
        return;
    };
    if !contained_path(repo_root, &main_rel).is_some_and(|path| path.is_file()) {
        return;
    }
    let language = if config_rel.ends_with(".toml") {
        "toml"
    } else {
        "json"
    };
    result.ensure_file_node(config_rel, language);
    let mut extra = Bridge {
        relationship_role: "worker_entry",
        bridge_kind: "build_config",
        evidence_kind: "manifest",
        evidence_source: "main",
        source_language: language,
        target_language: "javascript",
        confidence: Confidence::Exact,
    }
    .extra();
    extra.insert("manifest_kind".to_string(), "wrangler".into());
    result.push_edge(config_rel, &main_rel, config_rel, extra);
}
