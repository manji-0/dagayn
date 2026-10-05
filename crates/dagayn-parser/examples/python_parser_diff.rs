//! Differential test of the two Python extractors: parses every Python
//! file and notebook under a directory with the tree-sitter extractor and
//! with the Ruff one, and writes one JSON line per file whose nodes or
//! edges differ. With `--bench BACKEND`, parses with that backend only and
//! prints the time.
//!
//! ```text
//! cargo run --release -p dagayn-parser --example python_parser_diff -- DIR [--repo-root] [--out FILE]
//! cargo run --release -p dagayn-parser --example python_parser_diff -- DIR --bench ruff
//! ```
//!
//! With `--repo-root`, DIR is the repository root the extractors resolve
//! imports against (as `dagayn build` does); without it, files parse with
//! no root, as for a site-packages tree.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dagayn_parser::{ParsedEdge, ParsedNode, PythonParserBackend, RustOwnedParser};
use serde_json::{Value, json};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if matches!(name, ".git" | "target" | "node_modules" | ".venv") {
                continue;
            }
            collect(&path, out);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("py" | "pyi" | "ipynb")
        ) {
            out.push(path);
        }
    }
}

fn node_key(node: &ParsedNode) -> String {
    format!(
        "{}|{}|{}",
        node.kind.as_str(),
        node.parent_name.as_deref().unwrap_or(""),
        node.name
    )
}

fn edge_key(edge: &ParsedEdge) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        edge.kind.as_str(),
        edge.source,
        edge.target,
        edge.line,
        edge.extra
    )
}

/// Multiset difference `a - b` of keyed items.
fn minus<T>(a: &[T], b: &[T], key: impl Fn(&T) -> String) -> Vec<String> {
    let mut counts = BTreeMap::<String, i64>::new();
    for item in b {
        *counts.entry(key(item)).or_default() += 1;
    }
    let mut out = Vec::new();
    for item in a {
        let entry = counts.entry(key(item)).or_default();
        if *entry > 0 {
            *entry -= 1;
        } else {
            out.push(key(item));
        }
    }
    out
}

