//! `dagayn.coverage`: the heuristic test inference behind `query_graph_tool`
//! `tests_for` and the review tools' test gaps.
//!
//! Python's `str.casefold` differs from `to_lowercase` for a few characters;
//! [`casefold`] maps the common ones and gives up (`None`, leaving the call
//! to Python) on the rest.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::json;

use crate::query::{Row, node_row};

const TEST_FILE_PARTS: &[&str] = &["/tests/", "/test/", "/__tests__/"];
const TEST_FILE_SUFFIXES: &[&str] = &[
    "_test.py",
    "_tests.py",
    ".test.js",
    ".test.ts",
    ".test.tsx",
    ".spec.js",
    ".spec.ts",
    ".spec.tsx",
    "_test.rs",
    "_tests.rs",
];
const NON_TEST_HELPER_NAMES: &[&str] = &[
    "setup",
    "teardown",
    "setup_method",
    "teardown_method",
    "setup_class",
    "teardown_class",
    "setup_module",
    "teardown_module",
];
const MODULE_MARKER_SKIP: &[&str] = &["src", "lib", "pkg", "internal", "tests", "test"];

/// `str.casefold`, or `None` for a character whose full case folding this
/// does not reproduce.
pub(crate) fn casefold(text: &str) -> Option<String> {
    if text.is_ascii() {
        return Some(text.to_ascii_lowercase());
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'ß' | 'ẞ' => out.push_str("ss"),
            'ς' => out.push('σ'),
            'µ' => out.push('μ'),
            'ſ' => out.push('s'),
            'ϐ' => out.push('β'),
            'ϑ' => out.push('θ'),
            'ϕ' => out.push('φ'),
            'ϖ' => out.push('π'),
            'ϰ' => out.push('κ'),
            'ϱ' => out.push('ρ'),
            'ϵ' => out.push('ε'),
            'ẛ' => out.push('ṡ'),
            '\u{345}' => out.push('ι'),
            'ﬀ' => out.push_str("ff"),
            'ﬁ' => out.push_str("fi"),
            'ﬂ' => out.push_str("fl"),
            'ﬃ' => out.push_str("ffi"),
            'ﬄ' => out.push_str("ffl"),
            'ﬅ' | 'ﬆ' => out.push_str("st"),
            // Folds this does not spell out: multi-character Greek,
            // Armenian, and Latin folds, Cherokee (folds to upper case), and
            // the old Cyrillic variants.
            'ŉ' | 'ǰ' | 'ΐ' | 'ΰ' | 'և' => return None,
            '\u{1E96}'..='\u{1E9A}'
            | '\u{1F50}'..='\u{1FFF}'
            | '\u{FB13}'..='\u{FB17}'
            | '\u{13A0}'..='\u{13FD}'
            | '\u{AB70}'..='\u{ABBF}'
            | '\u{1C80}'..='\u{1C88}' => return None,
            c => out.extend(c.to_lowercase()),
        }
    }
    Some(out)
}

/// `is_test_file_path`.
pub(crate) fn is_test_file_path(file_path: &str) -> bool {
    let normalized = file_path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or("");
    TEST_FILE_PARTS.iter().any(|part| normalized.contains(part))
        || name.starts_with("test_")
        || name == "tests.rs"
        || name == "test.rs"
        || TEST_FILE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// `PurePosixPath(path)`'s name and its parent's name.
fn path_names(path: &str) -> (&str, &str) {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    match parts.as_slice() {
        [] => ("", ""),
        [name] => (name, ""),
        [.., parent, name] => (name, parent),
    }
}

/// `PurePath.stem` of a name.
fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[..index],
        _ => name,
    }
}

