//! `review_tool` (`dagayn.tools.review_dispatcher.review_func`) for
//! `mode="affected_flows"` (`get_affected_flows_func`) and `mode="impact"`
//! (`dagayn.tools.query.get_impact_radius`). `changes` and `context` stay
//! Python's, as do jj, svn, and a ref Python rejects.

use std::path::{Component, Path, PathBuf};

use dagayn_build::{ChangeSources, change_file_sources, staged_and_unstaged};
use dagayn_graph::{GraphStore, is_low_confidence_unresolved_markdown_code_span};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::hints::{SessionState, generate_hints, session};
use crate::query::{edge_dict, node_dict};
use crate::{Args, Context, OpenGraph, Ordered, Payload, explicit_repo, open_graph};

const DECLARED: &[&str] = &[
    "mode",
    "base",
    "changed_files",
    "include_source",
    "max_depth",
    "max_nodes",
    "max_lines_per_file",
    "repo_root",
    "detail_level",
];

/// `get_impact_radius`'s `apply_output_budget` budget, in tokens.
const IMPACT_BUDGET: usize = 8000;

/// `review_tool`'s arguments once fastmcp and `parse_review_request` accept
/// them.
struct Request<'a> {
    base: &'a str,
    changed_files: Option<Vec<String>>,
    max_depth: i64,
    max_nodes: i64,
    detail_level: &'a str,
}

impl<'a> Request<'a> {
    fn parse(args: &Args<'a>, arguments: &'a Map<String, Value>) -> Option<Self> {
        let base = match arguments.get("base") {
            None => "HEAD~1",
            Some(Value::String(base)) => base.as_str(),
            Some(_) => return None,
        };
        let changed_files = match arguments.get("changed_files") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()?,
            ),
            Some(_) => return None,
        };
        if !matches!(
            arguments.get("include_source"),
            None | Some(Value::Null | Value::Bool(_))
        ) {
            return None;
        }
        args.integer("max_lines_per_file", 200)?;
        let detail_level = match arguments.get("detail_level") {
            None => "standard",
            Some(Value::String(level))
                if matches!(level.as_str(), "minimal" | "standard" | "verbose") =>
            {
                level.as_str()
            }
            Some(_) => return None,
        };
        Some(Self {
            base,
            changed_files,
            max_depth: args.integer("max_depth", 2)?,
            max_nodes: args.integer("max_nodes", 50)?,
            detail_level,
        })
    }
}

pub(crate) fn review(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let mode = match arguments.get("mode") {
        Some(Value::String(mode)) if matches!(mode.as_str(), "affected_flows" | "impact") => {
            mode.as_str()
        }
        _ => return None,
    };
    let request = Request::parse(&args, arguments)?;
    let runtime = context.runtime.clone()?;
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let stats = graph.store.get_stats().ok()?;
    let answerability = Answerability::recorded(&graph.store, &stats)?;
    let exposed = |tool: &str| {
        context
            .allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tool))
    };
    let mut hint_session = session();
    let mut review = Review {
        graph: &graph,
        answerability: &answerability,
        session: &mut hint_session,
        exposed: &exposed,
    };
    let (subtool, out) = match mode {
        "affected_flows" => ("get_affected_flows_func", review.affected_flows(&request)?),
        _ => ("get_impact_radius", review.impact(&request)?),
    };
    // `_with_dispatch_metadata`: `attach_answerability` adds `_runtime`, then
    // the review hints are built (and recorded) even when the subtool's stay.
    let mut seen = out.value();
    if let Some(object) = seen.as_object_mut() {
        object.insert("mode".to_string(), json!(mode));
        object.insert("called_subtool".to_string(), json!(subtool));
        object.insert("_runtime".to_string(), runtime.clone());
    }
    let review_hints = generate_hints("review", &seen, review.session, review.exposed);
    let has_hints = out.value().get("_hints").is_some();
    // `seal_dispatcher_ok`: the envelope's fields first, then the subtool's
    // in its order; the server's `attach_repo_context` last.
    let mut sealed = Ordered::default()
        .put("status", "ok")
        .put("mode", mode)
        .put("called_subtool", subtool)
        .put(
            "summary",
            seen.get("summary").cloned().unwrap_or(Value::Null),
        );
    for (key, value) in out.into_entries() {
        if !matches!(key.as_str(), "status" | "summary") {
            sealed = sealed.put(&key, value);
        }
    }
    sealed = sealed.put("_runtime", runtime);
    if !has_hints {
        sealed = sealed.put("_hints", review_hints);
    }
    Some(sealed.put("_repo", graph.repo_context()).into_payload())
}

struct Review<'a, 's> {
    graph: &'a OpenGraph,
    answerability: &'a Answerability,
    session: &'s mut SessionState,
    exposed: &'a dyn Fn(&str) -> bool,
}

impl Review<'_, '_> {
    fn store(&self) -> &GraphStore {
        &self.graph.store
    }

    fn root(&self) -> &Path {
        &self.graph.root
    }

