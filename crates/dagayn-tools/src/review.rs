//! `review_tool` (`dagayn.tools.review_dispatcher.review_func`) for every
//! mode: `changes` (`detect_changes_func`), `context`
//! (`get_review_context`), `affected_flows` (`get_affected_flows_func`), and
//! `impact` (`dagayn.tools.query.get_impact_radius`), in a git checkout, a
//! jj workspace, or an SVN working copy. A jj working copy jj cannot read is
//! Python's to report.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use dagayn_build::{ChangeSources, change_file_sources, staged_and_unstaged};
use dagayn_graph::{
    GraphNode, GraphStore, ImpactRadius, is_low_confidence_unresolved_markdown_code_span,
};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::changes::{DiffParse, analyze_changes, parse_diff};
use crate::coverage::splitlines;
use crate::hints::{generate_hints, session};
use crate::query::{edge_dict, node_dict};
use crate::refactor::python_repr;
use crate::review_summary::change_analysis_summary;
use crate::{Args, Context, OpenGraph, Ordered, Payload, open_graph, resolve_repo};

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
/// `detect_changes_func`'s `apply_output_budget` budget, in tokens.
const CHANGES_BUDGET: usize = 8000;
/// `get_review_context`'s budget, its graph caps, its source byte cap, and
/// its `max_lines_per_file` ceiling.
const CONTEXT_BUDGET: usize = 8000;
const MAX_GRAPH_ENTRIES: usize = 300;
const MAX_SNIPPET_BYTES: usize = 120_000;
const MAX_LINES_PER_FILE_CEILING: i64 = 2000;

/// `review_tool`'s arguments once fastmcp and `parse_review_request` accept
/// them.
struct Request<'a> {
    base: &'a str,
    changed_files: Option<Vec<String>>,
    max_depth: i64,
    max_nodes: i64,
    detail_level: &'a str,
    include_source: Option<bool>,
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
        let include_source = match arguments.get("include_source") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(flag)) => Some(*flag),
            Some(_) => return None,
        };
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
            include_source,
        })
    }
}

pub(crate) fn review(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let mode = match arguments.get("mode") {
        None => "changes",
        Some(Value::String(mode))
            if matches!(
                mode.as_str(),
                "changes" | "context" | "affected_flows" | "impact"
            ) =>
        {
            mode.as_str()
        }
        _ => return None,
    };
    let request = Request::parse(&args, arguments)?;
    let runtime = context.runtime.clone()?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let answerability = graph.answerability()?;
    let exposed = |tool: &str| context.exposes(tool);
    let review = Review {
        graph: &graph,
        answerability: &answerability,
        exposed: &exposed,
    };
    let (subtool, out) = match mode {
        "changes" => match review.changes(&request)? {
            Ok(out) => ("detect_changes_func", out),
            Err(error) => {
                // `with_dispatch_metadata` seals an error without hints.
                return Some(
                    Ordered::default()
                        .put("status", "error")
                        .put("summary", error.message.as_str())
                        .put("error", error.message.as_str())
                        .put("mode", mode)
                        .put("called_subtool", "detect_changes_func")
                        .put("base", request.base)
                        .put("diff_parse_status", "base_unresolved")
                        .put("answerability", answerability.full())
                        .put("missingness", json!(error.missingness))
                        .put("_runtime", runtime)
                        .put("_repo", graph.repo_context())
                        .into_payload(),
                );
            }
        },
        "context" => ("get_review_context", review.context(&request, &args)?),
        "affected_flows" => ("get_affected_flows_func", review.affected_flows(&request)?),
        _ => ("get_impact_radius", review.impact(&request)?),
    };
    Some(crate::seal_dispatch(
        out,
        crate::Dispatch {
            mode,
            subtool,
            hints_tool: "review",
            runtime,
            trailing: Vec::new(),
            repo: graph.repo_context(),
        },
        &exposed,
    ))
}

/// `detect_changes_func`'s `_error_response` for a diff base that does not
/// resolve.
struct BaseUnresolved {
    message: String,
    missingness: Vec<Value>,
}

struct Review<'a> {
    graph: &'a OpenGraph,
    answerability: &'a Answerability,
    exposed: &'a dyn Fn(&str) -> bool,
}