fn node_diffs(old: &[ParsedNode], new: &[ParsedNode]) -> Vec<Value> {
    let mut by_key = BTreeMap::<String, Vec<&ParsedNode>>::new();
    for node in new {
        by_key.entry(node_key(node)).or_default().push(node);
    }
    let mut out = Vec::new();
    for node in old {
        let Some(candidates) = by_key.get_mut(&node_key(node)) else {
            continue;
        };
        if candidates.is_empty() {
            continue;
        }
        let other = candidates.remove(0);
        let old_value = serde_json::to_value(node).unwrap();
        let new_value = serde_json::to_value(other).unwrap();
        let mut fields = serde_json::Map::new();
        for (field, value) in old_value.as_object().unwrap() {
            let other_value = &new_value[field];
            if value != other_value {
                fields.insert(field.clone(), json!([value, other_value]));
            }
        }
        if !fields.is_empty() {
            out.push(json!({"node": node_key(node), "fields": fields}));
        }
    }
    out
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let dir = PathBuf::from(args.first().expect("usage: python_parser_diff DIR [...]"));
    let use_root = args.iter().any(|arg| arg == "--repo-root");
    let flag = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let mut files = Vec::new();
    collect(&dir, &mut files);
    let root = use_root.then_some(dir.as_path());
    let rel = |path: &Path| {
        path.strip_prefix(&dir)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };

    if let Some(backend) = flag("--dump") {
        // One JSON line per file: the definitions (`kind|parent|name`) and
        // the CALLS edges (`source|target`) the backend finds.
        let backend = match backend.as_str() {
            "ruff" => PythonParserBackend::Ruff,
            _ => PythonParserBackend::TreeSitter,
        };
        let mut parser = RustOwnedParser::new().with_python_backend(backend);
        for path in &files {
            let Ok(source) = std::fs::read(path) else {
                continue;
            };
            let file = rel(path);
            let (nodes, edges) = parser.parse_file_in_repo(root, &file, &source);
            let calls = edges
                .iter()
                .filter(|edge| edge.kind.as_str() == "CALLS")
                .map(|edge| format!("{}|{}", edge.source, edge.target))
                .collect::<Vec<_>>();
            println!(
                "{}",
                json!({"file": file, "nodes": nodes.iter().map(node_key).collect::<Vec<_>>(), "calls": calls})
            );
        }
        return;
    }

    if let Some(backend) = flag("--bench") {
        let backend = match backend.as_str() {
            "ruff" => PythonParserBackend::Ruff,
            _ => PythonParserBackend::TreeSitter,
        };
        let sources = files
            .iter()
            .filter_map(|path| Some((rel(path), std::fs::read(path).ok()?)))
            .collect::<Vec<_>>();
        let mut parser = RustOwnedParser::new().with_python_backend(backend);
        let (mut nodes, mut edges) = (0, 0);
        let started = Instant::now();
        for (path, source) in &sources {
            let (n, e) = parser.parse_file_in_repo(root, path, source);
            nodes += n.len();
            edges += e.len();
        }
        let elapsed = started.elapsed();
        let bytes = sources
            .iter()
            .map(|(_, source)| source.len())
            .sum::<usize>();
        println!(
            "{backend:?}: {} files, {:.1} MB, {nodes} nodes, {edges} edges, {:.2} s",
            sources.len(),
            bytes as f64 / 1e6,
            elapsed.as_secs_f64()
        );
        return;
    }

    let mut out: Box<dyn Write> = match flag("--out") {
        Some(path) => Box::new(std::fs::File::create(path).expect("cannot create --out")),
        None => Box::new(std::io::stdout()),
    };
    let mut old_parser =
        RustOwnedParser::new().with_python_backend(PythonParserBackend::TreeSitter);
    let mut new_parser = RustOwnedParser::new().with_python_backend(PythonParserBackend::Ruff);
    let (mut differing, mut old_time, mut new_time) = (0, Duration::ZERO, Duration::ZERO);
    let (mut old_counts, mut new_counts) = ((0, 0), (0, 0));
    for path in &files {
        let Ok(source) = std::fs::read(path) else {
            continue;
        };
        let file = rel(path);
        let started = Instant::now();
        let (old_nodes, old_edges) = old_parser.parse_file_in_repo(root, &file, &source);
        old_time += started.elapsed();
        let started = Instant::now();
        let (new_nodes, new_edges) = new_parser.parse_file_in_repo(root, &file, &source);
        new_time += started.elapsed();
        old_counts.0 += old_nodes.len();
        old_counts.1 += old_edges.len();
        new_counts.0 += new_nodes.len();
        new_counts.1 += new_edges.len();
        let record = json!({
            "file": file,
            "syntax_errors": dagayn_parser::python_syntax_error_count(&source),
            "nodes_only_old": minus(&old_nodes, &new_nodes, node_key),
            "nodes_only_new": minus(&new_nodes, &old_nodes, node_key),
            "node_fields": node_diffs(&old_nodes, &new_nodes),
            "edges_only_old": minus(&old_edges, &new_edges, edge_key),
            "edges_only_new": minus(&new_edges, &old_edges, edge_key),
        });
        let differs = [
            "nodes_only_old",
            "nodes_only_new",
            "node_fields",
            "edges_only_old",
            "edges_only_new",
        ]
        .iter()
        .any(|field| {
            record[field]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        });
        if differs {
            differing += 1;
            writeln!(out, "{record}").unwrap();
        }
    }
    eprintln!(
        "{} files, {differing} differ; tree-sitter {} nodes {} edges {:.2} s; ruff {} nodes {} edges {:.2} s",
        files.len(),
        old_counts.0,
        old_counts.1,
        old_time.as_secs_f64(),
        new_counts.0,
        new_counts.1,
        new_time.as_secs_f64(),
    );
}