/// `_identifier_tokens`.
fn identifier_tokens(value: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = value.chars().collect();
    // `([a-z])([A-Z])` -> `\1_\2`.
    let mut camel: Vec<char> = Vec::with_capacity(chars.len() + 4);
    for (index, c) in chars.iter().enumerate() {
        camel.push(*c);
        if c.is_ascii_lowercase() && chars.get(index + 1).is_some_and(char::is_ascii_uppercase) {
            camel.push('_');
        }
    }
    // `([A-Z]+)([A-Z][a-z])` -> `\1_\2`: a run of capitals followed by a
    // lower-case letter splits before its last capital.
    let mut split: Vec<char> = Vec::with_capacity(camel.len() + 4);
    let mut index = 0;
    while index < camel.len() {
        if !camel[index].is_ascii_uppercase() {
            split.push(camel[index]);
            index += 1;
            continue;
        }
        let mut end = index;
        while end < camel.len() && camel[end].is_ascii_uppercase() {
            end += 1;
        }
        let run = &camel[index..end];
        if run.len() >= 2 && camel.get(end).is_some_and(char::is_ascii_lowercase) {
            split.extend_from_slice(&run[..run.len() - 1]);
            split.push('_');
            split.push(run[run.len() - 1]);
            split.push(camel[end]);
            index = end + 1;
        } else {
            split.extend_from_slice(run);
            index = end;
        }
    }
    let folded = casefold(&split.into_iter().collect::<String>())?;
    Some(
        folded
            .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
            .filter(|token| !token.is_empty() && *token != "test" && *token != "tests")
            .map(str::to_string)
            .collect(),
    )
}

