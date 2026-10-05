//! `apply_refactor_tool`: a preview from the shared pending store
//! ([`crate::pending`]), applied at the line it recorded and to whole
//! identifiers only, or shown as a unified diff with `dry_run`.
//!
//! Every edited file is read before anything is written. A file that is not
//! UTF-8 is read with replacement characters and universal newlines, and is
//! written back that way.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::difflib::{splitlines, unified_diff};
use crate::docs::read_text;
use crate::pypath::realpath;
use crate::{Args, Context, Ordered, Payload, is_placeholder, pending};

struct Edit {
    file: String,
    line: Option<i64>,
    old: String,
    new: String,
    raw: Map<String, Value>,
}

impl Edit {
    fn parse(value: &Value) -> Option<Self> {
        let raw = value.as_object()?.clone();
        let text = |key: &str| raw.get(key)?.as_str().map(str::to_string);
        // `int(edit["line"])`, which truncates a float and counts a bool.
        let line = match raw.get("line") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(flag)) => Some(i64::from(*flag)),
            Some(Value::Number(number)) => match number.as_i64() {
                Some(line) => Some(line),
                None => {
                    let line = number.as_f64()?.trunc();
                    if line.abs() >= 9.0e15 {
                        return None;
                    }
                    Some(line as i64)
                }
            },
            Some(_) => return None,
        };
        Some(Self {
            file: text("file")?,
            line,
            old: text("old")?,
            new: text("new")?,
            raw,
        })
    }

    fn skipped(&self, reason: &str) -> Value {
        let field = |key: &str| self.raw.get(key).cloned().unwrap_or(Value::Null);
        json!({
            "file": field("file"),
            "line": field("line"),
            "old": field("old"),
            "new": field("new"),
            "reason": reason,
        })
    }
}

/// `_resolve_repo_path`: `(repo_root / path).resolve()`, an absolute path as
/// it is, resolved whether or not it exists.
fn resolve(file: &str, root: &str) -> Option<PathBuf> {
    let joined = if file.starts_with('/') {
        file.to_string()
    } else {
        format!("{root}/{file}")
    };
    realpath(&joined).map(PathBuf::from)
}

/// The root `apply_refactor_func` applies under, after `dagayn serve`'s
/// `_resolve_repo_root` (a `${...}` placeholder is no root; the pinned
/// `--repo` stands in for none): a named root checked as
/// `_validate_repo_root` does, with its message when it fails, or
/// `find_project_root()` from the working directory, which, unlike the
/// graph tools, may be the home directory. `None` for a root only Python
/// resolves (a jj or SVN walk, ambiguous workspace hints).
fn apply_root(context: &Context, requested: Option<&str>) -> Option<Result<String, String>> {
    let named = requested
        .filter(|value| !value.is_empty() && !is_placeholder(value))
        .map(str::to_string)
        .or_else(|| {
            let pinned = context.pinned_repo.as_ref()?.to_str()?;
            (!is_placeholder(pinned)).then(|| pinned.to_string())
        });
    let Some(named) = named else {
        let cwd = std::env::current_dir().ok()?;
        let dagayn_build::ProjectRoot::Found(root) = dagayn_build::find_project_root(&cwd) else {
            return None;
        };
        return realpath(root.to_str()?).map(Ok);
    };
    let resolved = realpath(&named)?;
    let path = Path::new(&resolved);
    if !path.is_dir() {
        return Some(Err(format!(
            "repo_root is not an existing directory: {resolved}"
        )));
    }
    if !crate::pypath::is_project_root(path)? {
        return Some(Err(format!(
            "repo_root does not look like a project root (no .git or .dagayn/graph.db found): {resolved}"
        )));
    }
    Some(Ok(resolved))
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `_identifier_spans`, in characters.
fn identifier_spans(line: &[char], name: &[char]) -> Vec<usize> {
    let mut spans = Vec::new();
    let mut quote: Option<char> = None;
    let mut index = 0;
    let length = line.len();
    while index < length {
        let c = line[index];
        if let Some(open) = quote {
            if c == '\\' {
                index += 2;
                continue;
            }
            if c == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            index += 1;
            continue;
        }
        if line[index..].starts_with(name) {
            let before_ok = index == 0 || !is_ident(line[index - 1]);
            let after = index + name.len();
            let after_ok = after >= length || !is_ident(line[after]);
            if before_ok && after_ok {
                spans.push(index);
                index = after;
                continue;
            }
        }
        index += 1;
    }
    spans
}

/// `_apply_edit`: the new content, or the reason the edit was skipped.
fn apply_edit(content: &str, edit: &Edit) -> Result<String, &'static str> {
    let Some(target) = edit.line else {
        return Err("no_line_recorded");
    };
    let mut lines = splitlines(content);
    let index = target - 1;
    if index < 0 || index as usize >= lines.len() {
        return Err("line_out_of_range");
    }
    let index = index as usize;
    // Python's scan never advances past a place an empty name fits, and
    // finds no span on a line it does not fit; both are reported as no match.
    if edit.old.is_empty() {
        return Err("line_no_longer_matches");
    }
    let mut line: Vec<char> = lines[index].chars().collect();
    let name: Vec<char> = edit.old.chars().collect();
    let spans = identifier_spans(&line, &name);
    if spans.is_empty() {
        return Err("line_no_longer_matches");
    }
    let replacement: Vec<char> = edit.new.chars().collect();
    for start in spans.into_iter().rev() {
        line.splice(start..start + name.len(), replacement.iter().copied());
    }
    lines[index] = line.into_iter().collect();
    Ok(lines.concat())
}

struct Planned {
    file: String,
    path: PathBuf,
    original: String,
    content: String,
    count: usize,
}

