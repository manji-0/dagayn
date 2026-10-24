//! `get_docs_section_tool` (`dagayn.tools.docs.get_docs_section`).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo};

const REFERENCE: &str = "docs/LLM-OPTIMIZED-REFERENCE.md";

pub(crate) fn get_docs_section(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(arguments, &["section_name", "repo_root", "max_chars"])?;
    let section = args.string("section_name")?;
    let max_chars = args.integer("max_chars", 4000)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    // Python opens the store to learn the root (and to report `_repo`); one
    // it would create or refuse is left to it.
    let graph = open_graph(&root)?;
    let repo = graph.repo_context();
    drop(graph);
    let reply = docs_section(
        vec![root.path.clone()],
        context.package_root.as_deref(),
        section,
        max_chars,
    )?;
    Some(reply.put("_repo", repo).into_payload())
}

/// `get_docs_section`'s answer from `search_roots`' reference files, then the
/// package's (`package_root`), without `_repo`: what the tool reports, and
/// what the Python tool reports when it has no graph.
pub(crate) fn docs_section(
    mut search_roots: Vec<PathBuf>,
    package_root: Option<&Path>,
    section: &str,
    max_chars: i64,
) -> Option<Ordered> {
    if let Some(package_root) = package_root
        && package_root.join(REFERENCE).exists()
        && !search_roots.iter().any(|known| known == package_root)
    {
        search_roots.push(package_root.to_path_buf());
    }
    let pattern = regex::Regex::new(&format!(
        r#"(?is)<section name="{}">(.*?)</section>"#,
        regex::escape(section)
    ))
    .ok()?;
    let names = regex::Regex::new(r#"(?i)<section name="([^"]*)">"#).ok()?;
    let mut available: Vec<String> = Vec::new();
    for search_root in search_roots {
        let Ok(bytes) = std::fs::read(search_root.join(REFERENCE)) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for name in names
            .captures_iter(&text)
            .filter_map(|captures| captures.get(1))
        {
            if !available.iter().any(|known| known == name.as_str()) {
                available.push(name.as_str().to_string());
            }
        }
        let Some(found) = pattern.captures(&text).and_then(|captures| captures.get(1)) else {
            continue;
        };
        let content = found.as_str().trim();
        let length = content.chars().count();
        // `len(content) > max_chars`, then `content[:max_chars]`: a negative
        // limit counts back from the end, as a Python slice does.
        let truncated = i64::try_from(length).ok()? > max_chars;
        let content = if truncated {
            let kept = if max_chars >= 0 {
                usize::try_from(max_chars).ok()?
            } else {
                length.saturating_sub(usize::try_from(max_chars.unsigned_abs()).ok()?)
            };
            let kept: String = content.chars().take(kept).collect();
            format!("{kept}\n... (truncated)")
        } else {
            content.to_string()
        };
        return Some(
            Ordered::default()
                .put("status", "ok")
                .put("section", section)
                .put("content", content)
                .put("truncated", truncated),
        );
    }
    Some(Ordered::default().put("status", "not_found").put(
        "error",
        format!(
            "Section '{section}' not found. Available: {}",
            available.join(", ")
        ),
    ))
}

/// `Path.read_text(encoding="utf-8", errors="replace")`: each maximal invalid
/// UTF-8 subpart reads as one U+FFFD (the rule Python's decoder and
/// `from_utf8_lossy` share), then universal newlines. `None` when the file
/// cannot be read, where Python raises.
pub(crate) fn read_text(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(decode_text(&bytes))
}

/// [`read_text`]'s decoding of `bytes`.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}
