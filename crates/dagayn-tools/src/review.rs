//! `review_tool(mode="affected_flows")`: `review_func` dispatching to
//! `get_affected_flows_func` (`dagayn.tools.review_flows`). Every other mode
//! stays Python's.

use std::path::{Path, PathBuf};

use dagayn_build::{ChangeSources, change_file_sources, staged_and_unstaged};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::hints::{generate_hints, session};
use crate::{Args, Context, Ordered, Payload, explicit_repo, open_graph};

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

pub(crate) fn review(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    if arguments.get("mode")?.as_str()? != "affected_flows" {
        return None;
    }
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
    // The arguments this mode ignores still go through fastmcp and
    // `parse_review_request`.
    if !matches!(
        arguments.get("include_source"),
        None | Some(Value::Null | Value::Bool(_))
    ) {
        return None;
    }
    for key in ["max_depth", "max_nodes", "max_lines_per_file"] {
        args.integer(key, 0)?;
    }
    match arguments.get("detail_level") {
        None => {}
        Some(Value::String(level))
            if matches!(level.as_str(), "minimal" | "standard" | "verbose") => {}
        Some(_) => return None,
    }
    let runtime = context.runtime.clone()?;
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let store = &graph.store;
    let stats = store.get_stats().ok()?;
    let answerability = Answerability::recorded(store, &stats)?;

    let (changed_files, sources) = match changed_files {
        Some(files) => {
            let sources = json!({"files": files, "explicit": files});
            (files, sources)
        }
        None => {
            let sources = change_file_sources(&root, base)?;
            if sources.files.is_empty() {
                let files = staged_and_unstaged(&root)?;
                let sources = json!({"files": files, "worktree": files});
                (files, sources)
            } else {
                (sources.files.clone(), sources_value(sources))
            }
        }
    };

    let exposed = |tool: &str| {
        context
            .allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tool))
    };
    let mut hint_session = session();
    let payload = if changed_files.is_empty() {
        let out = Ordered::default()
            .put("status", "ok")
            .put("mode", "affected_flows")
            .put("called_subtool", "get_affected_flows_func")
            .put("summary", "No changed files detected.")
            .put("affected_flows", json!([]))
            .put("total", 0)
            .put("answerability", answerability.full())
            .put("missingness", json!(answerability.missingness()))
            .put("_runtime", runtime);
        let hints = generate_hints("review", &out.value(), &mut hint_session, &exposed);
        out.put("_hints", hints)
    } else {
        let absolute: Vec<String> = changed_files
            .iter()
            .map(|file| absolute_path(&root, file))
            .collect();
        let flows = store.get_affected_flows_annotated(&absolute).ok()?;
        let total = flows.len();
        let summary = format!(
            "{total} flow(s) affected by changes in {} file(s)",
            changed_files.len()
        );
        let mut seen = json!({
            "status": "ok",
            "summary": summary,
            "changed_files": changed_files,
            "change_file_sources": sources,
            "affected_flows": flows,
            "total": total,
            "answerability": answerability.full(),
            "missingness": answerability.missingness(),
        });
        let hints = generate_hints("get_affected_flows", &seen, &mut hint_session, &exposed);
        // `_with_dispatch_metadata` builds the review hints too, though the
        // flows' hints stay: the session records both.
        if let Some(object) = seen.as_object_mut() {
            object.insert("_hints".to_string(), hints.clone());
            object.insert("mode".to_string(), json!("affected_flows"));
            object.insert(
                "called_subtool".to_string(),
                json!("get_affected_flows_func"),
            );
            object.insert("_runtime".to_string(), runtime.clone());
        }
        generate_hints("review", &seen, &mut hint_session, &exposed);
        let field = |key: &str| seen.get(key).cloned().unwrap_or(Value::Null);
        // `seal_dispatcher_ok`: the envelope's fields first.
        Ordered::default()
            .put("status", "ok")
            .put("mode", "affected_flows")
            .put("called_subtool", "get_affected_flows_func")
            .put("summary", summary.as_str())
            .put("changed_files", field("changed_files"))
            .put("change_file_sources", field("change_file_sources"))
            .put("affected_flows", field("affected_flows"))
            .put("total", total)
            .put("answerability", field("answerability"))
            .put("missingness", field("missingness"))
            .put("_hints", hints)
            .put("_runtime", runtime)
    };
    // The server's `attach_repo_context`, after the sealing.
    Some(payload.put("_repo", graph.repo_context()).into_payload())
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
