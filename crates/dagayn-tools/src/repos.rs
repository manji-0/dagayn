//! `list_repos_tool` (`dagayn.tools.registry_tools.list_repos_func` over
//! `dagayn.registry.Registry`): the multi-repo registry at
//! `~/.dagayn/registry.json`. Like `Registry()`, the call creates
//! `~/.dagayn` when it is missing; a registry that is not JSON lists nothing,
//! and one Python could not list is reported as its error.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::{Args, Context, Ordered, Payload, suggestions};

const NEXT_TOOL_SUGGESTIONS: [&str; 2] = [
    "cross_repo_search_tool -- search across registered repositories",
    "dagayn register <path> -- add another repository to the registry",
];

pub(crate) fn list_repos(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    Args::new(arguments, &[])?;
    let directory = home()?.join(".dagayn");
    if let Err((error, path)) = mkdir(&directory, true, true) {
        return Some(runtime_error(os_error(&error, &path)?));
    }
    let repos = match load(&directory.join("registry.json"))? {
        Ok(repos) => repos,
        Err(message) => return Some(runtime_error(message)),
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

/// `Path.home()`: `$HOME` without trailing slashes (`/` when that leaves
/// nothing), else the password database's entry. `None` when there is
/// neither, where Python keeps `~` as a relative path.
fn home() -> Option<PathBuf> {
    let home = match std::env::var_os("HOME") {
        Some(home) => home.into_string().ok()?,
        None => std::env::home_dir()?.into_os_string().into_string().ok()?,
    };
    let home = home.trim_end_matches('/');
    Some(PathBuf::from(if home.is_empty() { "/" } else { home }))
}

/// `Path.mkdir(parents=..., exist_ok=...)`, failing with the error and the
/// path of the `mkdir` call that raised it.
fn mkdir(path: &Path, parents: bool, exist_ok: bool) -> Result<(), (std::io::Error, PathBuf)> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let parent = path.parent().filter(|parent| *parent != path);
            match parent {
                Some(parent) if parents => {
                    mkdir(parent, true, true)?;
                    mkdir(path, false, exist_ok)
                }
                _ => Err((error, path.to_path_buf())),
            }
        }
        Err(_) if exist_ok && path.is_dir() => Ok(()),
        Err(error) => Err((error, path.to_path_buf())),
    }
}

/// `str(OSError)` for an error from a call on `path`:
/// `[Errno N] <strerror>: '<path>'`. `None` for an error without an errno.
fn os_error(error: &std::io::Error, path: &Path) -> Option<String> {
    let errno = error.raw_os_error()?;
    let text = std::io::Error::from_raw_os_error(errno).to_string();
    let message = text
        .strip_suffix(&format!(" (os error {errno})"))
        .unwrap_or(&text);
    Some(format!(
        "[Errno {errno}] {message}: {}",
        crate::pyunicode::repr(path.to_str()?)
    ))
}

/// The type name `json.loads` gives a JSON value.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Whether `json.loads` may accept what serde rejected: `NaN`, `Infinity`, a
/// lone surrogate escape, a number past `f64`, or nesting past serde's limit.
fn python_may_parse(text: &str, error: &serde_json::Error) -> bool {
    let message = error.to_string();
    if message.contains("recursion limit") || message.contains("number out of range") {
        return true;
    }
    if text.contains("NaN") || text.contains("Infinity") {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    lower
        .match_indices("\\ud")
        .any(|(at, _)| lower[at + 3..].starts_with(['8', '9', 'a', 'b', 'c', 'd', 'e', 'f']))
}

/// `Registry()._load()` then `list_repos()`: the listed repos, or the message
/// of the exception Python raises. `None` where the outcome is Python's alone
/// (a registry only its JSON parser accepts, or one listed as a mapping, whose
/// key order serde does not keep).
fn load(path: &Path) -> Option<Result<Vec<Value>, String>> {
    if !path.exists() {
        return Some(Ok(Vec::new()));
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return Some(Err(os_error(&error, path)?)),
    };
    let text = crate::docs::decode_text(&bytes);
    let data = match serde_json::from_str::<Value>(&text) {
        Ok(data) => data,
        // Logged as an invalid registry, and listed as empty.
        Err(error) if !python_may_parse(&text, &error) => return Some(Ok(Vec::new())),
        Err(_) => return None,
    };
    let Value::Object(data) = data else {
        return Some(Err(format!(
            "'{}' object has no attribute 'get'",
            type_name(&data)
        )));
    };
    // `list(data.get("repos", []))`.
    Some(match data.get("repos") {
        None => Ok(Vec::new()),
        Some(Value::Array(repos)) => Ok(repos.clone()),
        Some(Value::String(text)) => Ok(text.chars().map(|c| json!(c.to_string())).collect()),
        Some(Value::Object(_)) => return None,
        Some(other) => Err(format!("'{}' object is not iterable", type_name(other))),
    })
}

/// `handle_tool_runtime_error` for an `OSError`, `AttributeError`, or
/// `TypeError`.
fn runtime_error(message: String) -> Payload {
    Ordered::default()
        .put("status", "error")
        .put("error", message)
        .put(
            "missingness",
            json!([{
                "reason_code": "tool_runtime_error",
                "severity": "high",
                "claim_effect": "tool output is unavailable until the underlying failure is resolved",
            }]),
        )
        .into_payload()
}
