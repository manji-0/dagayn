//! `review_tool` (`dagayn.tools.review_dispatcher.review_func`) for every
//! mode: `changes` (`detect_changes_func`), `context`
//! (`get_review_context`), `affected_flows` (`get_affected_flows_func`), and
//! `impact` (`dagayn.tools.query.get_impact_radius`), in a git checkout, a
//! jj workspace, an SVN working copy, or none, with the error envelopes of
//! the exceptions those bodies raised.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use dagayn_build::{
    ChangeError, ChangeSources, change_file_sources, diff_stamp_error, staged_and_unstaged,
};
use dagayn_graph::{
    GraphNode, GraphStore, ImpactRadius, is_low_confidence_unresolved_markdown_code_span,
};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::changes::{Analysis, DiffParse, analyze_changes, parse_diff};
use crate::coverage::splitlines;
use crate::findings;
use crate::hints::{generate_hints, session};
use crate::query::{edge_dict, node_dict};
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
/// The source cap below `verbose`, so source and graph fit `CONTEXT_BUDGET`
/// together (docs/plans/AGENT-WORKFLOW-TARGET.md#target-contract).
const STANDARD_SNIPPET_BYTES: usize = 16_000;
/// Low-confidence bridges a folded caveat names as examples.
const CAVEAT_EXAMPLES: usize = 3;
const MAX_LINES_PER_FILE_CEILING: i64 = 2000;
/// The lists `get_review_context` halves to fit its budget, highest
/// priority first.
/// The path lists of `change_file_sources`, which a long diff repeats,
/// highest priority first.
const SOURCE_LISTS: [&str; 6] = [
    "change_file_sources.files",
    "change_file_sources.base_diff",
    "change_file_sources.worktree",
    "change_file_sources.staged",
    "change_file_sources.unstaged",
    "change_file_sources.untracked",
];
const CONTEXT_PRIORITIES: [&str; 13] = [
    "context.changed_files",
    "context.impacted_files",
    "context.graph.changed_nodes",
    "context.graph.impacted_nodes",
    "context.graph.edges",
    // Paths a long diff repeats, which go before the graph does.
    "context.unmatched_changed_files",
    "context.source_snippets_omitted",
    "context.change_file_sources.untracked",
    "context.change_file_sources.unstaged",
    "context.change_file_sources.staged",
    "context.change_file_sources.worktree",
    "context.change_file_sources.base_diff",
    "context.change_file_sources.files",
];
/// Fields `detail_level="verbose"` still carries from the score-first
/// contract, for one release.
/// Entry points `affected_flows` lists before counting the rest.
const ENTRY_POINT_LIMIT: usize = 10;