    /// `get_affected_flows_func`.
    fn affected_flows(&mut self, request: &Request) -> Option<Ordered> {
        let (changed_files, sources) = match &request.changed_files {
            Some(files) => (
                files.clone(),
                Some(json!({"files": files, "explicit": files})),
            ),
            None => {
                let sources = change_file_sources(self.root(), request.base)?;
                if sources.files.is_empty() {
                    let files = staged_and_unstaged(self.root())?;
                    let sources = json!({"files": files, "worktree": files});
                    (files, Some(sources))
                } else {
                    (sources.files.clone(), Some(sources_value(sources)))
                }
            }
        };
        if changed_files.is_empty() {
            return Some(
                Ordered::default()
                    .put("status", "ok")
                    .put("summary", "No changed files detected.")
                    .put("affected_flows", json!([]))
                    .put("total", 0)
                    .put("answerability", self.answerability.full())
                    .put("missingness", json!(self.answerability.missingness())),
            );
        }
        let absolute: Vec<String> = changed_files
            .iter()
            .map(|file| absolute_path(self.root(), file))
            .collect();
        let flows = self.store().get_affected_flows_annotated(&absolute).ok()?;
        let total = flows.len();
        let out = Ordered::default()
            .put("status", "ok")
            .put(
                "summary",
                format!(
                    "{total} flow(s) affected by changes in {} file(s)",
                    changed_files.len()
                ),
            )
            .put("changed_files", json!(changed_files))
            .put("change_file_sources", sources.unwrap_or(Value::Null))
            .put("affected_flows", Value::Array(flows))
            .put("total", total)
            .put("answerability", self.answerability.full())
            .put("missingness", json!(self.answerability.missingness()));
        let hints = generate_hints(
            "get_affected_flows",
            &out.value(),
            self.session,
            self.exposed,
        );
        Some(out.put("_hints", hints))
    }