fn error(message: String) -> Payload {
    Ordered::default()
        .put("status", "error")
        .put("error", message)
        .into_payload()
}

pub(crate) fn apply_refactor(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, &["refactor_id", "repo_root", "dry_run"])?;
    let refactor_id = args.string("refactor_id")?;
    let dry_run = match arguments.get("dry_run") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return None,
    };
    let root = match apply_root(context, args.optional_string("repo_root")?)? {
        Ok(root) => root,
        Err(message) => return Some(error(message)),
    };

    pending::cleanup_expired();
    let Some(raw) = pending::get(refactor_id) else {
        return Some(error(format!(
            "Refactor '{refactor_id}' not found or expired."
        )));
    };
    let preview: Value = serde_json::from_str(&raw).ok()?;
    let created = preview.get("created_at")?.as_f64()?;
    if pending::now() - created > pending::EXPIRY_SECONDS {
        pending::remove(refactor_id);
        return Some(error(format!("Refactor '{refactor_id}' has expired.")));
    }
    let edits: Vec<Edit> = match preview.get("edits") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(Edit::parse).collect::<Option<_>>()?,
        Some(_) => return None,
    };
    if edits.is_empty() {
        let out = Ordered::default().put("status", "ok");
        let out = if dry_run {
            out.put("dry_run", true)
                .put("applied", 0)
                .put("files_modified", json!([]))
                .put("edits_applied", 0)
                .put("would_modify", json!([]))
                .put("diffs", json!({}))
        } else {
            out.put("applied", 0)
                .put("files_modified", json!([]))
                .put("edits_applied", 0)
        };
        return Some(out.into_payload());
    }

    // `_validate_edit_paths`: every path inside the root, before any read.
    let mut resolved = Vec::with_capacity(edits.len());
    for edit in &edits {
        let path = resolve(&edit.file, &root)?;
        if !path.starts_with(Path::new(&root)) {
            return Some(error(format!(
                "Edit path '{}' is outside repo root.",
                edit.file
            )));
        }
        resolved.push(path);
    }

    // `_plan_edits`, files in first-edit order.
    let mut files: Vec<(&str, PathBuf, Vec<&Edit>)> = Vec::new();
    for (edit, path) in edits.iter().zip(resolved) {
        match files.iter_mut().find(|(file, _, _)| *file == edit.file) {
            Some((_, _, group)) => group.push(edit),
            None => files.push((&edit.file, path, vec![edit])),
        }
    }
    let mut planned: Vec<Planned> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for (file, path, group) in files {
        if !path.is_file() {
            skipped.extend(group.iter().map(|edit| edit.skipped("file_not_found")));
            continue;
        }
        let Some(original) = read_text(&path) else {
            skipped.extend(group.iter().map(|edit| edit.skipped("file_unreadable")));
            continue;
        };
        let mut content = original.clone();
        let mut count = 0;
        for edit in group {
            match apply_edit(&content, edit) {
                Ok(next) => {
                    content = next;
                    count += 1;
                }
                Err(reason) => skipped.push(edit.skipped(reason)),
            }
        }
        if count > 0 {
            planned.push(Planned {
                file: file.to_string(),
                path,
                original,
                content,
                count,
            });
        }
    }
    let status = if skipped.is_empty() { "ok" } else { "partial" };

    if dry_run {
        let diffs: Map<String, Value> = planned
            .iter()
            .map(|plan| {
                let diff = unified_diff(
                    &splitlines(&plan.original),
                    &splitlines(&plan.content),
                    &format!("a/{}", plan.file),
                    &format!("b/{}", plan.file),
                    3,
                );
                (plan.file.clone(), json!(diff))
            })
            .collect();
        let mut would_modify: Vec<&str> = planned.iter().map(|plan| plan.file.as_str()).collect();
        would_modify.sort_unstable();
        return Some(
            Ordered::default()
                .put("status", status)
                .put("dry_run", true)
                .put("applied", 0)
                .put(
                    "edits_applied",
                    planned.iter().map(|plan| plan.count).sum::<usize>(),
                )
                .put("edits_skipped", skipped.len())
                .put("skipped", Value::Array(skipped))
                .put("would_modify", json!(would_modify))
                .put("files_modified", json!([]))
                .put("diffs", Value::Object(diffs))
                .into_payload(),
        );
    }

    // `_write_planned_edits`: a file that cannot be written is left out.
    let mut applied = 0;
    let mut modified: Vec<String> = Vec::new();
    for plan in &planned {
        if std::fs::write(&plan.path, &plan.content).is_ok() {
            applied += plan.count;
            let name = plan.path.to_string_lossy().into_owned();
            if !modified.contains(&name) {
                modified.push(name);
            }
        }
    }
    modified.sort_unstable();
    pending::remove(refactor_id);
    Some(
        Ordered::default()
            .put("status", status)
            .put("applied", applied)
            .put("files_modified", json!(modified))
            .put("edits_applied", applied)
            .put("edits_skipped", skipped.len())
            .put("skipped", Value::Array(skipped))
            .into_payload(),
    )
}

#[cfg(test)]
mod tests {
    use super::identifier_spans;

    fn spans(line: &str, name: &str) -> Vec<usize> {
        let line: Vec<char> = line.chars().collect();
        let name: Vec<char> = name.chars().collect();
        identifier_spans(&line, &name)
    }

    #[test]
    fn spans_skip_quoted_text_and_partial_identifiers() {
        assert_eq!(spans("foo(foo_bar, 'foo', foo)", "foo"), [0, 20]);
        assert_eq!(spans("x = \"a\\\"foo\" + foo", "foo"), [15]);
        assert!(spans("afoo", "foo").is_empty());
    }
}