const DEPRECATED_FIELDS: &[&str] = &[
    "risk_score",
    "review_priority_score",
    "score_semantics",
    "review_priorities",
    "test_gaps",
    "test_gap_evidence",
    "changed_edges",
    "analysis_summary",
];

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
        // No base: `review` picks one from the working tree once the repo
        // root is known.
        let base = match arguments.get("base") {
            None => "",
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
    let mut request = Request::parse(&args, arguments)?;
    let runtime = context.runtime.clone()?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    if !arguments.contains_key("base") {
        request.base = dagayn_build::default_review_base(&root);
    }
    let graph = open_graph(&root)?;
    let answerability = graph.answerability()?;
    let exposed = |tool: &str| context.exposes(tool);
    let review = Review {
        graph: &graph,
        answerability: &answerability,
        exposed: &exposed,
    };
    let (subtool, answer) = match mode {
        "changes" => ("detect_changes_func", review.changes(&request)?),
        "context" => ("get_review_context", review.context(&request, &args)?),
        "affected_flows" => ("get_affected_flows_func", review.affected_flows(&request)?),
        _ => ("get_impact_radius", review.impact(&request)?),
    };
    let out = match answer {
        Ok(out) => out,
        // `with_dispatch_metadata` seals an error without hints.
        Err(Failure::BaseUnresolved {
            message,
            missingness,
        }) => {
            return Some(
                Ordered::default()
                    .put("status", "error")
                    .put("summary", message.as_str())
                    .put("error", message.as_str())
                    .put("mode", mode)
                    .put("called_subtool", subtool)
                    .put("base", request.base)
                    .put("diff_parse_status", "base_unresolved")
                    .put("answerability", answerability.full())
                    .put("missingness", json!(missingness))
                    .put("_runtime", runtime)
                    .put("_repo", graph.repo_context())
                    .into_payload(),
            );
        }
        // `handle_tool_runtime_error`'s envelope has no summary, so the
        // dispatcher's stands; the graph's answerability is attached, but
        // its missingness is the failure's alone.
        Err(Failure::Raised(error)) => {
            let reason_code = if error.runtime_error {
                "tool_runtime_error"
            } else {
                "unexpected_tool_failure"
            };
            return Some(
                Ordered::default()
                    .put("status", "error")
                    .put("summary", format!("Review mode '{mode}' completed."))
                    .put("error", error.message)
                    .put("mode", mode)
                    .put("called_subtool", subtool)
                    .put(
                        "missingness",
                        json!([{
                            "reason_code": reason_code,
                            "severity": "high",
                            "claim_effect": "tool output is unavailable until the underlying failure is resolved",
                        }]),
                    )
                    .put("answerability", answerability.full())
                    .put("_runtime", runtime)
                    .put("_repo", graph.repo_context())
                    .into_payload(),
            );
        }
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

/// A review body's error reply instead of its answer.
enum Failure {
    /// `detect_changes_func`'s `_error_response` for a diff base that does
    /// not resolve.
    BaseUnresolved {
        message: String,
        missingness: Vec<Value>,
    },
    /// An exception the body raised, which its `ToolStoreScope` reports.
    Raised(ChangeError),
}

impl From<ChangeError> for Failure {
    fn from(error: ChangeError) -> Self {
        Self::Raised(error)
    }
}

/// A body's answer, or its error reply; `None` leaves the call to Python.
type Answer = Option<Result<Ordered, Failure>>;

/// `?` for a body's [`Answer`]: the value, or an early return of the
/// declined call or the error reply.
macro_rules! attempt {
    ($value:expr) => {
        match $value {
            Some(Ok(value)) => value,
            Some(Err(failure)) => return Some(Err(failure.into())),
            None => return None,
        }
    };
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

    /// The changed files and their sources, as `_resolve_changed_files`
    /// detects them, or what it raises.
    fn changed_files(&self, request: &Request) -> Result<(Vec<String>, Value), ChangeError> {
        Ok(match &request.changed_files {
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

    /// The findings for a change set (`findings.rs`), most actionable kind
    /// first, the per-kind omissions, and the base-side symbol delta.
    ///
    /// `changed_functions` loses the ones the base comparison shows only
    /// moved or were reformatted: when the graph is behind a file, every
    /// function in it is mapped as changed.
    fn findings(
        &self,
        analysis: &mut Analysis,
        changed_files: &[String],
        base: &str,
    ) -> Option<(Value, Value, Value)> {
        let relative: Vec<String> = changed_files
            .iter()
            .map(|file| normalized_repo_path(file, self.root()))
            .collect();
        let found =
            findings::change_findings(self.store(), self.root(), analysis, &relative, base)?;
        for (key, value) in &mut analysis.fields {
            if *key == "changed_functions"
                && let Value::Array(functions) = value
            {
                functions.retain(|function| {
                    function["qualified_name"]
                        .as_str()
                        .is_none_or(|qn| !found.delta.unchanged_bodies.contains(qn))
                });
            }
        }
        Some((
            Value::Array(found.findings),
            Value::Object(found.omitted),
            found.delta.to_json(),
        ))
    }

    /// `detect_changes_func`.
    fn changes(&self, request: &Request) -> Answer {
        let (changed_files, sources) = attempt!(Some(self.changed_files(request)));
        if changed_files.is_empty() {
            return Some(Ok(Ordered::default()
                .put("status", "ok")
                .put("summary", "No changed files detected.")
                .put("base", request.base)
                .put("findings", json!([]))
                .put("findings_omitted", json!({}))
                .put("next", json!([]))
                .put("changed_file_count", 0)
                .put("changed_files", json!([]))
                .put("answerability", self.answerability.compact())
                .put("missingness", json!(self.answerability.missingness()))));
        }
        // `parse_diff_result` reads the cache stamp before the diff.
        if let Some(error) = diff_stamp_error(self.root()) {
            return Some(Err(error.into()));
        }
        let mut ranges = match parse_diff(self.root(), request.base) {
            DiffParse::Ranges(ranges) => ranges,
            DiffParse::BaseUnresolved => {
                return Some(Err(self.base_unresolved(request.base)));
            }
        };
        // An explicit file list scopes the review: the base diff only narrows
        // those files to their changed lines and adds no other file's nodes.
        if request.changed_files.is_some() {
            let wanted: HashSet<String> = changed_files
                .iter()
                .map(|file| absolute_path(self.root(), file))
                .collect();
            ranges.retain(|rel, _| wanted.contains(&absolute_path(self.root(), rel)));
        }
        let mut analysis = analyze_changes(
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
        let summary = change_analysis_summary(
            self.store(),
            &analysis,
            &impact,
            &changed_files,
            request.detail_level == "verbose",
        )?;
        let (findings, findings_omitted, symbol_delta) =
            self.findings(&mut analysis, &changed_files, request.base)?;
        if request.include_source == Some(true) {
            for (key, value) in &mut analysis.fields {
                if *key == "changed_functions"
                    && let Value::Array(functions) = value
                {
                    for function in functions.iter_mut() {
                        self.attach_source(function);
                    }
                }
            }
        }

        let summary_text =
            findings_summary(&changed_files, &analysis, &findings, &findings_omitted);
        let stale_files = analysis.get("attribution")["stale_line_range_files"]
            .as_array()
            .cloned();
        let flow_count = analysis
            .get("affected_flows")
            .as_array()
            .map_or(0, Vec::len);
        let mut out = Ordered::default()
            .put("status", "ok")
            .put("summary", summary_text)
            .put("base", request.base)
            .put("findings", findings.clone())
            .put("findings_omitted", findings_omitted)
            .put(
                "next",
                crate::next::from_findings(findings.as_array().map_or(&[], Vec::as_slice)),
            )
            .put("changed_file_count", changed_files.len())
            .put("changed_files", json!(changed_files))
            .put("change_file_source_counts", source_counts(&sources))
            .put(
                "change_entity_summary",
                analysis.get("change_entity_summary").clone(),
            )
            .put("affected_flow_count", flow_count)
            .put(
                "unmapped_changed_files",
                analysis.get("unmapped_changed_files").clone(),
            )
            .put("next_drill_downs", summary["next_drill_downs"].clone());
        out = match request.detail_level {
            "minimal" => out,
            "standard" => out
                .put(
                    "changed_functions",
                    analysis.get("changed_functions").clone(),
                )
                .put("affected_flows", analysis.get("affected_flows").clone()),
            // `verbose`: the score-first fields of the earlier contract, kept
            // for one release (docs/plans/REVIEW-TOOL-TARGET.md).
            _ => {
                let mut legacy = out
                    .put("change_file_sources", sources)
                    .put("symbol_delta", symbol_delta)
                    .put("deprecated_fields", json!(DEPRECATED_FIELDS));
                for (key, value) in &analysis.fields {
                    if *key != "summary" {
                        legacy = legacy.put(key, value.clone());
                    }
                }
                legacy.put("analysis_summary", summary)
            }
        };
        let mut missingness = self.answerability.missingness();
        if let Some(stale) = stale_files.filter(|files| !files.is_empty()) {
            missingness.push(json!({
                "reason_code": "stale_graph_line_ranges",
                "severity": "medium",
                "claim_effect": format!(
                    "{} changed file(s) differ from what the graph indexed, so every function in them counts as changed; update the graph before trusting their findings",
                    stale.len()
                ),
                "files": stale,
            }));
        }
        let out = out
            .put(
                "answerability",
                if request.detail_level == "verbose" {
                    self.answerability.full()
                } else {
                    self.answerability.compact()
                },
            )
            .put("missingness", json!(missingness))
            .apply_output_budget(
                if request.detail_level == "minimal" {
                    crate::MINIMAL_BUDGET
                } else {
                    CHANGES_BUDGET
                },
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
                    "changed_edges",
                    // Counted in `changed_file_count`; a long diff's paths
                    // go first.
                    "unmapped_changed_files",
                    "changed_files",
                ],
            );
        let hints = findings_hints(&findings);
        Some(Ok(out.put("_hints", hints)))
    }

    /// `detect_changes_func`'s `include_source`: the changed function's lines,
    /// numbered, when its record names a file and a nonzero span.
    fn attach_source(&self, function: &mut Value) {
        let Value::Object(record) = function else {
            return;
        };
        let file = record
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let start = record
            .get("line_start")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let end = record.get("line_end").and_then(Value::as_i64).unwrap_or(0);
        if file.is_empty() || start == 0 || end == 0 {
            return;
        }
        let path = self.root().join(file);
        if !path.is_file() {
            return;
        }
        let source = match std::fs::read(&path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let lines = splitlines(&text);
                let first = (start - 1).max(0);
                let last = end.min(lines.len() as i64);
                (first..last)
                    .map(|index| format!("{}: {}", index + 1, lines[index as usize]))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Err(_) => "(could not read file)".to_string(),
        };
        record.insert("source".to_string(), json!(source));
    }

    /// The error `detect_changes_func` reports for a `base` the diff cannot
    /// resolve.
    fn base_unresolved(&self, base: &str) -> Failure {
        let message = format!(
            "Could not resolve the diff base {} in {}. Pass a reachable ref (the default HEAD~1 \
             does not exist in a single-commit repository, and a rebase or gc can make a \
             recorded sha unreachable).",
            crate::pyunicode::repr(base),
            self.root().display()
        );
        let mut missingness = self.answerability.missingness();
        missingness.push(json!({
            "reason_code": "diff_base_unreachable",
            "severity": "high",
            "claim_effect": "no diff could be computed, so nothing here describes what actually changed",
        }));
        Failure::BaseUnresolved {
            message,
            missingness,
        }
    }

    /// `get_review_context`.
    fn context(&self, request: &Request, args: &Args) -> Answer {
        let (changed_files, sources) = attempt!(Some(self.changed_files(request)));
        self.context_of(request, args, changed_files, sources)
            .map(Ok)
    }

    fn context_of(
        &self,
        request: &Request,
        args: &Args,
        changed_files: Vec<String>,
        sources: Value,
    ) -> Option<Ordered> {
        let include_source = request.include_source.unwrap_or(true);
        let max_lines = args
            .integer("max_lines_per_file", 200)?
            .clamp(1, MAX_LINES_PER_FILE_CEILING) as usize;
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
                    )
                    .apply_output_budget(crate::MINIMAL_BUDGET, &SOURCE_LISTS),
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
        let verbose = request.detail_level == "verbose";
        let snippet_bytes = if verbose {
            MAX_SNIPPET_BYTES
        } else {
            STANDARD_SNIPPET_BYTES
        };
        let mut payload = Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("context", context.value())
            .put("answerability", self.answerability.full())
            .put("missingness", json!(missingness));
        payload = budget_source_snippets(payload, &snippets, snippet_bytes);
        if !verbose {
            payload = payload.apply_output_budget(CONTEXT_BUDGET, &CONTEXT_PRIORITIES);
        }
        if !graph_truncation.is_empty() {
            payload = payload.set("truncated", json!(true));
            let mut merged = payload
                .get("_truncation")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            // A list the budget halved after the cap keeps the budget's count
            // and the total from before the cap.
            for (field, record) in graph_truncation {
                let record = match merged.remove(&format!("context.graph.{field}")) {
                    Some(budgeted) => json!({"kept": budgeted["kept"], "total": record["total"]}),
                    None => record,
                };
                merged.insert(field, record);
            }
            payload = payload.set("_truncation", Value::Object(merged));
        }
        Some(payload)
    }

    /// `get_affected_flows_func`: the entry points that reach the changed
    /// functions (docs/plans/FLOW-TOOL-TARGET.md#modes); the stored flows
    /// that contain them only at `detail_level="verbose"`, for one release.
    fn affected_flows(&self, request: &Request) -> Answer {
        let (changed_files, sources) = attempt!(Some(self.changed_files(request)));
        if changed_files.is_empty() {
            return Some(Ok(Ordered::default()
                .put("status", "ok")
                .put("summary", "No changed files detected.")
                .put("entry_points", json!([]))
                .put("next", json!([]))
                .put("entry_points_omitted", 0)
                .put("changed_function_count", 0)
                .put("answerability", self.answerability.full())
                .put("missingness", json!(self.answerability.missingness()))));
        }
        // Changed lines narrow a file to the functions they touch; a file
        // without a diff range (untracked, or an unresolved base) counts whole.
        let ranges = match parse_diff(self.root(), request.base) {
            DiffParse::Ranges(ranges) => ranges,
            DiffParse::BaseUnresolved => Default::default(),
        };
        let mut changed: Vec<GraphNode> = Vec::new();
        for file in &changed_files {
            let absolute = absolute_path(self.root(), file);
            let file_ranges = ranges
                .iter()
                .find(|(rel, _)| absolute_path(self.root(), rel) == absolute)
                .map(|(_, ranges)| ranges);
            for node in self.store().get_nodes_by_file(&absolute).ok()? {
                if node.kind != "Function" || !findings::is_production_code(&node, &node.file_path)
                {
                    continue;
                }
                let touched = file_ranges.is_none_or(|ranges| {
                    ranges
                        .iter()
                        .any(|(start, end)| *start <= node.line_end && node.line_start <= *end)
                });
                if touched {
                    changed.push(node);
                }
            }
        }
        let (entries, omitted, reached, truncated) = crate::entry_points::entry_points_reaching(
            self.store(),
            &changed,
            ENTRY_POINT_LIMIT,
            request.detail_level,
        )?;
        let count = entries.len() + omitted;
        let mut missingness = self.answerability.missingness();
        if truncated {
            missingness.push(json!({
                "reason_code": "truncated_search",
                "severity": "medium",
                "claim_effect": "the entry-point search stopped early; farther entry points are not listed",
            }));
        }
        let mut out = Ordered::default()
            .put("status", "ok")
            .put(
                "summary",
                format!(
                    "{count} entry point(s) reach the {} changed function(s) in {} file(s)",
                    changed.len(),
                    changed_files.len()
                ),
            )
            .put("changed_files", json!(changed_files))
            .put("change_file_sources", sources)
            .put("changed_function_count", changed.len())
            .put("next", crate::next::read_entry_points(&entries))
            .put("entry_points", Value::Array(entries))
            .put("entry_points_omitted", omitted)
            .put("reached_callers", reached)
            .put("truncated", truncated);
        if request.detail_level == "verbose" {
            let absolute: Vec<String> = changed_files
                .iter()
                .map(|file| absolute_path(self.root(), file))
                .collect();
            let flows = self.store().get_affected_flows_annotated(&absolute).ok()?;
            out = out
                .put("total", flows.len())
                .put("affected_flows", Value::Array(flows))
                .put("deprecated_fields", json!(["affected_flows", "total"]));
        }
        let budget = if request.detail_level == "minimal" {
            crate::MINIMAL_BUDGET
        } else {
            crate::STANDARD_BUDGET
        };
        // Highest priority first: the paths go before the entry points do.
        let mut priorities = vec!["entry_points", "changed_files"];
        priorities.extend(SOURCE_LISTS);
        let out = out
            .put("answerability", self.answerability.full())
            .put("missingness", json!(missingness));
        let out = if request.detail_level == "verbose" {
            out
        } else {
            out.apply_output_budget(budget, &priorities)
        };
        let hints = self.hints("get_affected_flows", &out.value());
        Some(Ok(out.put("_hints", hints)))
    }

    /// `get_impact_radius` (the tool, `dagayn.tools.query`).
    fn impact(&self, request: &Request) -> Answer {
        let changed_files = match &request.changed_files {
            Some(files) => files.clone(),
            None => {
                // `get_changed_files`, then the worktree when it is empty.
                let files = attempt!(Some(change_file_sources(self.root(), request.base))).files;
                if files.is_empty() {
                    attempt!(Some(staged_and_unstaged(self.root())))
                } else {
                    files
                }
            }
        };
        self.impact_of(request, changed_files).map(Ok)
    }

    fn impact_of(&self, request: &Request, changed_files: Vec<String>) -> Option<Ordered> {
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
        if !caveats.is_empty() {
            impact_missingness.push(folded_caveats(caveats));
        }
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
                }],
                "confidence": "low",
                "missingness": [folded_caveats(caveats)],
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
                    .put("guidance", json!(guidance))
                    .apply_output_budget(crate::MINIMAL_BUDGET, &["unmatched_changed_files"]),
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

/// One missingness item for every low-confidence bridge: they share a
/// reason and an effect, so the count and a few examples say all of it
/// (docs/plans/AGENT-WORKFLOW-TARGET.md#caveats).
fn folded_caveats(caveats: &[Value]) -> Value {
    let examples: Vec<Value> = caveats
        .iter()
        .take(CAVEAT_EXAMPLES)
        .map(|caveat| caveat.get("bridge").cloned().unwrap_or(Value::Null))
        .collect();
    json!({
        "reason_code": "low_confidence_cross_artifact_bridge",
        "severity": "medium",
        "claim_effect":
            "bridges are visible as caveats only; do not treat the other side as confirmed impact",
        "count": caveats.len(),
        "examples": examples,
    })
}

/// The one-line summary: what changed, and the findings by kind, or that
/// nothing beyond the diff needs checking.
fn findings_summary(
    changed_files: &[String],
    analysis: &Analysis,
    findings: &Value,
    omitted: &Value,
) -> String {
    let symbols = analysis
        .get("changed_functions")
        .as_array()
        .map_or(0, Vec::len);
    let head = format!(
        "{} changed file(s), {symbols} changed symbol(s).",
        changed_files.len()
    );
    let mut counts: Vec<(String, u64)> = Vec::new();
    for finding in findings.as_array().into_iter().flatten() {
        let kind = finding["kind"].as_str().unwrap_or_default();
        match counts.iter_mut().find(|(k, _)| k == kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((kind.to_string(), 1)),
        }
    }
    for (kind, count) in &mut counts {
        *count += omitted[kind.as_str()].as_u64().unwrap_or(0);
    }
    if counts.is_empty() {
        return format!("{head} Nothing beyond the diff needs checking.");
    }
    let listed: Vec<String> = counts
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect();
    format!("{head} Findings: {}.", listed.join(", "))
}

/// `_hints` from the findings: the first places to look, in order.
fn findings_hints(findings: &Value) -> Value {
    let mut steps: Vec<Value> = Vec::new();
    for finding in findings.as_array().into_iter().flatten() {
        if steps.len() == 3 {
            break;
        }
        let kind = finding["kind"].as_str().unwrap_or_default();
        let step = match kind {
            "tests_to_run" => match finding["command"].as_str() {
                Some(command) => json!({"tool": "shell", "suggestion": command}),
                None => {
                    json!({"tool": "shell", "suggestion": format!("run the tests in {}", finding["file"].as_str().unwrap_or_default())})
                }
            },
            "untested_change" => json!({
                "tool": "review_tool",
                "suggestion": "review_tool mode=\"context\" -- read the untested functions before adding a test",
            }),
            _ => {
                let target = finding["sites"][0]["qualified_name"]
                    .as_str()
                    .or_else(|| finding["qualified_name"].as_str())
                    .or_else(|| finding["file"].as_str())
                    .unwrap_or_default();
                json!({
                    "tool": "query_graph_tool",
                    "suggestion": format!("query_graph_tool pattern=\"source_of\" target=\"{target}\" -- {kind}"),
                })
            }
        };
        // Several untested files point at the same next step; list it once.
        if !steps.contains(&step) {
            steps.push(step);
        }
    }
    json!({"next_steps": steps, "related": [], "warnings": []})
}

/// How many changed files each source (`base_diff`, `staged`, ...) named;
/// `minimal` lists the files once in `changed_files`.
fn source_counts(sources: &Value) -> Value {
    let counts: Map<String, Value> = sources
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.as_str() != "files")
        .filter_map(|(key, value)| Some((key.clone(), json!(value.as_array()?.len()))))
        .collect();
    Value::Object(counts)
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
        let step = json!({"tool": tool, "suggestion": suggestion});
        if !next_steps.contains(&step) {
            next_steps.push(step);
        }
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
                let code = match code {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if !warnings.contains(&code) {
                    warnings.push(code);
                }
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

/// `_budget_source_snippets`: keep source until `max_bytes`, clipping
/// only a first file that is over on its own.
fn budget_source_snippets(
    payload: Ordered,
    snippets: &[(String, String)],
    max_bytes: usize,
) -> Ordered {
    if snippets.is_empty() {
        return payload;
    }
    let mut kept: Vec<(String, String)> = Vec::new();
    let (mut used, mut dropped, mut clipped) = (0_usize, Vec::new(), Vec::new());
    for (path, text) in snippets {
        let mut body = text.clone();
        let mut size = body.len();
        let remaining = max_bytes.saturating_sub(used);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caveats_fold_into_one_counted_item() {
        let caveats: Vec<Value> = (0..600)
            .map(|i| {
                json!({
                    "reason_code": "low_confidence_cross_artifact_bridge",
                    "bridge": {"source": format!("README.{i}.md::usage"), "target": "app.py::helper"},
                })
            })
            .collect();
        let folded = folded_caveats(&caveats);
        assert_eq!(folded["count"], 600);
        assert_eq!(
            folded["examples"].as_array().map(Vec::len),
            Some(CAVEAT_EXAMPLES)
        );
        assert_eq!(folded["examples"][0]["source"], "README.0.md::usage");
    }

    #[test]
    fn hints_follow_the_findings_once_each() {
        let findings = json!([
            {"kind": "untested_change", "file": "a.py"},
            {"kind": "untested_change", "file": "b.py"},
            {"kind": "tests_to_run", "file": "t.py", "command": "pytest t.py"},
            {"kind": "dangling_reference", "qualified_name": "a.py::gone",
             "sites": [{"qualified_name": "c.py::caller"}]},
        ]);
        let steps = findings_hints(&findings)["next_steps"].clone();
        let tools: Vec<&str> = steps
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["tool"].as_str().unwrap())
            .collect();
        assert_eq!(tools, ["review_tool", "shell", "query_graph_tool"]);
        assert_eq!(steps[1]["suggestion"], "pytest t.py");
        assert!(
            steps[2]["suggestion"]
                .as_str()
                .unwrap()
                .contains("target=\"c.py::caller\"")
        );
        assert_eq!(findings_hints(&json!([]))["next_steps"], json!([]));
    }
}