impl Review<'_> {
    /// `generate_hints(tool, result, get_session())`, holding the session
    /// only while it records.
    fn hints(&self, tool: &str, result: &Value) -> Value {
        generate_hints(tool, result, &mut session(), self.exposed)
    }

    fn store(&self) -> &GraphStore {
        &self.graph.store
    }

    fn root(&self) -> &Path {
        &self.graph.root
    }

    /// The changed files and their sources, as `detect_changes_func` and
    /// `get_affected_flows_func` detect them.
    fn changed_files(&self, request: &Request) -> Option<(Vec<String>, Value)> {
        Some(match &request.changed_files {
            Some(files) => (files.clone(), json!({"files": files, "explicit": files})),
            None => {
                let sources = change_file_sources(self.root(), request.base)?;
                if sources.files.is_empty() {
                    let files = staged_and_unstaged(self.root())?;
                    let sources = json!({"files": files, "worktree": files});
                    (files, sources)
                } else {
                    (sources.files.clone(), sources_value(sources))
                }
            }
        })
    }

    /// `detect_changes_func` at `standard` and `minimal` detail, without
    /// source snippets; `Err` for the error it reports when `base` does not
    /// resolve.
    fn changes(&self, request: &Request) -> Option<Result<Ordered, BaseUnresolved>> {
        if request.include_source == Some(true) || request.detail_level == "verbose" {
            return None;
        }
        let (changed_files, sources) = self.changed_files(request)?;
        if changed_files.is_empty() {
            return Some(Ok(Ordered::default()
                .put("status", "ok")
                .put("summary", "No changed files detected.")
                .put("risk_score", 0.0)
                .put("changed_functions", json!([]))
                .put("affected_flows", json!([]))
                .put("test_gaps", json!([]))
                .put("review_priorities", json!([]))
                .put("answerability", self.answerability.full())
                .put("missingness", json!(self.answerability.missingness()))));
        }
        let ranges = match parse_diff(self.root(), request.base) {
            DiffParse::Ranges(ranges) => ranges,
            DiffParse::BaseUnresolved => {
                return self.base_unresolved(request.base).map(Err);
            }
        };
        let analysis = analyze_changes(
            self.store(),
            self.root(),
            request.base,
            &changed_files,
            &ranges,
        )?;
        let absolute: Vec<String> = changed_files
            .iter()
            .map(|file| absolute_path(self.root(), file))
            .collect();
        let impact = self
            .store()
            .get_impact_radius(&absolute, request.max_depth, 500)
            .ok()?;
        let summary = change_analysis_summary(self.store(), &analysis, &impact, &changed_files)?;
        let full_guidance = summary["guidance"].clone();

        let out = if request.detail_level == "minimal" {
            let field = |key: &str| summary.get(key).cloned().unwrap_or(Value::Null);
            let first = |key: &str, count: usize| -> Value {
                json!(
                    summary[key]
                        .as_array()
                        .map(|items| items.iter().take(count).cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                )
            };
            let delta = &summary["architecture_delta"];
            let baseline = delta.get("baseline_comparison")?.clone();
            let priorities: Vec<Value> = analysis
                .get("review_priorities")
                .as_array()
                .into_iter()
                .flatten()
                .take(3)
                .map(|p| {
                    p.get("name")
                        .cloned()
                        .unwrap_or_else(|| p.get("qualified_name").cloned().unwrap_or(json!("")))
                })
                .collect();
            let semantics = match analysis.get("score_semantics") {
                value if value.as_object().is_some_and(|m| !m.is_empty()) => value.clone(),
                _ => field("score_semantics"),
            };
            Ordered::default()
                .put("status", "ok")
                .put("summary", analysis.get("summary").clone())
                .put("risk_score", analysis.get("risk_score").clone())
                .put(
                    "review_priority_score",
                    analysis.get("review_priority_score").clone(),
                )
                .put("score_semantics", semantics)
                .put("risk_level", field("risk_level"))
                .put("reason_codes", field("reason_codes"))
                .put("changed_file_count", changed_files.len())
                .put("change_file_sources", sources)
                .put(
                    "change_entity_summary",
                    analysis.get("change_entity_summary").clone(),
                )
                .put("changed_node_count", field("changed_node_count"))
                .put("impacted_node_count", field("impacted_node_count"))
                .put("impacted_file_count", field("impacted_file_count"))
                .put(
                    "test_gap_count",
                    analysis.get("test_gaps").as_array().map_or(0, Vec::len),
                )
                .put(
                    "test_gap_evidence",
                    analysis.get("test_gap_evidence").clone(),
                )
                .put("test_gap_ranking", field("test_gap_ranking"))
                .put("signal_quality", field("signal_quality"))
                .put("recommended_tests", first("recommended_tests", 5))
                .put("affected_flow_rankings", first("affected_flow_rankings", 5))
                .put(
                    "documentation_update_candidates",
                    first("documentation_update_candidates", 5),
                )
                .put("stability_contracts", first("stability_contracts", 5))
                .put("guidance", first("guidance", 3))
                .put(
                    "architecture_delta",
                    json!({
                        "mode": delta["mode"],
                        "changed_scopes": delta["changed_scopes"],
                        "counts": delta["counts"],
                        "baseline_comparison": baseline,
                    }),
                )
                .put("review_priorities", priorities)
                .put("next_drill_downs", field("next_drill_downs"))
                .put("answerability", self.answerability.full())
                .put("missingness", json!(self.answerability.missingness()))
        } else {
            let mut out = Ordered::default()
                .put("status", "ok")
                .put("changed_files", json!(changed_files))
                .put("change_file_sources", sources);
            for (key, value) in &analysis.fields {
                out = out.put(key, value.clone());
            }
            out.put("analysis_summary", summary)
                .put("answerability", self.answerability.full())
                .put("missingness", json!(self.answerability.missingness()))
                .apply_output_budget(
                    CHANGES_BUDGET,
                    &[
                        "analysis_summary.recommended_tests",
                        "analysis_summary.affected_flow_rankings",
                        "analysis_summary.documentation_update_candidates",
                        "analysis_summary.stability_contracts",
                        "analysis_summary.guidance",
                        "review_priorities",
                        "affected_flows",
                        "test_gaps",
                        "changed_functions",
                    ],
                )
        };
        // The hints read the guidance as the result holds it after the trim
        // (`standard`), or the summary's whole list (`minimal`).
        let value = out.value();
        let guidance_list = match value.get("analysis_summary") {
            Some(trimmed) => trimmed["guidance"].clone(),
            None => full_guidance,
        };
        let mut hints =
            guidance_actions_to_hints(guidance_list.as_array().map(Vec::as_slice).unwrap_or(&[]));
        if hints["next_steps"].as_array().is_none_or(Vec::is_empty) {
            hints = self.hints("detect_changes", &value);
        }
        Some(Ok(out.put("_hints", hints)))
    }

    /// The error `detect_changes_func` reports for a `base` the diff cannot
    /// resolve; `None` for a base `repr` would escape beyond ASCII.
    fn base_unresolved(&self, base: &str) -> Option<BaseUnresolved> {
        if !base.is_ascii() {
            return None;
        }
        let message = format!(
            "Could not resolve the diff base {} in {}. Pass a reachable ref (the default HEAD~1 \
             does not exist in a single-commit repository, and a rebase or gc can make a \
             recorded sha unreachable).",
            python_repr(base),
            self.root().display()
        );
        let mut missingness = self.answerability.missingness();
        missingness.push(json!({
            "reason_code": "diff_base_unreachable",
            "severity": "high",
            "claim_effect": "no diff could be computed, so nothing here describes what actually changed",
        }));
        Some(BaseUnresolved {
            message,
            missingness,
        })
    }

    /// `get_review_context`.
    fn context(&self, request: &Request, args: &Args) -> Option<Ordered> {
        let include_source = request.include_source.unwrap_or(true);
        let max_lines = args
            .integer("max_lines_per_file", 200)?
            .clamp(1, MAX_LINES_PER_FILE_CEILING) as usize;
        let (changed_files, sources) = self.changed_files(request)?;
        if changed_files.is_empty() {
            return Some(
                Ordered::default()
                    .put("status", "ok")
                    .put("summary", "No changes detected. Nothing to review.")
                    .put("context", json!({}))
                    .put("answerability", self.answerability.full())
                    .put("missingness", json!(self.answerability.missingness())),
            );
        }
        let absolute: Vec<String> = changed_files
            .iter()
            .map(|file| absolute_path(self.root(), file))
            .collect();
        let radius = self
            .store()
            .get_impact_radius(&absolute, request.max_depth, 500)
            .ok()?;
        let changed_funcs: Vec<&GraphNode> = radius
            .changed_nodes
            .iter()
            .filter(|node| node.kind == "Function")
            .collect();
        let tested: HashSet<&str> = radius
            .edges
            .iter()
            .filter(|edge| edge.kind == "TESTED_BY")
            .map(|edge| edge.source_qualified.as_str())
            .collect();

        if request.detail_level == "minimal" {
            let impacted = radius.impacted_nodes.len();
            let risk = match impacted {
                count if count > 20 => "high",
                count if count > 5 => "medium",
                _ => "low",
            };
            let key_entities: Vec<String> = radius
                .changed_nodes
                .iter()
                .take(5)
                .map(|node| relative_qualified_name(&node.qualified_name, self.root()))
                .collect();
            let gaps = changed_funcs
                .iter()
                .filter(|f| !f.is_test && !tested.contains(f.qualified_name.as_str()))
                .count();
            let summary = [
                format!(
                    "Review context for {} changed file(s):",
                    changed_files.len()
                ),
                format!("  - Risk: {risk}"),
                format!(
                    "  - {impacted} impacted nodes in {} files",
                    radius.impacted_files.len()
                ),
            ]
            .join("\n");
            return Some(
                Ordered::default()
                    .put("status", "ok")
                    .put("summary", summary)
                    .put("risk", risk)
                    .put("changed_file_count", changed_files.len())
                    .put("change_file_sources", sources)
                    .put("impacted_file_count", radius.impacted_files.len())
                    .put("key_entities", json!(key_entities))
                    .put("test_gaps", gaps)
                    .put("answerability", self.answerability.full())
                    .put("missingness", json!(self.answerability.missingness()))
                    .put(
                        "next_tool_suggestions",
                        json!([
                            "review_tool mode=\"changes\"",
                            "review_tool mode=\"affected_flows\"",
                            "review_tool mode=\"impact\"",
                        ]),
                    ),
            );
        }

        let changed: Vec<Value> = radius.changed_nodes.iter().map(node_dict).collect();
        let impacted: Vec<Value> = radius.impacted_nodes.iter().map(node_dict).collect();
        let edges: Vec<Value> = radius
            .edges
            .iter()
            .filter(|edge| !is_low_confidence_unresolved_markdown_code_span(edge))
            .map(|edge| Value::Object(edge_dict(edge)))
            .collect();
        let mut graph_truncation = Map::new();
        for (field, values) in [
            ("changed_nodes", &changed),
            ("impacted_nodes", &impacted),
            ("edges", &edges),
        ] {
            if values.len() > MAX_GRAPH_ENTRIES {
                graph_truncation.insert(
                    field.to_string(),
                    json!({"kept": MAX_GRAPH_ENTRIES, "total": values.len()}),
                );
            }
        }
        let first = |values: &[Value]| {
            values
                .iter()
                .take(MAX_GRAPH_ENTRIES)
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut context = Ordered::default()
            .put("changed_files", json!(changed_files))
            .put("change_file_sources", sources)
            .put("impacted_files", json!(radius.impacted_files))
            .put(
                "graph",
                json!({
                    "changed_nodes": first(&changed),
                    "impacted_nodes": first(&impacted),
                    "edges": first(&edges),
                }),
            );
        let mut snippets: Vec<(String, String)> = Vec::new();
        if include_source {
            let mut out_of_repo = Vec::new();
            for rel in &changed_files {
                let Some(full) = resolve_contained_path(rel, self.root()) else {
                    out_of_repo.push(rel.clone());
                    continue;
                };
                if !full.is_file() {
                    continue;
                }
                let text = match std::fs::read(&full) {
                    Ok(bytes) => {
                        let decoded = String::from_utf8_lossy(&bytes).into_owned();
                        let lines = splitlines(&decoded);
                        if lines.len() > max_lines {
                            relevant_lines(&lines, &radius.changed_nodes, rel)
                        } else {
                            numbered(&lines, 0, lines.len())
                        }
                    }
                    Err(_) => "(could not read file)".to_string(),
                };
                // A repeated path keeps its first place, as a dict key does.
                match snippets.iter_mut().find(|(path, _)| path == rel) {
                    Some(slot) => slot.1 = text,
                    None => snippets.push((rel.clone(), text)),
                }
            }
            context = context.put(
                "source_snippets",
                Value::Object(
                    snippets
                        .iter()
                        .map(|(k, v)| (k.clone(), json!(v)))
                        .collect(),
                ),
            );
            if !out_of_repo.is_empty() {
                context = context.put("out_of_repo_files", json!(out_of_repo));
            }
        }
        let guidance = review_guidance_text(&radius, &changed_funcs, &tested);
        context = context.put("review_guidance", guidance.as_str());
        let mut missingness = self.answerability.missingness();
        let unmatched = unmatched_changed_files(&changed_files, &radius.changed_nodes, self.root());
        if !unmatched.is_empty() {
            context = context.put("unmatched_changed_files", json!(unmatched));
            missingness.push(json!({
                "reason_code": "changed_files_not_in_graph",
                "severity": "high",
                "claim_effect": "these files are absent from the graph, so their context and impact are unknown rather than empty",
                "details": {"unmatched_changed_files": &unmatched[..unmatched.len().min(20)]},
            }));
        }
        let summary = [
            format!(
                "Review context for {} changed file(s):",
                changed_files.len()
            ),
            format!("  - {} directly changed nodes", radius.changed_nodes.len()),
            format!(
                "  - {} impacted nodes in {} files",
                radius.impacted_nodes.len(),
                radius.impacted_files.len()
            ),
            String::new(),
            "Review guidance:".to_string(),
            guidance,
        ]
        .join("\n");
        let mut payload = Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("context", context.value())
            .put("answerability", self.answerability.full())
            .put("missingness", json!(missingness))
            .apply_output_budget(CONTEXT_BUDGET, &["impacted_files", "changed_files"]);
        payload = budget_source_snippets(payload, &snippets);
        if !graph_truncation.is_empty() {
            payload = payload.set("truncated", json!(true));
            let mut merged = payload
                .get("_truncation")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            merged.extend(graph_truncation);
            payload = payload.set("_truncation", Value::Object(merged));
        }
        Some(payload)
    }

    /// `get_affected_flows_func`.
    fn affected_flows(&self, request: &Request) -> Option<Ordered> {
        let (changed_files, sources) = self.changed_files(request)?;
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
            .put("change_file_sources", sources)
            .put("affected_flows", Value::Array(flows))
            .put("total", total)
            .put("answerability", self.answerability.full())
            .put("missingness", json!(self.answerability.missingness()));
        let hints = self.hints("get_affected_flows", &out.value());
        Some(out.put("_hints", hints))
    }

    /// `get_impact_radius` (the tool, `dagayn.tools.query`).
    fn impact(&self, request: &Request) -> Option<Ordered> {
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
        let unmatched = unmatched_changed_files(&changed_files, &radius.changed_nodes, self.root());

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

/// `guidance_actions_to_hints(guidance)`: the first three actions as next
/// steps, and the medium or high missingness codes met on the way.
pub(crate) fn guidance_actions_to_hints(guidance: &[Value]) -> Value {
    let mut next_steps = Vec::new();
    let mut warnings = Vec::new();
    for item in guidance {
        let (tool, suggestion) = match item.get("action") {
            Some(Value::Object(action)) => {
                let text = |v: Option<&Value>| match v {
                    Some(value) if !value.is_null() && value != "" => Some(match value {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }),
                    _ => None,
                };
                let tool = text(action.get("tool")).unwrap_or_else(|| "manual".to_string());
                let suggestion = text(action.get("suggestion"))
                    .or_else(|| text(action.get("command")))
                    .unwrap_or_else(|| tool.clone());
                (tool, suggestion)
            }
            action => {
                let text = match action {
                    Some(Value::String(s)) => s.clone(),
                    None | Some(Value::Null) => String::new(),
                    Some(other) => other.to_string(),
                };
                let head = text.split(" -- ").next().unwrap_or("");
                let tool = if head.is_empty() {
                    "manual".to_string()
                } else {
                    head.split(' ')
                        .next()
                        .unwrap_or("")
                        .split('(')
                        .next()
                        .unwrap_or("")
                        .to_string()
                };
                (tool, text)
            }
        };
        if suggestion.is_empty() {
            continue;
        }
        next_steps.push(json!({"tool": tool, "suggestion": suggestion}));
        let missing = match item.get("missingness") {
            Some(Value::Object(one)) => vec![Value::Object(one.clone())],
            Some(Value::Array(many)) => many.clone(),
            _ => Vec::new(),
        };
        for entry in missing {
            let severity = entry
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("info");
            if matches!(severity, "medium" | "high")
                && let Some(code) = entry
                    .get("reason_code")
                    .filter(|c| !c.is_null() && *c != "")
            {
                warnings.push(match code {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
        }
        if next_steps.len() >= 3 {
            break;
        }
    }
    json!({"next_steps": next_steps, "related": [], "warnings": warnings})
}

/// `_unmatched_changed_files`: the changed files no changed node belongs to.
fn unmatched_changed_files(
    changed_files: &[String],
    nodes: &[GraphNode],
    root: &Path,
) -> Vec<String> {
    let matched: HashSet<String> = nodes
        .iter()
        .filter(|node| !node.file_path.is_empty())
        .map(|node| normalized_repo_path(&node.file_path, root))
        .collect();
    changed_files
        .iter()
        .filter(|file| !matched.contains(&normalized_repo_path(file, root)))
        .cloned()
        .collect()
}

/// `_relative_qualified_name`.
fn relative_qualified_name(qualified_name: &str, root: &Path) -> String {
    let (head, tail) = match qualified_name.split_once("::") {
        Some((head, tail)) => (head, Some(tail)),
        None => (qualified_name, None),
    };
    let path = Path::new(head);
    let head = if path.is_absolute() {
        let normal: PathBuf = path.components().collect();
        match normal.strip_prefix(root) {
            Ok(rel) => {
                let text = rel.to_string_lossy().into_owned();
                if text.is_empty() {
                    ".".to_string()
                } else {
                    text
                }
            }
            Err(_) => head.to_string(),
        }
    } else {
        head.to_string()
    };
    match tail {
        Some(tail) => format!("{head}::{tail}"),
        None => head,
    }
}

/// `resolve_contained_path`: the file under `root`, symlinks resolved, or
/// `None` when it escapes or cannot be resolved.
fn resolve_contained_path(rel: &str, root: &Path) -> Option<PathBuf> {
    let candidate = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        root.join(rel)
    };
    let resolved = python_resolve(&candidate)?;
    let root = python_resolve(root)?;
    (resolved == root || resolved.starts_with(&root)).then_some(resolved)
}

/// `Path.resolve()` (non-strict): symlinks of the existing prefix resolved,
/// `..` folded, the missing rest kept.
fn python_resolve(path: &Path) -> Option<PathBuf> {
    if let Ok(real) = path.canonicalize() {
        return Some(real);
    }
    let mut existing = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        rest.push(existing.file_name()?.to_os_string());
        if !existing.pop() {
            return None;
        }
    }
    let mut resolved = existing.canonicalize().ok()?;
    for part in rest.into_iter().rev() {
        match part.to_str() {
            Some("..") => {
                resolved.pop();
            }
            Some(".") => {}
            _ => resolved.push(part),
        }
    }
    Some(resolved)
}

/// `"\n".join(f"{i + 1}: {line}" for i in range(start, end))`.
fn numbered(lines: &[&str], start: usize, end: usize) -> String {
    (start..end)
        .map(|i| format!("{}: {}", i + 1, lines[i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `_extract_relevant_lines`.
fn relevant_lines(lines: &[&str], nodes: &[GraphNode], file_path: &str) -> String {
    let mut ranges: Vec<(usize, usize)> = nodes
        .iter()
        .filter(|node| node.file_path == file_path)
        .map(|node| {
            let start = (node.line_start - 3).max(0) as usize;
            let end = ((node.line_end + 2).max(0) as usize).min(lines.len());
            (start, end)
        })
        .collect();
    if ranges.is_empty() {
        return numbered(lines, 0, lines.len().min(50));
    }
    ranges.sort();
    let mut merged = vec![ranges[0]];
    for (start, end) in ranges.into_iter().skip(1) {
        let last = merged.len() - 1;
        if start <= merged[last].1 + 1 {
            merged[last].1 = merged[last].1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for (start, end) in merged {
        if !parts.is_empty() {
            parts.push("...".to_string());
        }
        // `range(start, end)` is empty when the span is.
        if start < end {
            for (offset, line) in lines[start..end].iter().enumerate() {
                parts.push(format!("{}: {line}", start + offset + 1));
            }
        }
    }
    parts.join("\n")
}

/// `_generate_review_guidance`.
fn review_guidance_text(
    radius: &ImpactRadius,
    changed_funcs: &[&GraphNode],
    tested: &HashSet<&str>,
) -> String {
    let mut parts = Vec::new();
    let untested: Vec<&&GraphNode> = changed_funcs
        .iter()
        .filter(|f| !tested.contains(f.qualified_name.as_str()) && !f.is_test)
        .collect();
    if !untested.is_empty() {
        let names: Vec<&str> = untested.iter().take(5).map(|f| f.name.as_str()).collect();
        parts.push(format!(
            "- {} changed function(s) lack test coverage: {}",
            untested.len(),
            names.join(", ")
        ));
    }
    if radius.impacted_nodes.len() > 20 {
        parts.push(format!(
            "- Wide blast radius: {} nodes impacted. Review callers and dependents carefully.",
            radius.impacted_nodes.len()
        ));
    }
    let inheritance = radius
        .edges
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "INHERITS" | "IMPLEMENTS"))
        .count();
    if inheritance > 0 {
        parts.push(format!(
            "- {inheritance} inheritance/implementation relationship(s) affected. Check for Liskov substitution violations."
        ));
    }
    if radius.impacted_files.len() > 3 {
        parts.push(format!(
            "- Changes impact {} other files. Consider splitting into smaller PRs.",
            radius.impacted_files.len()
        ));
    }
    if parts.is_empty() {
        parts.push("- Changes appear well-contained with minimal blast radius.".to_string());
    }
    parts.join("\n")
}

/// `_budget_source_snippets`: keep source until `MAX_SNIPPET_BYTES`, clipping
/// only a first file that is over on its own.
fn budget_source_snippets(payload: Ordered, snippets: &[(String, String)]) -> Ordered {
    if snippets.is_empty() {
        return payload;
    }
    let mut kept: Vec<(String, String)> = Vec::new();
    let (mut used, mut dropped, mut clipped) = (0_usize, Vec::new(), Vec::new());
    for (path, text) in snippets {
        let mut body = text.clone();
        let mut size = body.len();
        let remaining = MAX_SNIPPET_BYTES.saturating_sub(used);
        if remaining == 0 {
            dropped.push(path.clone());
            continue;
        }
        if size > remaining {
            if !kept.is_empty() {
                dropped.push(path.clone());
                continue;
            }
            let bytes = &body.as_bytes()[..remaining];
            let valid = match std::str::from_utf8(bytes) {
                Ok(text) => text.len(),
                Err(error) => error.valid_up_to(),
            };
            body = format!("{}\n... (truncated)", &body[..valid]);
            clipped.push(path.clone());
            size = remaining;
        }
        kept.push((path.clone(), body));
        used += size;
    }
    if dropped.is_empty() && clipped.is_empty() {
        return payload;
    }
    let mut payload = payload;
    let mut context = payload
        .get("context")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // Python assigns `source_snippets` in place and appends the rest.
    context.insert(
        "source_snippets".into(),
        Value::Object(kept.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
    );
    if !dropped.is_empty() {
        context.insert("source_snippets_omitted".into(), json!(dropped));
    }
    if !clipped.is_empty() {
        context.insert("source_snippets_clipped".into(), json!(clipped));
    }
    payload = payload
        .replace("context", Value::Object(context))
        .set("truncated", json!(true));
    let mut truncation = payload
        .get("_truncation")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    truncation.insert(
        "source_snippets".into(),
        json!({"kept": kept.len(), "total": snippets.len()}),
    );
    payload.set("_truncation", Value::Object(truncation))
}
