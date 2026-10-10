//! `flow_tool` (`dagayn.tools.flow_dispatcher.flow_func`): the entry points
//! that reach a symbol, or every entry point per unit.

use serde_json::{Map, Value};

use crate::{Args, Context, Payload, open_graph, resolve_repo, seal_dispatch};

const DECLARED: &[&str] = &["mode", "limit", "detail_level", "target", "repo_root"];

pub(crate) fn flow(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let text = |key: &str, default: &'static str, allowed: &[&str]| -> Option<String> {
        match arguments.get(key) {
            None => Some(default.to_string()),
            Some(Value::String(value)) if allowed.contains(&value.as_str()) => Some(value.clone()),
            Some(_) => None,
        }
    };
    let mode = text("mode", "entry_points", &["entry_points"])?;
    let detail_level = text("detail_level", "standard", &["minimal", "standard"])?;
    let limit = args.integer("limit", 10)?;
    let target = args.optional_string("target")?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let runtime = context.runtime.clone()?;
    let graph = open_graph(&root)?;
    let answerability = graph.answerability()?;
    let out = match target.filter(|t| !t.is_empty()) {
        Some(target) => crate::entry_points::entry_points(
            &graph.store,
            arguments,
            &answerability,
            target,
            limit,
            &detail_level,
        )?,
        None => crate::entry_points::entry_point_map(
            &graph.store,
            &root,
            &answerability,
            limit,
            &detail_level,
        )?,
    };
    Some(seal_dispatch(
        out,
        crate::Dispatch {
            mode: &mode,
            subtool: "entry_points",
            runtime,
            trailing: Vec::new(),
            repo: graph.repo_context(),
        },
    ))
}
