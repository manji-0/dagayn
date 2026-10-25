//! `review_tool` (`dagayn.tools.review_dispatcher.review_func`) for both
//! modes: `changes` (`detect_changes_func`) and `affected_flows`
//! (`get_affected_flows_func`), in a git checkout, a jj workspace, an SVN
//! working copy, or none, with the error envelopes of the exceptions those
//! bodies raised.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use dagayn_build::{
    ChangeError, ChangeSources, change_file_sources, diff_stamp_error, staged_and_unstaged,
};
use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::changes::{Analysis, DiffParse, Ranges, analyze_changes, parse_diff};
use crate::coverage::splitlines;
use crate::findings;
use crate::{Args, Context, OpenGraph, Ordered, Payload, open_graph, resolve_repo};

const DECLARED: &[&str] = &[
    "mode",
    "base",
    "changed_files",
    "include_source",
    "repo_root",
    "detail_level",
];

/// `detect_changes_func`'s `apply_output_budget` budget, in tokens.
const CHANGES_BUDGET: usize = 8000;
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
/// Entry points `affected_flows` lists before counting the rest.
const ENTRY_POINT_LIMIT: usize = 10;

/// `review_tool`'s arguments once fastmcp and `parse_review_request` accept
/// them.
struct Request<'a> {
    base: &'a str,
    changed_files: Option<Vec<String>>,
    detail_level: &'a str,
    include_source: Option<bool>,
}

impl<'a> Request<'a> {
    fn parse(arguments: &'a Map<String, Value>) -> Option<Self> {
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
            detail_level,
            include_source,
        })
    }
}

pub(crate) fn review(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let mode = match arguments.get("mode") {
        None => "changes",
        Some(Value::String(mode)) if matches!(mode.as_str(), "changes" | "affected_flows") => {
            mode.as_str()
        }
        _ => return None,
    };
    let mut request = Request::parse(arguments)?;
    let runtime = context.runtime.clone()?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    if !arguments.contains_key("base") {
        request.base = dagayn_build::default_review_base(&root);
    }
    let graph = open_graph(&root)?;
    let answerability = graph.answerability()?;
    let review = Review {
        graph: &graph,
        answerability: &answerability,
    };
    let (subtool, answer) = match mode {
        "changes" => ("detect_changes_func", review.changes(&request)?),
        _ => ("get_affected_flows_func", review.affected_flows(&request)?),
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
            runtime,
            trailing: Vec::new(),
            repo: graph.repo_context(),
        },
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
}

impl Review<'_> {
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
        ranges: &Ranges,
        base: &str,
    ) -> Option<(Value, Value, Value)> {
        let relative: Vec<String> = changed_files
            .iter()
            .map(|file| normalized_repo_path(file, self.root()))
            .collect();
        let found = findings::change_findings(
            self.store(),
            self.root(),
            analysis,
            findings::ChangeSet {
                files: &relative,
                ranges,
                base,
            },
        )?;
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
        let (findings, findings_omitted, symbol_delta) =
            self.findings(&mut analysis, &changed_files, &ranges, request.base)?;
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
            .put(
                "unmapped_changed_files",
                analysis.get("unmapped_changed_files").clone(),
            );
        out = match request.detail_level {
            "minimal" => out,
            "standard" => out.put(
                "changed_functions",
                analysis.get("changed_functions").clone(),
            ),
            // `verbose`: every analysis field, and what changed at `base`.
            _ => {
                let mut full = out
                    .put("change_file_sources", sources)
                    .put("symbol_delta", symbol_delta);
                for (key, value) in &analysis.fields {
                    if full.get(key).is_none() {
                        full = full.put(key, value.clone());
                    }
                }
                full
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
                    "changed_functions",
                    // Counted in `changed_file_count`; a long diff's paths
                    // go first.
                    "unmapped_changed_files",
                    "changed_files",
                ],
            );
        Some(Ok(out))
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
        let out = Ordered::default()
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
        Some(Ok(out))
    }
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
