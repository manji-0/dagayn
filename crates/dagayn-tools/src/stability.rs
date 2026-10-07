//! `unstable_dependency`: a declared unit that depends on a less stable one
//! (the Stable Dependencies Principle), with the SAP position of the unit it
//! depends on as evidence (docs/plans/STABILITY-FINDING-TARGET.md).
//!
//! `architecture_analysis_tool(mode="overview")` lists every such dependency;
//! `review_tool(mode="changes")` lists the ones the change introduced: every
//! edge behind the dependency sits on a line the diff touches, and the base
//! version of those files did not name the unit depended on.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::{Value, json};

use crate::architecture::{Artifact, Profile, ScopeGraph, Snapshot, View, sap_metrics};
use crate::changes::Ranges;
use crate::suggestions::round_to;

/// The instability gap that makes a dependency a finding: the default of
/// `architecture_analysis_tool(mode="sdp_violations")`, so the mode and the
/// finding agree.
pub(crate) const MIN_DELTA: f64 = 0.1;
/// Edges a finding names as the places that make the dependency.
const MAX_SITES: usize = 3;

/// `(file, line, qualified name of the depending node)`, repo-relative.
type Site = (String, i64, String);

/// One dependency of `source` on the less stable `target`.
pub(crate) struct UnstableDependency {
    source: String,
    target: String,
    delta: f64,
    metrics: [(i64, i64, f64); 2],
    sites: Vec<Site>,
    target_sap: Option<Value>,
}

/// The declared-unit view the SDP metric modes use by default.
fn unit_view(root: &Path, snapshot: &Snapshot) -> View {
    View {
        file_scopes: false,
        artifact: Artifact::Code,
        profile: Profile::StrictStatic,
        units: Some(snapshot.unit_scopes(root)),
    }
}

/// A scope of production code: not tests, not fixtures.
fn is_production_scope(scope: &str) -> bool {
    let probe = format!("{scope}/probe.rs");
    !crate::findings::is_test_path(&probe) && !scope.split('/').any(|part| part == "fixtures")
}

fn relative(root: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string())
}

/// Every production unit that depends on a less stable one, largest gap
/// first.
pub(crate) fn unstable_dependencies(root: &Path, snapshot: &Snapshot) -> Vec<UnstableDependency> {
    let view = unit_view(root, snapshot);
    let edges = snapshot.dependency_edges(&view);
    let pairs: Vec<(String, String)> = edges
        .iter()
        .map(|(source, target, _)| (source.clone(), target.clone()))
        .collect();
    let graph = ScopeGraph::new(&pairs);
    let metrics: HashMap<String, (i64, i64, f64)> = graph
        .sdp_metrics()
        .into_iter()
        .map(|(name, ca, ce, instability)| (name, (ca, ce, instability)))
        .collect();
    let violations = graph.sdp_violations(MIN_DELTA, view.profile);
    if violations.is_empty() {
        return Vec::new();
    }
    let mut sites: BTreeMap<(&str, &str), Vec<Site>> = BTreeMap::new();
    for (source, target, edge) in &edges {
        sites
            .entry((source.as_str(), target.as_str()))
            .or_default()
            .push((
                relative(root, &edge.file_path),
                edge.line,
                relative(root, &edge.source_qualified),
            ));
    }
    let sap: HashMap<String, Value> = sap_metrics(snapshot, &view, "package", None)
        .into_iter()
        .filter(|metric| metric["sap_applicable"].as_bool() == Some(true))
        .filter_map(|metric| Some((metric["scope_key"].as_str()?.to_string(), metric)))
        .collect();
    violations
        .iter()
        .filter_map(|violation| {
            let source = violation["source"].as_str()?.to_string();
            let target = violation["target"].as_str()?.to_string();
            if !is_production_scope(&source) || !is_production_scope(&target) {
                return None;
            }
            let mut found = sites
                .get(&(source.as_str(), target.as_str()))
                .cloned()
                .unwrap_or_default();
            found.sort();
            found.dedup();
            Some(UnstableDependency {
                delta: violation["delta"].as_f64().unwrap_or(0.0),
                metrics: [
                    metrics.get(&source).copied().unwrap_or((0, 0, 0.0)),
                    metrics.get(&target).copied().unwrap_or((0, 0, 0.0)),
                ],
                target_sap: sap.get(&target).map(|metric| {
                    json!({
                        "abstractness": metric["abstractness"],
                        "distance": metric["distance"],
                    })
                }),
                source,
                target,
                sites: found,
            })
        })
        .collect()
}

