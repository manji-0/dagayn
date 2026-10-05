//! Parser quality probe: parses a list of files, optionally with mutated
//! copies, and reports panics, invariant violations and slow files.
//!
//! ```text
//! cargo run --release -p dagayn-parser --example quality_probe -- FILE_LIST [--mutate N]
//! ```
//!
//! FILE_LIST holds one path per line. Each file is parsed with its real path so
//! language detection matches production. With `--mutate N`, N mutated copies
//! of every file (truncation, deleted or duplicated spans, inserted bytes,
//! including cuts inside UTF-8 sequences) are parsed as well; mutations only
//! check for panics and invariants, not for meaningful output.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dagayn_parser::{EdgeKind, NodeKind, ParsedEdge, ParsedNode, RustOwnedParser};

#[derive(Default)]
struct Stats {
    files: usize,
    bytes: usize,
    nodes: usize,
    edges: usize,
    elapsed: Duration,
    tree_sitter: Duration,
    panics: Vec<String>,
    violations: BTreeMap<&'static str, (usize, Vec<String>)>,
}

impl Stats {
    fn violation(&mut self, kind: &'static str, example: String) {
        let entry = self.violations.entry(kind).or_default();
        entry.0 += 1;
        if entry.1.len() < 4 {
            entry.1.push(example);
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const INSERTS: &[&[u8]] = &[
    b"{",
    b"}",
    b"(",
    b")",
    b"\"",
    b"'",
    b"`",
    b"/*",
    b"\n",
    b"\\",
    b"<",
    b"\xe3\x81\x82",
    b"\xf0\x9f",
    b"\0",
    b"#",
    b"--",
    b"end",
    b"\r\n",
];

fn mutate(source: &[u8], rng: &mut Rng) -> Vec<u8> {
    let len = source.len();
    let at = rng.below(len + 1);
    match rng.below(4) {
        0 => source[..at].to_vec(),
        1 => {
            let end = (at + 1 + rng.below(64)).min(len);
            [&source[..at], &source[end..]].concat()
        }
        2 => {
            let end = (at + 1 + rng.below(256)).min(len);
            [&source[..end], &source[at..end], &source[end..]].concat()
        }
        _ => {
            let insert = INSERTS[rng.below(INSERTS.len())];
            [&source[..at], insert, &source[at..]].concat()
        }
    }
}

fn qualified(node: &ParsedNode) -> String {
    match (&node.kind, &node.parent_name) {
        (NodeKind::File, _) => node.file_path.to_string(),
        (_, Some(parent)) => format!("{}::{}.{}", node.file_path, parent, node.name),
        (_, None) => format!("{}::{}", node.file_path, node.name),
    }
}

fn check(path: &str, source: &[u8], nodes: &[ParsedNode], edges: &[ParsedEdge], stats: &mut Stats) {
    let lines = source.iter().filter(|b| **b == b'\n').count() as i64 + 1;
    let mut seen: HashMap<(NodeKind, String), i64> = HashMap::new();
    let mut qualified_names: HashSet<String> = HashSet::new();
    let mut files = 0;
    for node in nodes {
        let q = qualified(node);
        let at = format!("{path}:{} {:?} {:?}", node.line_start, node.kind, node.name);
        if node.kind == NodeKind::File {
            files += 1;
        }
        if node.file_path.as_str() != path {
            stats.violation(
                "node_file_path_mismatch",
                format!("{at} -> {}", node.file_path),
            );
        }
        if node.name.trim().is_empty() {
            stats.violation("node_empty_name", at.clone());
        } else if node.kind != NodeKind::File
            && node.kind != NodeKind::DocSection
            && node.kind != NodeKind::DocBody
            && node.name.contains(['\n', '\r'])
        {
            stats.violation("node_name_has_newline", at.clone());
        }
        if node.line_start < 1 || node.line_start > node.line_end {
            stats.violation(
                "node_bad_line_range",
                format!("{at} {}..{}", node.line_start, node.line_end),
            );
        }
        if node.line_end > lines {
            stats.violation(
                "node_line_past_eof",
                format!("{at} end={} lines={lines}", node.line_end),
            );
        }
        if let Some(previous) = seen.insert((node.kind, q.clone()), node.line_start) {
            stats.violation(
                "node_duplicate_qualified",
                format!("{at} (also line {previous})"),
            );
        }
        qualified_names.insert(q);
    }
    if files != 1 {
        stats.violation("file_node_count", format!("{path} has {files} File nodes"));
    }
    for edge in edges {
        let at = format!(
            "{path}:{} {:?} {} -> {}",
            edge.line, edge.kind, edge.source, edge.target
        );
        if edge.file_path.as_str() != path {
            stats.violation(
                "edge_file_path_mismatch",
                format!("{at} ({})", edge.file_path),
            );
        }
        if edge.source.trim().is_empty() || edge.target.trim().is_empty() {
            stats.violation("edge_empty_endpoint", at.clone());
        }
        if edge.target.contains(['\n', '\r']) {
            stats.violation("edge_target_has_newline", at.clone());
        }
        if edge.line < 1 || edge.line > lines {
            stats.violation("edge_line_out_of_file", format!("{at} lines={lines}"));
        }
        // TESTED_BY points from the tested symbol, which may be an unresolved name.
        if edge.kind != EdgeKind::TestedBy
            && edge.source != path
            && !qualified_names.contains(&edge.source)
        {
            stats.violation("edge_source_not_a_node", at.clone());
        }
    }
}

fn raw_language(ext: &str) -> Option<tree_sitter::Language> {
    use dagayn_grammars::*;
    Some(match ext {
        "rs" => rust_language(),
        "js" | "mjs" | "cjs" | "jsx" => javascript_language(),
        "ts" | "mts" | "cts" => typescript_language(),
        "tsx" => tsx_language(),
        "go" => go_language(),
        "java" => java_language(),
        "rb" => ruby_language(),
        "cs" => csharp_language(),
        "php" => php_language(),
        "kt" | "kts" => kotlin_language(),
        "scala" => scala_language(),
        "dart" => dart_language(),
        "lua" => lua_language(),
        "c" | "h" => c_language(),
        "cc" | "cpp" | "hpp" => cpp_language(),
        "m" => objc_language(),
        "ex" | "exs" => elixir_language(),
        "gd" => gdscript_language(),
        "r" => r_language(),
        "jl" => julia_language(),
        "pl" | "pm" => perl_language(),
        "zig" => zig_language(),
        "swift" => swift_language(),
        "sh" | "bash" => bash_language(),
        "md" => markdown_language(),
        "tf" => terraform_language(),
        _ => return None,
    })
}

fn raw_parse_time(ext: &str, source: &[u8]) -> Duration {
    let Some(language) = raw_language(ext) else {
        return Duration::ZERO;
    };
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).expect("language");
    let started = Instant::now();
    let _tree = parser.parse(source, None);
    started.elapsed()
}

fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_ascii_lowercase(),
        None => name.to_string(),
    }
}