/// `_contains_word` on an already casefolded haystack.
fn contains_word(haystack_cf: &str, needle: &str) -> Option<bool> {
    if needle.is_empty() {
        return Some(false);
    }
    let needle = casefold(needle)?;
    let bytes = haystack_cf.as_bytes();
    let word = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let mut from = 0;
    while let Some(offset) = haystack_cf[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before = start > 0 && word(bytes[start - 1]);
        let after = end < bytes.len() && word(bytes[end]);
        if !before && !after {
            return Some(true);
        }
        from = start
            + haystack_cf[start..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
    }
    Some(false)
}

/// `str.splitlines`.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// One test-like candidate with its casefolded path and identity.
struct Candidate {
    node: GraphNode,
    path_cf: String,
    identity_cf: String,
}

/// `build_scan_state`: the candidates, their import edges, and the caches
/// one analysis pass shares across targets.
pub(crate) struct ScanState {
    candidates: Vec<Candidate>,
    imports: HashMap<String, Vec<GraphEdge>>,
    tokens: HashMap<String, (Vec<String>, String)>,
    sources: HashMap<String, Vec<String>>,
}

impl ScanState {
    pub(crate) fn build(store: &GraphStore) -> Option<Self> {
        let kinds = ["Test", "Function", "Class"].map(String::from);
        let mut candidates = Vec::new();
        let mut keys = HashSet::new();
        for node in store.get_nodes_by_kind(&kinds, None).ok()? {
            if !is_test_like(&node)? {
                continue;
            }
            let path_cf = casefold(&node.file_path.replace('\\', "/"))?;
            let identity_cf = casefold(&format!(
                "{} {} {}",
                node.qualified_name, node.name, node.file_path
            ))?;
            keys.insert(node.file_path.clone());
            keys.insert(node.qualified_name.clone());
            candidates.push(Candidate {
                node,
                path_cf,
                identity_cf,
            });
        }
        let mut imports: HashMap<String, Vec<GraphEdge>> = HashMap::new();
        if !keys.is_empty() {
            for edge in store.get_edges_by_kind("IMPORTS_FROM", false).ok()? {
                if keys.contains(&edge.source_qualified) {
                    imports
                        .entry(edge.source_qualified.clone())
                        .or_default()
                        .push(edge);
                }
            }
        }
        Some(Self {
            candidates,
            imports,
            tokens: HashMap::new(),
            sources: HashMap::new(),
        })
    }

    /// `_identifier_tokens` and `_squashed_identifier`, cached.
    fn tokens_for(&mut self, value: &str) -> Option<(Vec<String>, String)> {
        if let Some(cached) = self.tokens.get(value) {
            return Some(cached.clone());
        }
        let tokens = identifier_tokens(value)?;
        let squashed = tokens.concat();
        self.tokens
            .insert(value.to_string(), (tokens.clone(), squashed.clone()));
        Some((tokens, squashed))
    }

    /// `_span_text`.
    fn span_text(&mut self, store: &GraphStore, node: &GraphNode) -> Option<String> {
        if !self.sources.contains_key(&node.file_path) {
            let lines = store
                .resolve_file_path(&node.file_path)
                .ok()
                .and_then(|path| std::fs::read(path).ok())
                .map(|bytes| {
                    splitlines(&String::from_utf8_lossy(&bytes))
                        .into_iter()
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            self.sources.insert(node.file_path.clone(), lines);
        }
        let lines = &self.sources[&node.file_path];
        if lines.is_empty() || node.line_start <= 0 || node.line_end < node.line_start {
            return Some(String::new());
        }
        let start = (node.line_start - 1) as usize;
        let end = (node.line_end as usize).min(lines.len());
        if start >= end {
            return Some(String::new());
        }
        casefold(&lines[start..end].join("\n"))
    }

    /// `_name_matches_target_symbol`.
    fn name_matches(
        &mut self,
        target_name: &str,
        candidate_text: &str,
        allowed_suffix: Option<&HashSet<String>>,
    ) -> Option<bool> {
        let target_cf = casefold(target_name)?;
        let candidate_cf = casefold(candidate_text)?;
        if target_cf == candidate_cf {
            return Some(true);
        }
        for prefix in ["test_", "test"] {
            if let Some(rest) = candidate_cf.strip_prefix(prefix)
                && rest.trim_start_matches('_') == target_cf
            {
                return Some(true);
            }
        }
        let (target_tokens, target_squashed) = self.tokens_for(target_name)?;
        let (candidate_tokens, candidate_squashed) = self.tokens_for(candidate_text)?;
        if target_squashed == candidate_squashed {
            return Some(true);
        }
        if target_tokens.is_empty() || target_tokens.len() > candidate_tokens.len() {
            return Some(false);
        }
        // Only a match at the start counts (`before` must be empty).
        if candidate_tokens[..target_tokens.len()] != target_tokens[..] {
            return Some(false);
        }
        let after = &candidate_tokens[target_tokens.len()..];
        Some(
            after.is_empty()
                || allowed_suffix.is_some_and(|allowed| after.iter().all(|t| allowed.contains(t))),
        )
    }
}

/// `_is_test_like_node`.
fn is_test_like(node: &GraphNode) -> Option<bool> {
    let name = casefold(&node.name)?;
    let qualified = casefold(&node.qualified_name)?;
    if node.kind == "Function" && !node.is_test && NON_TEST_HELPER_NAMES.contains(&name.as_str()) {
        return Some(false);
    }
    if node.kind == "Function" && !node.is_test {
        return Some(
            name.starts_with("test")
                || qualified.contains("::test")
                || qualified.contains(".test.")
                || qualified.contains(".spec."),
        );
    }
    Some(
        node.is_test
            || node.kind == "Test"
            || name.starts_with("test")
            || qualified.contains(".test.")
            || qualified.contains("::test")
            || is_test_file_path(&node.file_path),
    )
}

/// What `infer_tests_for_node` derives from its target once.
struct Target<'a> {
    node: &'a GraphNode,
    markers: HashSet<String>,
    stem: String,
    path_cf: String,
}

impl<'a> Target<'a> {
    fn new(node: &'a GraphNode) -> Option<Self> {
        let path = node.file_path.replace('\\', "/");
        let (name, parent) = path_names(&path);
        let stem = casefold(stem(name))?;
        let mut markers = HashSet::new();
        if !stem.is_empty() {
            markers.insert(stem.clone());
            if let Some(rest) = stem.strip_prefix("test_") {
                markers.insert(rest.to_string());
            } else if let Some(rest) = stem.strip_suffix("_test") {
                markers.insert(rest.to_string());
            }
        }
        let parent = casefold(parent)?;
        if !parent.is_empty() && !MODULE_MARKER_SKIP.contains(&parent.as_str()) {
            markers.insert(parent);
        }
        Some(Self {
            node,
            markers,
            stem,
            path_cf: casefold(&path)?,
        })
    }
}

/// `_candidate_references_target_module`: its evidence when linked.
fn module_link(
    state: &ScanState,
    target: &Target,
    candidate: &Candidate,
) -> Option<Option<&'static str>> {
    let mut markers: Vec<&String> = target.markers.iter().collect();
    markers.sort_by_key(|marker| std::cmp::Reverse(marker.len()));
    for marker in markers {
        if (candidate.path_cf.contains(marker.as_str())
            || candidate.identity_cf.contains(marker.as_str()))
            && (contains_word(&candidate.path_cf, marker)?
                || contains_word(&candidate.identity_cf, marker)?)
        {
            return Some(Some("test artifact references target module/file"));
        }
    }
    if !target.stem.is_empty()
        && (candidate
            .path_cf
            .starts_with(&format!("{}.", target.path_cf))
            || candidate
                .path_cf
                .starts_with(&format!("{}_", target.path_cf))
            || contains_word(path_names(&candidate.path_cf).0, &target.stem)?)
    {
        return Some(Some("test file co-located with target module"));
    }
    let mut keys = vec![&candidate.node.file_path];
    if candidate.node.qualified_name != candidate.node.file_path {
        keys.push(&candidate.node.qualified_name);
    }
    for key in keys {
        for edge in state.imports.get(key).into_iter().flatten() {
            let import_target = casefold(&edge.target_qualified.replace('\\', "/"))?;
            if import_target == target.path_cf
                || import_target.starts_with(&format!("{}::", target.path_cf))
            {
                return Some(Some("test file imports target module"));
            }
        }
    }
    Some(None)
}

/// `_candidate_score`: score, confidence, and evidence.
fn candidate_score(
    store: &GraphStore,
    state: &mut ScanState,
    target: &Target,
    index: usize,
) -> Option<(i64, &'static str, Vec<&'static str>)> {
    let candidate = &state.candidates[index];
    let link = module_link(state, target, candidate)?;
    let candidate_name = candidate.node.name.clone();
    let identity_cf = candidate.identity_cf.clone();
    let mut evidence = Vec::new();
    if let Some(link) = link {
        evidence.push(link);
        if state.name_matches(&target.node.name, &candidate_name, Some(&target.markers))? {
            evidence.push("test node name references target symbol");
            return Some((80, "medium", evidence));
        }
        let node = state.candidates[index].node.clone();
        let span = state.span_text(store, &node)?;
        if !span.is_empty()
            && state.name_matches(&target.node.name, &span, Some(&target.markers))?
        {
            evidence.push("test source references target symbol");
            return Some((65, "medium", evidence));
        }
    }
    if !target.stem.is_empty() && contains_word(&identity_cf, &target.stem)? {
        evidence.push("test node name references target file stem");
        return Some((35, "low", evidence));
    }
    if state.name_matches(&target.node.name, &candidate_name, None)? {
        evidence.push("test node name resembles target symbol (no module link)");
        return Some((25, "low", evidence));
    }
    Some((0, "low", evidence))
}

fn coverage_row(node: &GraphNode, confidence: &str, evidence: Vec<&str>, source: &str) -> Row {
    let mut row = node_row(node);
    row.push(("confidence", json!(confidence)));
    row.push(("evidence", json!(evidence)));
    row.push(("coverage_source", json!(source)));
    row
}

fn rank(confidence: &str) -> i64 {
    match confidence {
        "medium" => 1,
        "high" => 2,
        _ => 0,
    }
}

/// `infer_tests_for_node`: the target's tests, best first.
pub(crate) fn infer_tests_for_node(
    store: &GraphStore,
    state: &mut ScanState,
    target: &GraphNode,
    limit: usize,
    minimum_confidence: &str,
) -> Option<Vec<Row>> {
    let min_rank = rank(minimum_confidence);
    let mut results: HashMap<String, (i64, Row)> = HashMap::new();

    let tested_by = |source: &str| -> Option<Vec<GraphEdge>> {
        Some(
            store
                .get_edges_by_source(source)
                .ok()?
                .into_iter()
                .filter(|edge| edge.kind == "TESTED_BY")
                .collect(),
        )
    };
    let mut seen_ids = HashSet::new();
    let mut direct: Vec<GraphEdge> = tested_by(&target.qualified_name)?
        .into_iter()
        .filter(|edge| seen_ids.insert(edge.id))
        .collect();
    if direct.is_empty() {
        direct = tested_by(&target.name)?
            .into_iter()
            .filter(|edge| seen_ids.insert(edge.id))
            .collect();
    }
    let names: Vec<String> = direct.iter().map(|e| e.target_qualified.clone()).collect();
    let nodes = store.get_nodes_by_qualified_names(&names).ok()?;
    for edge in &direct {
        if let Some(test) = nodes.get(&edge.target_qualified) {
            let row = coverage_row(test, "high", vec!["TESTED_BY edge"], "graph_edge");
            results.insert(test.qualified_name.clone(), (100, row));
        }
    }

    let target_info = Target::new(target)?;
    let early_exit = limit <= 1 && min_rank <= rank("medium");
    for index in 0..state.candidates.len() {
        if state.candidates[index].node.qualified_name == target.qualified_name {
            continue;
        }
        let (score, confidence, evidence) = candidate_score(store, state, &target_info, index)?;
        if score <= 0 || rank(confidence) < min_rank {
            continue;
        }
        let candidate = &state.candidates[index].node;
        if results
            .get(&candidate.qualified_name)
            .is_some_and(|(current, _)| *current >= score)
        {
            continue;
        }
        let row = coverage_row(candidate, confidence, evidence, "heuristic");
        results.insert(candidate.qualified_name.clone(), (score, row));
        if early_exit && score >= 80 {
            break;
        }
    }

    // Python sorts on the record's (sanitized) `qualified_name`.
    let mut ranked: Vec<(i64, String, Row)> = results
        .into_values()
        .map(|(score, row)| {
            let key = row
                .iter()
                .find(|(key, _)| *key == "qualified_name")
                .and_then(|(_, value)| value.as_str())
                .unwrap_or("")
                .to_string();
            (score, key, row)
        })
        .collect();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    Some(
        ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, row)| row)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_tokens_split_like_python() {
        let tokens = |value: &str| identifier_tokens(value).unwrap();
        assert_eq!(tokens("HTTPServerTest"), vec!["http", "server"]);
        assert_eq!(tokens("test_parse_URLValue"), vec!["parse", "url", "value"]);
        assert_eq!(tokens("ABcDEf"), vec!["a", "bc", "d", "ef"]);
        assert_eq!(tokens("größe"), vec!["gr", "sse"]);
    }

    #[test]
    fn words_need_ascii_boundaries() {
        assert_eq!(contains_word("tests/test_app.py", "app"), Some(true));
        assert_eq!(
            contains_word("tests/test_application.py", "app"),
            Some(false)
        );
        assert_eq!(contains_word("appapp app", "app"), Some(true));
        assert_eq!(contains_word("x", ""), Some(false));
    }

    #[test]
    fn splitlines_and_stems_follow_python() {
        assert_eq!(splitlines("a\r\nb\rc\u{2028}d\n"), vec!["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(stem("app.test.ts"), "app.test");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(stem("x."), "x.");
        assert_eq!(path_names("/a/b/./c.py"), ("c.py", "b"));
        assert_eq!(path_names("c.py"), ("c.py", ""));
    }

    #[test]
    fn casefold_gives_up_where_it_cannot_match_python() {
        assert_eq!(casefold("Straße").as_deref(), Some("strasse"));
        assert_eq!(casefold("ΣΑΣ").as_deref(), Some("σασ"));
        assert_eq!(casefold("\u{1F88}"), None);
        assert!(is_test_file_path("src\\tests\\a.py"));
        assert!(is_test_file_path("pkg/foo.spec.ts"));
        assert!(!is_test_file_path("pkg/contest.py"));
    }
}