impl UnstableDependency {
    /// The finding, in the shape the other kinds share.
    pub(crate) fn finding(&self) -> Value {
        let [
            (source_ca, source_ce, source_i),
            (target_ca, target_ce, target_i),
        ] = self.metrics;
        let (file, line, depending) = self
            .sites
            .first()
            .cloned()
            .unwrap_or_else(|| (String::new(), 0, String::new()));
        let mut target = json!({
            "unit": self.target,
            "afferent": target_ca,
            "efferent": target_ce,
            "instability": round_to(target_i, 4),
        });
        if let Some(sap) = &self.target_sap {
            target["sap"] = sap.clone();
        }
        let mut finding = json!({
            "kind": "unstable_dependency",
            "claim": format!(
                "{} (instability {}) depends on {} (instability {}), which is less stable.",
                self.source,
                round_to(source_i, 2),
                self.target,
                round_to(target_i, 2),
            ),
            "file": file,
            "line": line,
            "targets": [self.source, self.target],
            "evidence": {
                "source": {
                    "unit": self.source,
                    "afferent": source_ca,
                    "efferent": source_ce,
                    "instability": round_to(source_i, 4),
                },
                "target": target,
                "delta": round_to(self.delta, 4),
                "sites": self
                    .sites
                    .iter()
                    .take(MAX_SITES)
                    .map(|(file, line, _)| json!({"file": file, "line": line}))
                    .collect::<Vec<_>>(),
                "sites_omitted": self.sites.len().saturating_sub(MAX_SITES),
            },
            "action": format!(
                "Depend on something at least as stable as {}: move what it needs from {} into {} or a stable unit both use, or have {} implement an interface {} owns.",
                self.source, self.target, self.source, self.target, self.source
            ),
        });
        if depending.contains("::") {
            finding["qualified_name"] = json!(depending);
        }
        finding
    }

    /// Whether the change made this dependency: every edge behind it sits on
    /// a line the diff touches, and the base version of none of those files
    /// names the unit depended on.
    pub(crate) fn introduced_by(&self, root: &Path, base: &str, ranges: &Ranges) -> bool {
        if self.sites.is_empty() {
            return false;
        }
        let touched = |file: &str, line: i64| {
            ranges.get(file).is_some_and(|spans| {
                spans
                    .iter()
                    .any(|(start, end)| *start <= line && line <= *end)
            })
        };
        if !self
            .sites
            .iter()
            .all(|(file, line, _)| touched(file, *line))
        {
            return false;
        }
        let names = unit_names(&self.target);
        let mut files: Vec<&str> = self
            .sites
            .iter()
            .map(|(file, _, _)| file.as_str())
            .collect();
        files.dedup();
        files.iter().all(|file| {
            crate::base_symbols::base_source(root, base, file).is_none_or(|bytes| {
                let text = String::from_utf8_lossy(&bytes);
                !names.iter().any(|name| mentions(&text, name))
            })
        })
    }
}

/// The names code uses for a unit labelled `label`: `name (path)` labels
/// give the name, and a Rust crate is `use`d with `_` for `-`.
fn unit_names(label: &str) -> Vec<String> {
    let name = label.split(" (").next().unwrap_or(label);
    let last = name.rsplit('/').next().unwrap_or(name);
    let mut names = vec![last.to_string()];
    if last.contains('-') {
        names.push(last.replace('-', "_"));
    }
    names
}

/// `name` as a whole word of `text`.
fn mentions(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_is_mentioned_only_as_a_whole_word() {
        assert!(mentions("use plugins::plugin;", "plugins"));
        assert!(mentions("from plugins import x", "plugins"));
        assert!(!mentions("use my_plugins::x;", "plugins"));
        assert!(!mentions("let pluginsx = 1;", "plugins"));
        assert_eq!(unit_names("dagayn-graph"), ["dagayn-graph", "dagayn_graph"]);
        assert_eq!(unit_names("dagayn (dagayn)"), ["dagayn"]);
        assert_eq!(unit_names("tools/web"), ["web"]);
    }
}
