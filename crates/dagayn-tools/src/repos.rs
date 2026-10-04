//! `list_repos_tool` (`dagayn.tools.registry_tools.list_repos_func`): the
//! multi-repo registry at `~/.dagayn/registry.json`, read as it is.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::{Args, Context, Ordered, Payload, suggestions};

const NEXT_TOOL_SUGGESTIONS: [&str; 2] = [
    "cross_repo_search_tool -- search across registered repositories",
    "dagayn register <path> -- add another repository to the registry",
];

pub(crate) fn list_repos(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    Args::new(arguments, &[])?;
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    let directory = PathBuf::from(home).join(".dagayn");
    // `Registry()` creates the directory; that write is Python's.
    if !directory.is_dir() {
        return None;
    }
    let path = directory.join("registry.json");
    let repos = if path.exists() {
        // An unreadable or malformed registry is logged and replaced by
        // Python; only a well-formed one is read here.
        let text = String::from_utf8(std::fs::read(&path).ok()?).ok()?;
        match serde_json::from_str::<Value>(&text).ok()? {
            Value::Object(data) => match data.get("repos") {
                None => Vec::new(),
                Some(Value::Array(repos)) => repos.clone(),
                Some(_) => return None,
            },
            _ => return None,
        }
    } else {
        Vec::new()
    };
    let (hints, kept) = suggestions(context, &NEXT_TOOL_SUGGESTIONS);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put(
                "summary",
                format!("{} registered repository(ies).", repos.len()),
            )
            .put("repos", Value::Array(repos))
            .put("_hints", hints)
            .put("next_tool_suggestions", kept)
            .into_payload(),
    )
}
