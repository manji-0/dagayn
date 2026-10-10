//! The change analysis behind `detect_changes_func` (`review_tool
//! mode="changes"`) and `get_minimal_context`, ported from the retired
//! `dagayn/changes.py`: diff ranges against `base`, renames, node
//! attribution, the base revision's entities, review-priority scores, test
//! gaps, and affected flows, in the field order that module's
//! `ChangeAnalysisResult` gave them.
//!
//! A git checkout diffs `base` against the working tree; a jj workspace
//! diffs it (rebased onto `@-` when `HEAD`-relative) against the snapshot
//! commit `@` through the backing git directory; an SVN working copy reads
//! `svn diff`. `None` wherever this cannot reproduce Python.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use dagayn_build::{is_safe_git_ref, jj, svn};
use dagayn_graph::{ChangeRiskInputs, GraphEdge, GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::coverage::splitlines;
use crate::query::{edge_dict, node_dict};

/// Changed line ranges per repo-relative path, in path order.
pub(crate) type Ranges = BTreeMap<String, Vec<(i64, i64)>>;

fn git(root: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()
}

/// `parse_diff_result`'s answer: the ranges (`ok`), or `base_unresolved`.
pub(crate) enum DiffParse {
    Ranges(Ranges),
    BaseUnresolved,
}

/// `_working_tree_diff_argv`: `git diff <args> <base> --`, or in a jj
/// workspace the same diff from the resolved base to `@`; `None` when the jj
/// side cannot be resolved.
fn working_tree_diff(root: &Path, base: &str, args: &[&str]) -> Option<Command> {
    if !jj::is_jj_workspace(root) {
        let mut command = Command::new("git");
        command
            .arg("diff")
            .args(args)
            .args([base, "--"])
            .current_dir(root);
        return Some(command);
    }
    let wc = jj::working_copy(root)?;
    let resolved = jj::resolve_commit(root, base, Some(&wc))?;
    let mut command = jj::git_command(root)?;
    command
        .arg("diff")
        .args(args)
        .args([resolved.as_str(), wc.commit.as_str(), "--"]);
    Some(command)
}

/// `parse_diff_result`: `svn diff` where `.svn` exists (its range when `base`
/// is a revision range, else the local changes; always `ok`), otherwise the
/// working-tree git diff.
pub(crate) fn parse_diff(root: &Path, base: &str) -> DiffParse {
    if root.join(".svn").exists() {
        let rev_range = Some(base).filter(|rev| svn::is_safe_svn_rev(rev));
        return DiffParse::Ranges(parse_unified_diff(&svn::diff_text(root, rev_range)));
    }
    if !is_safe_git_ref(base) {
        return DiffParse::BaseUnresolved;
    }
    let Some(mut command) = working_tree_diff(root, base, &["--unified=0"]) else {
        return DiffParse::BaseUnresolved;
    };
    match command.output() {
        Ok(output) if output.status.success() => {
            DiffParse::Ranges(parse_unified_diff(&String::from_utf8_lossy(&output.stdout)))
        }
        _ => DiffParse::BaseUnresolved,
    }
}

/// `_parse_unified_diff`.
fn parse_unified_diff(text: &str) -> Ranges {
    let mut ranges = Ranges::new();
    let mut current: Option<String> = None;
    for line in splitlines(text) {
        if line.starts_with("+++") {
            current = plus_plus_path(line);
            continue;
        }
        if let (Some((start, count)), Some(file)) = (hunk_new_range(line), &current) {
            let end = if count == 0 { start } else { start + count - 1 };
            ranges.entry(file.clone()).or_default().push((start, end));
        }
    }
    ranges
}

/// `^@@ .+? \+(\d+)(?:,(\d+))? @@`: the new side's start and count.
fn hunk_new_range(line: &str) -> Option<(i64, i64)> {
    let rest = line.strip_prefix("@@ ")?;
    // `.+?` takes at least one character, then the earliest ` +` that the
    // rest of the pattern accepts.
    let mut from = rest.chars().next()?.len_utf8();
    while let Some(offset) = rest[from..].find(" +") {
        let at = from + offset;
        if let Some(found) = new_range_at(&rest[at + 2..]) {
            return Some(found);
        }
        from = at + 1;
    }
    None
}

fn new_range_at(text: &str) -> Option<(i64, i64)> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let start_len = digits(text);
    if start_len == 0 {
        return None;
    }
    let start: i64 = text[..start_len].parse().ok()?;
    let mut rest = &text[start_len..];
    let mut count = 1;
    if let Some(after) = rest.strip_prefix(',') {
        let count_len = digits(after);
        if count_len > 0 {
            count = after[..count_len].parse().ok()?;
            rest = &after[count_len..];
        }
    }
    rest.starts_with(" @@").then_some((start, count))
}

