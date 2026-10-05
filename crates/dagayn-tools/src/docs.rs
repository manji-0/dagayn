//! `get_docs_section_tool` (`dagayn.tools.docs.get_docs_section`).

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo};

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
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    // Python opens the store to learn the root (and to report `_repo`); one
    // it would create or refuse is left to it.
    let graph = open_graph(&root)?;
    let repo = graph.repo_context();
    drop(graph);

    let mut search_roots: Vec<PathBuf> = vec![root.path.clone()];
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

/// `Path.read_text(errors="replace")`: universal newlines; `None` for bytes
/// that are not UTF-8, whose replacement is Python's.
fn read_text(path: &std::path::Path) -> Option<String> {
    let text = String::from_utf8(std::fs::read(path).ok()?).ok()?;
    Some(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `dagayn.wiki._slugify` for an ASCII name.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut gap = false;
    for c in name.to_ascii_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            gap = false;
            slug.push(c);
        } else {
            gap = true;
        }
    }
    slug.truncate(80);
    if slug.is_empty() {
        "unnamed".to_string()
    } else {
        slug
    }
}

/// `get_wiki_page_tool` (`dagayn.tools.docs.get_wiki_page_func`).
pub(crate) fn get_wiki_page(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, &["community_name", "repo_root"])?;
    let name = args.string("community_name")?;
    // Python's `lower()` and `\w`-free regex agree with ASCII folding only.
    if !name.is_ascii() || name.contains('\0') {
        return None;
    }
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let repo = graph.repo_context();
    // `get_data_dir(root) / "wiki"`: next to the graph, wherever it lives.
    let wiki = graph.db_path.parent()?.join("wiki");
    drop(graph);

    let slugged = wiki.join(format!("{}.md", slugify(name)));
    let content = if slugged.is_file() {
        Some(read_text(&slugged)?)
    } else {
        // The exact file name, inside the wiki directory only.
        match (wiki.join(name).canonicalize(), wiki.canonicalize()) {
            (Ok(exact), Ok(base)) if exact.is_file() && exact.starts_with(&base) => {
                Some(read_text(&exact)?)
            }
            _ => None,
        }
    };
    let out = match content {
        None => Ordered::default()
            .put("status", "not_found")
            .put(
                "summary",
                format!(
                    "No wiki page found for '{name}'. Run generate_wiki_tool first to build the wiki."
                ),
            )
            .put(
                "next_tool_suggestions",
                serde_json::json!(["generate_wiki_tool -- build wiki pages from communities"]),
            ),
        Some(content) => Ordered::default()
            .put("status", "ok")
            .put(
                "summary",
                format!(
                    "Wiki page for '{name}' ({} chars)",
                    content.chars().count()
                ),
            )
            .put("content", content),
    };
    Some(out.put("_repo", repo).into_payload())
}
