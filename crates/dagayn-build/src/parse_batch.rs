//! Parsing batches of repository files for the native store: parallel parse with per-thread parsers, change detection against stored hashes and mtimes, and conversion of parser output to graph inputs.

use std::collections::HashMap;

use dagayn_graph::{EdgeInput, FileBatchItem, NodeInput};
use sha2::{Digest, Sha256};

pub struct CachedRustChangedFile {
    source: Vec<u8>,
    file_hash: String,
    mtime_ns: i64,
}

pub type RustStoreSummary = (usize, usize, Vec<(String, String)>);

pub type RustChangedFilesSummary = (Vec<String>, Vec<(String, String)>);

pub type RustFileBatchSummary = (Vec<FileBatchItem>, usize, usize, Vec<(String, String)>);

pub type RustChangedFileBatchSummary = (
    Vec<FileBatchItem>,
    Vec<(String, i64)>,
    usize,
    usize,
    Vec<(String, String)>,
);

pub type RustClassifiedFilesSummary = (
    Vec<(String, CachedRustChangedFile)>,
    Vec<(String, i64)>,
    Vec<(String, String)>,
);

fn parse_rust_owned_file_inputs(
    parser: &mut dagayn_parser::RustOwnedParser,
    repo_root: &std::path::Path,
    file_path: &str,
    source: &[u8],
) -> (Vec<NodeInput>, Vec<EdgeInput>) {
    let (nodes, edges) = parser.parse_file_in_repo(Some(repo_root), file_path, source);
    (
        nodes.into_iter().map(parsed_node_to_input).collect(),
        edges.into_iter().map(parsed_edge_to_input).collect(),
    )
}

/// Map `items` on the rayon pool, preserving their order.
///
/// Each chunk gets one parser, so its tree-sitter parsers and JavaScript module
/// caches stay warm across the chunk; a few chunks per thread keep the threads
/// busy when file sizes vary.
fn par_map_with_parser<I, T, F>(items: Vec<I>, f: F) -> Vec<T>
where
    I: Send,
    T: Send,
    F: Fn(&mut dagayn_parser::RustOwnedParser, I) -> T + Sync,
{
    use rayon::prelude::*;

    let chunk_len = items
        .len()
        .div_ceil(rayon::current_num_threads().max(1) * 4)
        .max(1);
    let mut chunks = Vec::new();
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        chunks.push(items.by_ref().take(chunk_len).collect::<Vec<_>>());
    }
    chunks
        .into_par_iter()
        .map(|chunk| {
            let mut parser = dagayn_parser::RustOwnedParser::new();
            chunk
                .into_iter()
                .map(|item| f(&mut parser, item))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect()
}

pub fn collect_rust_owned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
) -> RustFileBatchSummary {
    let parsed = par_map_with_parser(file_paths, |parser, file_path| {
        let full_path = repo_root.join(&file_path);
        let source = match std::fs::read(&full_path) {
            Ok(source) => source,
            Err(err) => return Err((file_path, err.to_string())),
        };
        if !dagayn_parser::rust_parser_owns_source(&file_path, &source) {
            return Err((file_path, "unsupported Rust parser path".to_string()));
        }
        let mtime_ns = file_mtime_ns(&full_path).unwrap_or(0);
        let (nodes, edges) = parse_rust_owned_file_inputs(parser, repo_root, &file_path, &source);
        Ok((file_path, nodes, edges, sha256_hex(&source), mtime_ns))
    });

    summarize_batch(parsed)
}

fn summarize_batch(parsed: Vec<Result<FileBatchItem, (String, String)>>) -> RustFileBatchSummary {
    let mut batch = Vec::new();
    let mut errors = Vec::new();
    let mut total_nodes = 0_usize;
    let mut total_edges = 0_usize;
    for result in parsed {
        match result {
            Ok(item) => {
                total_nodes += item.1.len();
                total_edges += item.2.len();
                batch.push(item);
            }
            Err(error) => errors.push(error),
        }
    }
    (batch, total_nodes, total_edges, errors)
}

/// Parse files the Rust-owned path rules do not claim (an extensionless
/// script whose shebang names a language outside them, for one).
///
/// Mirrors the Python fallback, which parses them without repository context.
pub fn collect_unowned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
) -> RustFileBatchSummary {
    let parsed = par_map_with_parser(file_paths, |parser, file_path| {
        let full_path = repo_root.join(&file_path);
        let source = match std::fs::read(&full_path) {
            Ok(source) => source,
            Err(err) => return Err((file_path, err.to_string())),
        };
        let mtime_ns = file_mtime_ns(&full_path).unwrap_or(0);
        let (nodes, edges) = parser.parse_file_in_repo(None, &file_path, &source);
        Ok((
            file_path,
            nodes.into_iter().map(parsed_node_to_input).collect(),
            edges.into_iter().map(parsed_edge_to_input).collect(),
            sha256_hex(&source),
            mtime_ns,
        ))
    });
    summarize_batch(parsed)
}

