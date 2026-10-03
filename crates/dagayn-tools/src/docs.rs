//! `get_docs_section_tool` (`dagayn.tools.docs.get_docs_section`).

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::{Args, Context, Ordered, Payload, explicit_repo, open_graph};

const REFERENCE: &str = "docs/LLM-OPTIMIZED-REFERENCE.md";

pub(crate) fn get_docs_section(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(arguments, &["section_name", "repo_root", "max_chars"])?;
    let section = args.string("section_name")?;
    let max_chars = usize::try_from(args.integer("max_chars", 4000)?)
        .ok()
        .filter(|max| *max > 0)?;
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    // Python opens the store to learn the root (and to report `_repo`); one
    // it would create or refuse is left to it.
    let graph = open_graph(&root)?;
    let repo = graph.repo_context();
    drop(graph);

    let mut search_roots: Vec<PathBuf> = vec![root];
    if let Some(package_root) = &context.package_root
        && package_root.join(REFERENCE).exists()
        && !search_roots.contains(package_root)
    {
        search_roots.push(package_root.clone());
    }
    let pattern = regex::Regex::new(&format!(
        r#"(?is)<section name="{}">(.*?)</section>"#,
        regex::escape(section)
    ))
    .ok()?;
    for search_root in search_roots {
        let Ok(bytes) = std::fs::read(search_root.join(REFERENCE)) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let Some(found) = pattern.captures(&text).and_then(|captures| captures.get(1)) else {
            continue;
        };
        let content = found.as_str().trim();
        let truncated = content.chars().count() > max_chars;
        let content = if truncated {
            let kept: String = content.chars().take(max_chars).collect();
            format!("{kept}\n... (truncated)")
        } else {
            content.to_string()
        };
        return Some(
            Ordered::default()
                .put("status", "ok")
                .put("section", section)
                .put("content", content)
                .put("truncated", truncated)
                .put("_repo", repo)
                .into_payload(),
        );
    }
    // Not found: the error listing the sections is Python's.
    None
}