/// The nearest ancestor holding `.git`, as production passes the repo root and
/// a repo-relative path (JavaScript module resolution reads sibling files).
fn split_repo(path: &str) -> (Option<PathBuf>, String) {
    let full = Path::new(path);
    for ancestor in full.ancestors().skip(1) {
        if ancestor.join(".git").exists() {
            let rel = full.strip_prefix(ancestor).unwrap_or(full);
            return (
                Some(ancestor.to_path_buf()),
                rel.to_string_lossy().into_owned(),
            );
        }
    }
    (None, path.to_string())
}

fn run(
    parser: &mut RustOwnedParser,
    path: &str,
    source: &[u8],
    label: &str,
    stats: &mut Stats,
) -> Option<Duration> {
    let (root, rel) = split_repo(path);
    let started = Instant::now();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        parser.parse_file_in_repo(root.as_deref(), &rel, source)
    }));
    let elapsed = started.elapsed();
    match outcome {
        Ok((nodes, edges)) => {
            stats.nodes += nodes.len();
            stats.edges += edges.len();
            check(&rel, source, &nodes, &edges, stats);
            Some(elapsed)
        }
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            stats.panics.push(format!("{label} {path}: {message}"));
            *parser = RustOwnedParser::new();
            None
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list = std::fs::read_to_string(&args[1]).expect("file list");
    let mutations: usize = args
        .iter()
        .position(|a| a == "--mutate")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    panic::set_hook(Box::new(|_| {}));

    let mut parser = RustOwnedParser::new();
    let mut by_ext: BTreeMap<String, Stats> = BTreeMap::new();
    let mut slow: Vec<(Duration, usize, String)> = Vec::new();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for path in list.lines().filter(|l| !l.is_empty()) {
        let Ok(source) = std::fs::read(path) else {
            continue;
        };
        let ext = extension(path);
        let raw = raw_parse_time(&ext, &source);
        let stats = by_ext.entry(ext).or_default();
        stats.tree_sitter += raw;
        stats.files += 1;
        stats.bytes += source.len();
        if let Some(elapsed) = run(&mut parser, path, &source, "original", stats) {
            stats.elapsed += elapsed;
            slow.push((
                elapsed,
                source.len(),
                format!("{path} (tree-sitter {:.1}ms)", raw.as_secs_f64() * 1e3),
            ));
        }
        for _ in 0..mutations {
            let mutated = mutate(&source, &mut rng);
            run(&mut parser, path, &mutated, "mutated", stats);
        }
    }

    println!(
        "{:<8} {:>6} {:>9} {:>8} {:>8} {:>9} {:>6} {:>6} {:>5}",
        "ext", "files", "KB", "nodes", "edges", "MB/s", "panic", "viol", "ts%"
    );
    println!("(ts% = share of time spent in tree-sitter itself)");
    for (ext, s) in &by_ext {
        let mbps = s.bytes as f64 / 1e6 / s.elapsed.as_secs_f64().max(1e-9);
        let violations: usize = s.violations.values().map(|v| v.0).sum();
        println!(
            "{ext:<8} {:>6} {:>9} {:>8} {:>8} {mbps:>9.2} {:>6} {violations:>6} {:>5.0}",
            s.files,
            s.bytes / 1024,
            s.nodes,
            s.edges,
            s.panics.len(),
            100.0 * s.tree_sitter.as_secs_f64() / s.elapsed.as_secs_f64().max(1e-9)
        );
    }
    for (ext, s) in &by_ext {
        for panic in s.panics.iter().take(5) {
            println!("PANIC [{ext}] {panic}");
        }
        for (kind, (count, examples)) in &s.violations {
            println!("VIOLATION [{ext}] {kind} x{count}");
            for example in examples {
                println!("    {example}");
            }
        }
    }
    slow.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (elapsed, bytes, path) in slow.iter().take(15) {
        println!(
            "SLOW {:>8.1}ms {:>8}B {path}",
            elapsed.as_secs_f64() * 1e3,
            bytes
        );
    }
}