/// `_parse_plus_plus_path`.
fn plus_plus_path(line: &str) -> Option<String> {
    let raw = line.chars().skip(4).collect::<String>();
    let raw = raw.trim();
    if raw.is_empty() || raw == "/dev/null" {
        return None;
    }
    let mut path = decode_git_quoted_path(raw);
    if let Some(index) = path.find('\t') {
        path.truncate(index);
    }
    path.strip_prefix("b/").map(str::to_string)
}

/// `_decode_git_quoted_path`.
fn decode_git_quoted_path(text: &str) -> String {
    if !(text.len() >= 2 && text.starts_with('"') && text.ends_with('"')) {
        return text.to_string();
    }
    let inner: Vec<char> = text[1..text.len() - 1].chars().collect();
    let mut out: Vec<u8> = Vec::new();
    let mut index = 0;
    while index < inner.len() {
        let c = inner[index];
        if c != '\\' {
            let mut buffer = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            index += 1;
            continue;
        }
        index += 1;
        let Some(&escape) = inner.get(index) else {
            break;
        };
        match escape {
            '0'..='7' => {
                let mut octal = String::from(escape);
                index += 1;
                for _ in 0..2 {
                    match inner.get(index) {
                        Some(&digit @ '0'..='7') => {
                            octal.push(digit);
                            index += 1;
                        }
                        _ => break,
                    }
                }
                out.push(u8::from_str_radix(&octal, 8).unwrap_or(0));
            }
            'n' => {
                out.push(b'\n');
                index += 1;
            }
            't' => {
                out.push(b'\t');
                index += 1;
            }
            'r' => {
                out.push(b'\r');
                index += 1;
            }
            other => {
                let mut buffer = [0; 4];
                out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `resolve_git_renames`: `{new_path: old_path}`.
fn resolve_git_renames(root: &Path, base: &str) -> HashMap<String, String> {
    let mut renames = HashMap::new();
    if !is_safe_git_ref(base) {
        return renames;
    }
    let Some(output) = working_tree_diff(root, base, &["--name-status", "-M"])
        .and_then(|mut command| command.output().ok())
    else {
        return renames;
    };
    if !output.status.success() {
        return renames;
    }
    for line in splitlines(&String::from_utf8_lossy(&output.stdout)) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 && parts[0].starts_with('R') {
            renames.insert(parts[2].to_string(), parts[1].to_string());
        }
    }
    renames
}

/// `PurePosixPath(path).as_posix()`.
fn posix(path: &str) -> String {
    let absolute = path.starts_with('/');
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// `_repo_relative_path` with a resolved `root`.
fn repo_relative(file_path: &str, root: &Path) -> String {
    let path = Path::new(file_path);
    if path.is_absolute() {
        let normal: PathBuf = path.components().collect();
        return match normal.strip_prefix(root) {
            Ok(rel) => {
                let text = rel.to_string_lossy().into_owned();
                if text.is_empty() {
                    ".".to_string()
                } else {
                    text
                }
            }
            Err(_) => normal
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
    }
    posix(file_path)
}

/// `str(Path(root) / rel)`.
fn join(root: &Path, rel: &str) -> String {
    root.join(rel)
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

struct Context<'a> {
    store: &'a GraphStore,
    root: &'a Path,
    renames: HashMap<String, String>,
}

impl Context<'_> {
    fn renamed_from(&self, rel: &str, file_path: &str) -> Option<&String> {
        self.renames
            .get(rel)
            .filter(|old| !old.is_empty())
            .or_else(|| self.renames.get(file_path).filter(|old| !old.is_empty()))
    }

    /// `_nodes_for_changed_file`.
    fn nodes_for_changed_file(&self, file_path: &str) -> Option<Vec<GraphNode>> {
        let rel = repo_relative(file_path, self.root);
        let mut paths = vec![file_path.to_string(), rel.clone()];
        if let Some(old) = self.renames.get(&rel).filter(|old| !old.is_empty()) {
            paths.push(old.clone());
            paths.push(join(self.root, old));
        }
        let mut seen = HashSet::new();
        let mut nodes = Vec::new();
        for path in paths {
            if seen.insert(path.clone()) {
                nodes.extend(self.store.get_nodes_by_file(&path).ok()?);
            }
        }
        if nodes.is_empty() {
            for matched in self.store.get_files_matching(&rel).ok()? {
                nodes.extend(self.store.get_nodes_by_file(&matched).ok()?);
            }
        }
        Some(nodes)
    }

    /// `_graph_line_ranges_stale`.
    fn line_ranges_stale(&self, changed: &str, graph_paths: &[String]) -> Option<bool> {
        let path = if Path::new(changed).is_absolute() {
            PathBuf::from(changed)
        } else {
            self.root.join(changed)
        };
        if !path.is_file() {
            return Some(false);
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return Some(false);
        };
        let current = sha256_hex(&bytes);
        // The graph stores repo-relative paths, while the node lookup also
        // answers for absolute ones: ask for the hash under both.
        let mut keys: Vec<String> = graph_paths.to_vec();
        for graph_path in graph_paths {
            let rel = repo_relative(graph_path, self.root);
            if !keys.contains(&rel) {
                keys.push(rel);
            }
        }
        let stored = self.store.get_file_meta_for_files(&keys).ok()?;
        Some(keys.iter().any(|key| {
            stored
                .get(key)
                .is_some_and(|(hash, _)| !hash.is_empty() && *hash != current)
        }))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `map_changes_with_attribution`: the nodes and the files whose graph line
/// ranges are stale.
fn map_changes(
    ctx: &Context,
    ranges: &[(String, Vec<(i64, i64)>)],
) -> Option<(Vec<GraphNode>, Vec<String>)> {
    let mut lookup = Vec::new();
    for (file_path, _) in ranges {
        lookup.push(file_path.clone());
        let rel = repo_relative(file_path, ctx.root);
        if let Some(old) = ctx.renamed_from(&rel, file_path) {
            lookup.push(old.clone());
            lookup.push(join(ctx.root, old));
        }
    }
    let nodes_by_file = ctx.store.get_nodes_by_files(&lookup).ok()?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    let mut stale = Vec::new();
    for (file_path, file_ranges) in ranges {
        // `_resolve_graph_file_paths`.
        let rel = repo_relative(file_path, ctx.root);
        let mut candidates: Vec<String> = Vec::new();
        let mut add = |path: String| {
            if !path.is_empty() && !candidates.contains(&path) {
                candidates.push(path);
            }
        };
        if nodes_by_file
            .get(file_path)
            .is_some_and(|nodes| !nodes.is_empty())
        {
            add(file_path.clone());
        }
        if let Some(old) = ctx.renamed_from(&rel, file_path) {
            add(old.clone());
            add(join(ctx.root, old));
        }
        let graph_paths = if candidates.is_empty() {
            let matched = ctx.store.get_files_matching(&rel).ok()?;
            let mut found = Vec::new();
            for path in matched {
                if !path.is_empty() && !found.contains(&path) {
                    found.push(path);
                }
            }
            if found.is_empty() && nodes_by_file.contains_key(file_path) {
                vec![file_path.clone()]
            } else {
                found
            }
        } else {
            candidates
        };
        let nodes: Vec<&GraphNode> = graph_paths
            .iter()
            .flat_map(|path| nodes_by_file.get(path).into_iter().flatten())
            .collect();
        if nodes.is_empty() {
            continue;
        }
        if ctx.line_ranges_stale(file_path, &graph_paths)? {
            stale.push(file_path.clone());
            for node in nodes {
                if !seen.contains(&node.qualified_name)
                    && matches!(node.kind.as_str(), "Function" | "Test" | "Class")
                {
                    seen.insert(node.qualified_name.clone());
                    result.push(node.clone());
                }
            }
            continue;
        }
        for node in nodes {
            if seen.contains(&node.qualified_name) {
                continue;
            }
            if file_ranges
                .iter()
                .any(|(start, end)| node.line_start <= *end && node.line_end >= *start)
            {
                seen.insert(node.qualified_name.clone());
                result.push(node.clone());
            }
        }
    }
    Some((result, stale))
}

/// `_parser_path`: the path relative to the process's working directory.
fn parser_path(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    std::env::current_dir()
        .and_then(|cwd| cwd.canonicalize())
        .ok()
        .and_then(|cwd| resolved.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// `os.path.dirname`-like `str(Path(p).parent)`.
fn parent_text(path: &str) -> String {
    let parent = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let text = parent.to_string_lossy().into_owned();
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

/// `_normalize_path_string`.
fn normalize_path_string(value: &str, parser: &str, display: &str) -> String {
    let normalized = if value.starts_with("//") && !value.starts_with("///") {
        &value[1..]
    } else {
        value
    };
    if display == parser {
        return normalized.to_string();
    }
    if normalized == parser {
        return display.to_string();
    }
    if let Some(rest) = normalized.strip_prefix(&format!("{parser}::")) {
        return format!("{display}::{rest}");
    }
    let parser_dir = parent_text(parser);
    if parser_dir != "." {
        let display_dir = parent_text(display);
        if normalized == parser_dir {
            return display_dir;
        }
        if let Some(rest) = normalized.strip_prefix(&format!("{parser_dir}/")) {
            return format!("{display_dir}/{rest}");
        }
    }
    normalized.to_string()
}

type EdgeSignature = (String, String, String, String);

/// `_base_entity_sets`: qualified names and edge signatures of the changed
/// files at `base`.
fn base_entity_sets(
    root: &Path,
    base: &str,
    changed: &[GraphNode],
) -> (HashSet<String>, HashSet<EdgeSignature>) {
    let mut node_qns = HashSet::new();
    let mut edge_signatures = HashSet::new();
    let mut display_by_rel: Vec<(String, String)> = Vec::new();
    for node in changed {
        let rel = repo_relative(&node.file_path, root);
        if !display_by_rel.iter().any(|(known, _)| *known == rel) {
            display_by_rel.push((rel, node.file_path.clone()));
        }
    }
    // `_git_show_file`: in a jj workspace, `base` is resolved (each time in
    // Python, to the same commit) and shown from the backing git directory.
    let jj_base = (is_safe_git_ref(base) && jj::is_jj_workspace(root))
        .then(|| jj::resolve_commit(root, base, None));
    for (rel, display) in display_by_rel {
        if !is_safe_git_ref(base) {
            continue;
        }
        let output = match &jj_base {
            None => git(root, &["show", &format!("{base}:{rel}")]),
            Some(None) => None,
            Some(Some(resolved)) => jj::git_command(root).and_then(|mut command| {
                command
                    .args(["show", &format!("{resolved}:{rel}")])
                    .output()
                    .ok()
            }),
        };
        let Some(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let display_path = Path::new(&display);
        if dagayn_parser::detect_language(display_path).is_none() {
            continue;
        }
        let parser = parser_path(display_path);
        let parsed = dagayn_parser::parse_rust_owned_file_compact_json(&parser, &output.stdout);
        let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(&parsed) else {
            continue;
        };
        let text = |value: &Value| {
            value
                .as_str()
                .map(|s| normalize_path_string(s, &parser, &display))
        };
        for node in parts
            .first()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(item) = node.as_array() else {
                continue;
            };
            let kind = item.first().and_then(Value::as_str).unwrap_or("");
            let name = item.get(1).and_then(text).unwrap_or_default();
            let file_path = item.get(2).and_then(text).unwrap_or_default();
            let parent = item.get(6).and_then(Value::as_str);
            // `_node_qn`.
            node_qns.insert(match (kind, parent) {
                ("File", _) => file_path,
                (_, Some(parent)) if !parent.is_empty() => format!("{file_path}::{parent}.{name}"),
                _ => format!("{file_path}::{name}"),
            });
        }
        for edge in parts.get(1).and_then(Value::as_array).into_iter().flatten() {
            let Some(item) = edge.as_array() else {
                continue;
            };
            edge_signatures.insert((
                item.first()
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                item.get(1).and_then(text).unwrap_or_default(),
                item.get(2).and_then(text).unwrap_or_default(),
                item.get(3).and_then(text).unwrap_or_default(),
            ));
        }
    }
    (node_qns, edge_signatures)
}

fn edge_signature(edge: &GraphEdge) -> EdgeSignature {
    (
        edge.kind.clone(),
        edge.source_qualified.clone(),
        edge.target_qualified.clone(),
        edge.file_path.clone(),
    )
}

/// What `analyze_changes` hands back, with the nodes its callers reuse.
pub(crate) struct Analysis {
    /// The reply fields, in the retired `ChangeAnalysisResult`'s order.
    pub fields: Vec<(&'static str, Value)>,
}

impl Analysis {
    pub(crate) fn get(&self, key: &str) -> &Value {
        self.fields
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .unwrap_or(&Value::Null)
    }
}

/// `analyze_changes(store, abs_files, abs_ranges, repo_root, base, True,
/// SUPPLEMENTAL_TEST_DENSITY_NODE_LIMIT, "ok")`.
pub(crate) fn analyze_changes(
    store: &GraphStore,
    root: &Path,
    base: &str,
    changed_files: &[String],
    ranges: &Ranges,
) -> Option<Analysis> {
    analyze_changes_with(store, root, base, changed_files, ranges, true)
}

/// [`analyze_changes`]; `skip_ranged_files: false` is the call that leaves
/// `changed_ranges` to `analyze_changes` itself (`get_minimal_context`): its
/// ranges are keyed by relative path, so no absolute changed file matches
/// one and every changed file adds all of its nodes.
pub(crate) fn analyze_changes_with(
    store: &GraphStore,
    root: &Path,
    base: &str,
    changed_files: &[String],
    ranges: &Ranges,
    skip_ranged_files: bool,
) -> Option<Analysis> {
    let ctx = Context {
        store,
        root,
        renames: resolve_git_renames(root, base),
    };
    let abs_ranges: Vec<(String, Vec<(i64, i64)>)> = ranges
        .iter()
        .map(|(rel, file_ranges)| (join(root, rel), file_ranges.clone()))
        .collect();
    let abs_files: Vec<String> = changed_files.iter().map(|f| join(root, f)).collect();

    let mut reason_codes: Vec<&str> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut changed_nodes: Vec<GraphNode>;
    if !abs_ranges.is_empty() {
        let (mapped, stale_files) = map_changes(&ctx, &abs_ranges)?;
        changed_nodes = mapped;
        stale = stale_files;
        let ranged: HashSet<&String> = abs_ranges.iter().map(|(path, _)| path).collect();
        let mut seen: HashSet<String> = changed_nodes
            .iter()
            .map(|n| n.qualified_name.clone())
            .collect();
        for file_path in &abs_files {
            if skip_ranged_files && ranged.contains(file_path) {
                continue;
            }
            for node in ctx.nodes_for_changed_file(file_path)? {
                if seen.insert(node.qualified_name.clone()) {
                    changed_nodes.push(node);
                }
            }
        }
    } else {
        changed_nodes = Vec::new();
        let mut seen = HashSet::new();
        for file_path in &abs_files {
            for node in ctx.nodes_for_changed_file(file_path)? {
                if seen.insert(node.qualified_name.clone()) {
                    changed_nodes.push(node);
                }
            }
        }
    }

    // `_collect_unmapped_changed_files`.
    let attributed: HashSet<String> = changed_nodes
        .iter()
        .map(|node| repo_relative(&node.file_path, root))
        .collect();
    let mut unmapped = Vec::new();
    for file_path in &abs_files {
        let rel = repo_relative(file_path, root);
        let renamed = ctx.renames.get(&rel).filter(|old| !old.is_empty());
        if attributed.contains(&rel) || renamed.is_some_and(|old| attributed.contains(old)) {
            continue;
        }
        if !ctx.nodes_for_changed_file(file_path)?.is_empty() {
            continue;
        }
        unmapped.push(rel);
    }
    if !stale.is_empty() {
        reason_codes.push("stale_graph_line_ranges");
    }
    if !unmapped.is_empty() {
        reason_codes.push("unmapped_changed_files");
    }

    let funcs: Vec<&GraphNode> = changed_nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test" | "Class"))
        .collect();
    let ids: Vec<i64> = funcs.iter().map(|node| node.id).collect();
    let qns: Vec<String> = funcs
        .iter()
        .map(|node| node.qualified_name.clone())
        .collect();
    let crit = store.get_flow_criticalities_for_nodes(&ids).ok()?;
    let needing: Vec<i64> = crit
        .iter()
        .filter(|(_, values)| values.is_empty())
        .map(|(id, _)| *id)
        .collect();
    let counts = if needing.is_empty() {
        HashMap::new()
    } else {
        store.count_flow_memberships_for_nodes(&needing).ok()?
    };
    let communities = store.get_community_ids_by_node_ids(&ids).ok()?;
    let (outbound, inbound) = store.get_edges_by_endpoints(&qns).ok()?;
    let mut relevant: Vec<&GraphEdge> = Vec::new();
    let mut signatures = HashSet::new();
    for map in [&outbound, &inbound] {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for key in keys {
            for edge in &map[key] {
                if signatures.insert(edge_signature(edge)) {
                    relevant.push(edge);
                }
            }
        }
    }
    let funcs_owned: Vec<GraphNode> = funcs.iter().map(|node| (*node).clone()).collect();
    let (base_qns, base_edges) = base_entity_sets(root, base, &funcs_owned);
    let status_of = |known: bool| if known { "existing" } else { "added" };

    let callers: Vec<String> = inbound
        .values()
        .flatten()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| edge.source_qualified.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let caller_communities = if callers.is_empty() {
        HashMap::new()
    } else {
        store.get_community_ids_by_qualified_names(&callers).ok()?
    };

    let mut node_risks: Vec<Value> = Vec::new();
    for node in &funcs {
        let transitive = store
            .get_transitive_tests(&node.qualified_name, 1)
            .ok()?
            .len() as i64;
        let risk = store
            .compute_change_risk_score(ChangeRiskInputs {
                node,
                inbound_edges: inbound
                    .get(&node.qualified_name)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                flow_criticalities: crit.get(&node.id).map(Vec::as_slice).unwrap_or(&[]),
                flow_count: counts.get(&node.id).copied().unwrap_or(0),
                node_community_id: communities.get(&node.id).copied().flatten(),
                caller_community_ids: &caller_communities,
                transitive_test_count: transitive,
            })
            .ok()?;
        let mut record = node_dict(node);
        if let Some(object) = record.as_object_mut() {
            object.insert("risk_score".into(), json!(risk));
            object.insert("review_priority_score".into(), json!(risk));
            object.insert(
                "change_status".into(),
                json!(status_of(base_qns.contains(&node.qualified_name))),
            );
        }
        node_risks.push(record);
    }
    let changed_names: HashSet<&str> = changed_nodes
        .iter()
        .map(|node| node.qualified_name.as_str())
        .collect();
    let affected: Vec<Value> = store
        .get_affected_flows_annotated(&abs_files)
        .ok()?
        .into_iter()
        .map(|flow| compact_flow(flow, &changed_names))
        .collect();

    let changed_edges: Vec<Value> = relevant
        .iter()
        .map(|edge| {
            let mut record = edge_dict(edge);
            record.insert(
                "change_status".into(),
                json!(status_of(base_edges.contains(&edge_signature(edge)))),
            );
            Value::Object(record)
        })
        .collect();
    let tally = |records: &[Value]| {
        let count = |status: &str| {
            records
                .iter()
                .filter(|record| record["change_status"] == status)
                .count()
        };
        json!({"existing": count("existing"), "added": count("added"), "unknown": count("unknown")})
    };
    let node_counts = tally(&node_risks);
    let edge_counts = tally(&changed_edges);

    let stale_rel: Vec<String> = stale.iter().map(|path| repo_relative(path, root)).collect();
    let mut entity_summary = Map::new();
    entity_summary.insert("nodes".into(), node_counts);
    entity_summary.insert("edges".into(), edge_counts);
    entity_summary.insert("base".into(), json!(base));
    let fields = vec![
        ("changed_functions", Value::Array(node_risks)),
        ("change_entity_summary", Value::Object(entity_summary)),
        ("diff_parse_status", json!("ok")),
        ("unmapped_changed_files", json!(unmapped)),
        (
            "attribution",
            json!({"stale_line_range_files": stale_rel, "reason_codes": reason_codes}),
        ),
        ("affected_flows", Value::Array(affected)),
    ];
    Some(Analysis { fields })
}

/// Steps of a flow a change review keeps: the ones the change touches.
const CHANGED_STEPS_KEPT: usize = 5;

/// A flow as a change review reports it: its summary fields and the steps
/// the change touches, without the full `steps`, `path`, and `members`
/// (`review_tool(mode="affected_flows")` keeps those). A 512-node flow was
/// otherwise about 130K characters of mostly unchanged steps.
fn compact_flow(flow: Value, changed: &HashSet<&str>) -> Value {
    let Value::Object(mut map) = flow else {
        return flow;
    };
    let steps = map.remove("steps");
    map.remove("path");
    map.remove("members");
    let touched: Vec<Value> = steps
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|step| {
            step["qualified_name"]
                .as_str()
                .is_some_and(|name| changed.contains(name))
        })
        .cloned()
        .collect();
    map.insert("changed_step_count".to_string(), json!(touched.len()));
    map.insert(
        "changed_steps".to_string(),
        Value::Array(touched.into_iter().take(CHANGED_STEPS_KEPT).collect()),
    );
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_ranges_follow_python() {
        let diff = "diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -1,0 +2,3 @@ def f():\n+x\n@@ -9 +12 @@\n@@ -4,2 +5,0 @@\n+++ \"b/sp\\303\\244ce.py\"\n@@ -1 +1,2 @@\n+++ /dev/null\n@@ -1 +1 @@\n";
        let ranges = parse_unified_diff(diff);
        assert_eq!(ranges["a.py"], vec![(2, 4), (12, 12), (5, 5)]);
        assert_eq!(ranges["späce.py"], vec![(1, 2)]);
        assert_eq!(ranges.len(), 2);
        assert_eq!(hunk_new_range("@@ x +1 y +3,2 @@"), Some((3, 2)));
        assert_eq!(hunk_new_range("@@ +1 @@"), None);
    }

    #[test]
    fn paths_normalize_like_pathlib() {
        assert_eq!(posix("./a//b/"), "a/b");
        assert_eq!(posix(""), ".");
        let root = Path::new("/r");
        assert_eq!(repo_relative("/r/a/./b.py", root), "a/b.py");
        assert_eq!(repo_relative("/elsewhere/b.py", root), "b.py");
        assert_eq!(join(root, "./x/y.py"), "/r/x/y.py");
        assert_eq!(
            normalize_path_string("src/a.py::f", "src/a.py", "/r/src/a.py"),
            "/r/src/a.py::f"
        );
        assert_eq!(
            normalize_path_string("src/b.py", "src/a.py", "/r/src/a.py"),
            "/r/src/b.py"
        );
        assert_eq!(normalize_path_string("//x", "a", "a"), "/x");
    }
}