    /// `get_impact_radius` (the tool, `dagayn.tools.query`).
    fn impact(&mut self, request: &Request) -> Option<Ordered> {
        let changed_files = match &request.changed_files {
            Some(files) => files.clone(),
            None => {
                // `get_changed_files`, then the worktree when it is empty.
                let files = change_file_sources(self.root(), request.base)?.files;
                if files.is_empty() {
                    staged_and_unstaged(self.root())?
                } else {
                    files
                }
            }
        };
        let missingness = self.answerability.missingness();
        if changed_files.is_empty() {
            return Some(
                Ordered::default()
                    .put("status", "ok")
                    .put("summary", "No changed files detected.")
                    .put("changed_nodes", json!([]))
                    .put("impacted_nodes", json!([]))
                    .put("impacted_files", json!([]))
                    .put("truncated", false)
                    .put("total_impacted", 0)
                    .put("answerability", self.answerability.full())
                    .put("missingness", json!(missingness)),
            );
        }
        let absolute: Vec<String> = changed_files
            .iter()
            .map(|file| absolute_path(self.root(), file))
            .collect();
        let radius = self
            .store()
            .get_impact_radius(&absolute, request.max_depth, request.max_nodes)
            .ok()?;
        let changed: Vec<Value> = radius.changed_nodes.iter().map(node_dict).collect();
        let impacted: Vec<Value> = radius.impacted_nodes.iter().map(node_dict).collect();
        let edges: Vec<Value> = radius
            .edges
            .iter()
            .filter(|edge| !is_low_confidence_unresolved_markdown_code_span(edge))
            .map(|edge| Value::Object(edge_dict(edge)))
            .collect();
        let bridges = &radius.bridge_transitions;
        let caveats = &radius.low_confidence_bridges;
        let matched: std::collections::HashSet<String> = radius
            .changed_nodes
            .iter()
            .filter(|node| !node.file_path.is_empty())
            .map(|node| normalized_repo_path(&node.file_path, self.root()))
            .collect();
        let unmatched: Vec<String> = changed_files
            .iter()
            .filter(|file| !matched.contains(&normalized_repo_path(file, self.root())))
            .cloned()
            .collect();

        let mut summary = vec![
            format!("Blast radius for {} changed file(s):", changed_files.len()),
            format!("  - {} nodes directly changed", changed.len()),
            format!(
                "  - {} nodes impacted (within {} hops)",
                impacted.len(),
                request.max_depth
            ),
            format!(
                "  - {} additional files affected",
                radius.impacted_files.len()
            ),
        ];
        if !unmatched.is_empty() {
            summary.push(format!(
                "  - {} of {} changed file(s) are NOT in the graph: their blast radius is unknown, not zero",
                unmatched.len(),
                changed_files.len()
            ));
        }
        if !bridges.is_empty() {
            summary.push(format!(
                "  - {} reportable cross-artifact bridge hop(s)",
                bridges.len()
            ));
        }
        if !caveats.is_empty() {
            summary.push(format!(
                "  - {} low-confidence bridge caveat(s)",
                caveats.len()
            ));
        }
        if radius.truncated {
            summary.push(format!(
                "  - Results truncated: showing {} of {} impacted nodes",
                impacted.len(),
                radius.total_impacted
            ));
        }
        let summary = summary.join("\n");

        let mut impact_missingness = missingness;
        impact_missingness.extend(caveats.iter().cloned());
        if !unmatched.is_empty() {
            impact_missingness.push(json!({
                "reason_code": "changed_files_not_in_graph",
                "severity": "high",
                "claim_effect": "impact for these files is unknown, not zero -- run dagayn update (or check the paths) before treating the change as safe",
                "details": {"unmatched_changed_files": &unmatched[..unmatched.len().min(20)]},
            }));
        }
        if !bridges.is_empty() {
            impact_missingness.push(json!({
                "reason_code": "cross_artifact_bridge_is_static_evidence",
                "severity": "low",
                "claim_effect": "bridge hops are graph-derived explainable paths, not runtime traces",
            }));
        }

        // `make_guidance_item`s, sealed: evidence and missingness as lists.
        let mut guidance = Vec::new();
        if !bridges.is_empty() {
            guidance.push(json!({
                "claim": format!(
                    "Impact crosses {} reportable cross-artifact bridge(s).",
                    bridges.len()
                ),
                "evidence": [{
                    "type": "extracted",
                    "bridge_transitions": &bridges[..bridges.len().min(5)],
                }],
                "confidence": "high",
                "missingness": [{
                    "reason_code": "cross_artifact_bridge_is_static_evidence",
                    "severity": "low",
                    "claim_effect": "follow docs_for / implementations_of / bridge edges to confirm",
                }],
                "action": "query_graph_tool pattern=\"docs_for\" -- follow contract docs; also try implementations_of / CROSS_ARTIFACT neighbors",
                "reason_codes": ["cross_artifact_bridge_impact"],
                "counts": {"bridge_transition_count": bridges.len()},
            }));
        }
        if !caveats.is_empty() {
            guidance.push(json!({
                "claim": "Low-confidence cross-artifact bridges are caveats, not hard impact.",
                "evidence": [{
                    "type": "extracted",
                    "caveat_count": caveats.len(),
                    "examples": &caveats[..caveats.len().min(3)],
                }],
                "confidence": "low",
                "missingness": &caveats[..caveats.len().min(5)],
                "action": "query_graph_tool pattern=\"docs_for\" -- verify before treating as impact",
                "reason_codes": ["low_confidence_cross_artifact_bridge"],
                "counts": {"low_confidence_bridge_count": caveats.len()},
            }));
        }

        if request.detail_level == "minimal" {
            let risk = match impacted.len() {
                count if count > 20 => "high",
                count if count > 5 => "medium",
                _ => "low",
            };
            let key_entities: Vec<Value> = impacted
                .iter()
                .take(5)
                .map(|node| node.get("name").cloned().unwrap_or(Value::Null))
                .collect();
            return Some(
                Ordered::default()
                    .put("status", "ok")
                    .put("summary", summary)
                    .put("risk", risk)
                    .put("unmatched_changed_files", json!(unmatched))
                    .put("impacted_file_count", radius.impacted_files.len())
                    .put("key_entities", key_entities)
                    .put("bridge_transition_count", bridges.len())
                    .put("truncated", radius.truncated)
                    .put("answerability", self.answerability.full())
                    .put("missingness", json!(impact_missingness))
                    .put("guidance", json!(guidance)),
            );
        }
        let out = Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("changed_files", json!(changed_files))
            .put("unmatched_changed_files", json!(unmatched))
            .put("changed_nodes", json!(changed))
            .put("impacted_nodes", json!(impacted))
            .put("impacted_files", json!(radius.impacted_files))
            .put("edges", json!(edges))
            .put("bridge_transitions", json!(bridges))
            .put("low_confidence_bridges", json!(caveats))
            .put("truncated", radius.truncated)
            .put("total_impacted", radius.total_impacted)
            .put("answerability", self.answerability.full())
            .put("missingness", json!(impact_missingness))
            .put("guidance", json!(guidance));
        Some(out.apply_output_budget(
            IMPACT_BUDGET,
            &[
                "changed_files",
                "impacted_files",
                "changed_nodes",
                "impacted_nodes",
                "bridge_transitions",
                "edges",
                "low_confidence_bridges",
            ],
        ))
    }
}

/// `str(root / file)`: pathlib drops `.` components, repeated and trailing
/// separators.
fn absolute_path(root: &Path, file: &str) -> String {
    root.join(file)
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// `_normalized_repo_path`: repo-relative when under `root`, as pathlib
/// spells it.
fn normalized_repo_path(value: &str, root: &Path) -> String {
    let path = Path::new(value);
    let path = if path.is_absolute() {
        path.strip_prefix(root).unwrap_or(path)
    } else {
        path
    };
    let parts: PathBuf = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    let text = parts.to_string_lossy().into_owned();
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

fn sources_value(sources: ChangeSources) -> Value {
    json!({
        "files": sources.files,
        "base_diff": sources.base_diff,
        "worktree": sources.worktree,
        "staged": sources.staged,
        "unstaged": sources.unstaged,
        "untracked": sources.untracked,
    })
}