pub fn collect_changed_rust_owned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
    file_meta: &HashMap<String, (String, i64)>,
    mut cached: HashMap<String, CachedRustChangedFile>,
) -> RustChangedFileBatchSummary {
    let inputs = file_paths
        .into_iter()
        .map(|file_path| {
            let entry = cached.remove(&file_path);
            (file_path, entry)
        })
        .collect::<Vec<_>>();
    let parsed = par_map_with_parser(inputs, |parser, (file_path, entry)| {
        let mut mtime_updates = Vec::new();
        let mut errors = Vec::new();
        let cached_entry = entry.and_then(|entry| {
            file_mtime_ns(&repo_root.join(&file_path))
                .ok()
                .filter(|mtime_ns| *mtime_ns == entry.mtime_ns)
                .map(|_| (entry.source, entry.file_hash, entry.mtime_ns))
        });
        let item = match cached_entry.or_else(|| {
            changed_rust_owned_file_source(
                repo_root,
                &file_path,
                file_meta,
                &mut mtime_updates,
                &mut errors,
            )
        }) {
            Some((source, _, _))
                if !dagayn_parser::rust_parser_owns_source(&file_path, &source) =>
            {
                errors.push((file_path, "unsupported Rust parser path".to_string()));
                None
            }
            Some((source, file_hash, mtime_ns)) => {
                let (nodes, edges) =
                    parse_rust_owned_file_inputs(parser, repo_root, &file_path, &source);
                Some((file_path, nodes, edges, file_hash, mtime_ns))
            }
            None => None,
        };
        (item, mtime_updates, errors)
    });

    let mut batch = Vec::new();
    let mut mtime_updates = Vec::new();
    let mut errors = Vec::new();
    let mut total_nodes = 0_usize;
    let mut total_edges = 0_usize;
    for (item, item_mtime_updates, item_errors) in parsed {
        if let Some(item) = item {
            total_nodes += item.1.len();
            total_edges += item.2.len();
            batch.push(item);
        }
        mtime_updates.extend(item_mtime_updates);
        errors.extend(item_errors);
    }
    (batch, mtime_updates, total_nodes, total_edges, errors)
}

pub fn classify_changed_rust_owned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
    file_meta: &HashMap<String, (String, i64)>,
) -> RustClassifiedFilesSummary {
    let mut changed_files = Vec::new();
    let mut mtime_updates = Vec::new();
    let mut errors = Vec::new();

    for file_path in file_paths {
        if let Some((source, file_hash, mtime_ns)) = changed_rust_owned_file_source(
            repo_root,
            &file_path,
            file_meta,
            &mut mtime_updates,
            &mut errors,
        ) {
            if !dagayn_parser::rust_parser_owns_source(&file_path, &source) {
                errors.push((file_path, "unsupported Rust parser path".to_string()));
                continue;
            }
            changed_files.push((
                file_path,
                CachedRustChangedFile {
                    source,
                    file_hash,
                    mtime_ns,
                },
            ));
        }
    }

    (changed_files, mtime_updates, errors)
}

fn changed_rust_owned_file_source(
    repo_root: &std::path::Path,
    file_path: &str,
    file_meta: &HashMap<String, (String, i64)>,
    mtime_updates: &mut Vec<(String, i64)>,
    errors: &mut Vec<(String, String)>,
) -> Option<(Vec<u8>, String, i64)> {
    let full_path = repo_root.join(file_path);
    let mtime_ns = match file_mtime_ns(&full_path) {
        Ok(mtime_ns) => mtime_ns,
        Err(err) => {
            errors.push((file_path.to_string(), err.to_string()));
            return None;
        }
    };
    // No mtime short-circuit here: these paths come from `git diff`/`git
    // status`, which has already reported them changed. An mtime can be equal
    // for a file whose bytes differ (`cp -p`/`rsync -a`/`tar x` restore it, and
    // coarse filesystem granularity hides two writes in one tick), and skipping
    // on that basis left the file un-indexed forever, because the stored hash
    // stayed stale too. The hash comparison below still avoids re-parsing when
    // the content really is unchanged.
    let source = match std::fs::read(&full_path) {
        Ok(source) => source,
        Err(err) => {
            errors.push((file_path.to_string(), err.to_string()));
            return None;
        }
    };
    let file_hash = sha256_hex(&source);
    if file_meta
        .get(file_path)
        .is_some_and(|(stored_hash, _)| *stored_hash == file_hash)
    {
        mtime_updates.push((file_path.to_string(), mtime_ns));
        return None;
    }
    Some((source, file_hash, mtime_ns))
}

fn file_mtime_ns(path: &std::path::Path) -> std::io::Result<i64> {
    let modified = std::fs::metadata(path)?.modified()?;
    // A pre-1970 mtime makes `duration_since(UNIX_EPOCH)` fail. Returning 0
    // there disagreed with Python's `st_mtime_ns`, which returns the negative
    // offset — so the same file got different metadata depending on which
    // backend indexed it, and the mtime fast paths compare exactly that value.
    Ok(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().min(i64::MAX as u128) as i64,
        Err(err) => -(err.duration().as_nanos().min(i64::MAX as u128) as i64),
    })
}

fn parsed_node_to_input(node: dagayn_parser::ParsedNode) -> NodeInput {
    NodeInput {
        kind: node.kind.as_str().to_string(),
        name: node.name,
        file_path: node.file_path.to_string(),
        line_start: node.line_start,
        line_end: node.line_end,
        language: node.language,
        parent_name: node.parent_name,
        params: node.params,
        return_type: node.return_type,
        modifiers: node.modifiers,
        is_test: node.is_test,
        extra: node.extra,
    }
}

fn parsed_edge_to_input(edge: dagayn_parser::ParsedEdge) -> EdgeInput {
    EdgeInput {
        kind: edge.kind.as_str().to_string(),
        source: edge.source,
        target: edge.target,
        file_path: edge.file_path.to_string(),
        line: edge.line,
        extra: edge.extra,
    }
}

fn sha256_hex(source: &[u8]) -> String {
    hex_digest(Sha256::digest(source))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{:02x}", *byte);
    }
    out
}
