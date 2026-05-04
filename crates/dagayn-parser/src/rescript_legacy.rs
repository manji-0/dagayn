use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::types::{ParsedEdge, ParsedNode};
use super::util::{
    contains_ascii_ignore_case, ends_with_ascii_ignore_case, is_test_file,
    starts_with_ascii_ignore_case,
};
use super::{add_tested_by_edges, qualify, resolve_rust_call_targets};

static RESCRIPT_MODULE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*module\s+(?:type\s+)?([A-Z][A-Za-z0-9_']*)\s*[:=]").unwrap()
});
static RESCRIPT_LET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*(?:let\s+(?:rec\s+)?|and\s+)([A-Za-z_][A-Za-z0-9_']*)\b",
    )
    .unwrap()
});
static RESCRIPT_EXTERNAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*external\s+([A-Za-z_][A-Za-z0-9_']*)\s*:",
    )
    .unwrap()
});
static RESCRIPT_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*type\s+(?:rec\s+)?([A-Za-z_][A-Za-z0-9_']*)\b",
    )
    .unwrap()
});
static RESCRIPT_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(open|include)\s+([A-Z][A-Za-z0-9_'.]*)").unwrap());
static RESCRIPT_MODULE_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*module\s+([A-Z][A-Za-z0-9_']*)\s*=\s*([A-Z][A-Za-z0-9_']*(?:\.[A-Za-z_][A-Za-z0-9_']*)*)\s*$",
    )
    .unwrap()
});
static RESCRIPT_JSX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)(^|[\s{(,>}])<([A-Z][A-Za-z0-9_']*(?:\.[A-Z][A-Za-z0-9_']*)*)\b").unwrap()
});
static RESCRIPT_MODULE_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"@module\(\s*"([^"]+)"\s*\)"#).unwrap());
static RESCRIPT_CALL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^A-Za-z0-9_'])([A-Za-z_][A-Za-z0-9_']*(?:\.[A-Za-z_][A-Za-z0-9_']*)*)\s*\(")
        .unwrap()
});
static RESCRIPT_DEFINITION_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:let|type|external)\b").unwrap());

pub fn parse_rescript(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let cleaned = strip_rescript_noise(&text);
    let line_starts = line_starts(&cleaned);
    let is_interface = ends_with_ascii_ignore_case(file_path, ".resi");
    let test_file = rescript_is_test_file(file_path);

    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: text
            .as_bytes()
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as i64
            + 1,
        language: "rescript".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: test_file,
        extra: if is_interface {
            json!({"rescript_interface": true})
        } else {
            json!({})
        },
    }];
    let mut edges = Vec::new();
    let mut modules = scan_rescript_modules(&cleaned, &line_starts);
    assign_rescript_module_parents(&mut modules);
    let depth = rescript_brace_depth_array(&cleaned);

    push_rescript_module_nodes(&mut nodes, file_path, &modules);

    let mut lets = collect_rescript_lets(file_path, &cleaned, &line_starts, &modules, &depth);
    fill_rescript_let_ends(&mut lets, &cleaned, &line_starts, &modules);
    push_rescript_let_nodes(&mut nodes, file_path, &lets);
    collect_rescript_external_nodes_and_edges(
        &mut nodes,
        &mut edges,
        file_path,
        &text,
        &cleaned,
        &line_starts,
        &modules,
        &depth,
    );
    collect_rescript_type_nodes(
        &mut nodes,
        file_path,
        &cleaned,
        &line_starts,
        &modules,
        &depth,
    );

    for capture in RESCRIPT_OPEN_RE.captures_iter(&cleaned) {
        let Some(kind) = capture.get(1) else {
            continue;
        };
        let Some(target) = capture.get(2) else {
            continue;
        };
        let Some(full) = capture.get(0) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target: target.as_str().to_string(),
            file_path: file_path.to_string(),
            line: offset_to_line(&line_starts, full.start()),
            extra: json!({"rescript_import_kind": kind.as_str()}),
        });
    }

    for capture in RESCRIPT_MODULE_ALIAS_RE.captures_iter(&cleaned) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        if modules
            .iter()
            .any(|module| module.start_off == full.start())
        {
            continue;
        }
        let Some(alias) = capture.get(1) else {
            continue;
        };
        let Some(target) = capture.get(2) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target: target.as_str().to_string(),
            file_path: file_path.to_string(),
            line: offset_to_line(&line_starts, full.start()),
            extra: json!({
                "rescript_import_kind": "module_alias",
                "alias_name": alias.as_str(),
            }),
        });
    }

    if !is_interface {
        for capture in RESCRIPT_JSX_RE.captures_iter(&cleaned) {
            let Some(target_match) = capture.get(2) else {
                continue;
            };
            let target = target_match.as_str();
            let root = target.split('.').next().unwrap_or(target);
            let off = target_match.start();
            let line = offset_to_line(&line_starts, off);
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: root.to_string(),
                file_path: file_path.to_string(),
                line,
                extra: json!({"rescript_import_kind": "jsx"}),
            });
            if let Some(entry) = rescript_enclosing_let(&lets, off) {
                edges.push(ParsedEdge {
                    kind: "CALLS".to_string(),
                    source: qualify(file_path, &entry.name, entry.parent.as_deref()),
                    target: target.to_string(),
                    file_path: file_path.to_string(),
                    line,
                    extra: json!({"rescript_call_kind": "jsx"}),
                });
            }
        }
    }

    if !is_interface && !lets.is_empty() {
        for capture in RESCRIPT_CALL_RE.captures_iter(&cleaned) {
            let Some(target_match) = capture.get(2) else {
                continue;
            };
            let target = target_match.as_str();
            let top = target.split('.').next().unwrap_or(target);
            if rescript_is_keyword(top) || rescript_is_keyword(target) {
                continue;
            }
            let (target, off) = expand_rescript_call_target(&cleaned, target, target_match.start());
            let top = target.split('.').next().unwrap_or(&target);
            if rescript_is_keyword(top) || rescript_is_keyword(&target) {
                continue;
            }
            let Some(entry) = rescript_enclosing_let(&lets, off) else {
                continue;
            };
            if entry.name == target && off == entry.start_off {
                continue;
            }
            edges.push(ParsedEdge {
                kind: "CALLS".to_string(),
                source: qualify(file_path, &entry.name, entry.parent.as_deref()),
                target,
                file_path: file_path.to_string(),
                line: offset_to_line(&line_starts, off),
                extra: json!({}),
            });
        }
    }

    for node in &nodes {
        if matches!(node.kind.as_str(), "Function" | "Type" | "Test") {
            if let Some(parent) = node.parent_name.as_deref() {
                edges.push(ParsedEdge {
                    kind: "CONTAINS".to_string(),
                    source: qualify(file_path, parent, None),
                    target: qualify(file_path, &node.name, Some(parent)),
                    file_path: file_path.to_string(),
                    line: node.line_start,
                    extra: json!({}),
                });
            }
        }
    }

    tag_rescript_js_binding_modules(&mut nodes);
    edges = dedupe_rescript_imports(edges);
    edges = resolve_rust_call_targets(&nodes, edges, file_path);
    if test_file {
        add_tested_by_edges(&nodes, &mut edges);
    }
    (nodes, edges)
}

#[derive(Clone, Debug)]
struct RescriptModule {
    name: String,
    start_off: usize,
    end_off: usize,
    body_start_off: usize,
    start_line: i64,
    end_line: i64,
    parent: Option<String>,
}

#[derive(Clone, Debug)]
struct RescriptLet {
    name: String,
    start_off: usize,
    line_start: i64,
    parent: Option<String>,
    is_test: bool,
    end_off: usize,
    line_end: i64,
}

fn push_rescript_module_nodes(
    nodes: &mut Vec<ParsedNode>,
    file_path: &str,
    modules: &[RescriptModule],
) {
    for module in modules {
        nodes.push(ParsedNode {
            kind: "Class".to_string(),
            name: module.name.clone(),
            file_path: file_path.to_string(),
            line_start: module.start_line,
            line_end: module.end_line,
            language: "rescript".to_string(),
            parent_name: module.parent.clone(),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"rescript_kind": "module"}),
        });
    }
}

fn push_rescript_let_nodes(nodes: &mut Vec<ParsedNode>, file_path: &str, lets: &[RescriptLet]) {
    for entry in lets {
        nodes.push(ParsedNode {
            kind: if entry.is_test { "Test" } else { "Function" }.to_string(),
            name: entry.name.clone(),
            file_path: file_path.to_string(),
            line_start: entry.line_start,
            line_end: entry.line_end,
            language: "rescript".to_string(),
            parent_name: entry.parent.clone(),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: entry.is_test,
            extra: json!({}),
        });
    }
}

fn collect_rescript_external_nodes_and_edges(
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
    file_path: &str,
    text: &str,
    cleaned: &str,
    line_starts: &[usize],
    modules: &[RescriptModule],
    depth: &[usize],
) {
    for capture in RESCRIPT_EXTERNAL_RE.captures_iter(cleaned) {
        let Some(name_match) = capture.get(1) else {
            continue;
        };
        let name = name_match.as_str();
        if rescript_is_keyword(name) {
            continue;
        }
        let off = name_match.start();
        let parent = rescript_enclosing_module(modules, off);
        if !rescript_is_top_level(off, parent.as_deref(), modules, depth) {
            continue;
        }
        let line_start = offset_to_line(line_starts, off);
        nodes.push(ParsedNode {
            kind: "Function".to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start,
            line_end: line_start,
            language: "rescript".to_string(),
            parent_name: parent,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"rescript_external": true}),
        });
        let look_start = off.saturating_sub(200);
        if let Some(snippet) = safe_str_slice(text, look_start, off) {
            for attr in RESCRIPT_MODULE_ATTR_RE.captures_iter(snippet) {
                if let Some(target) = attr.get(1) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: file_path.to_string(),
                        target: target.as_str().to_string(),
                        file_path: file_path.to_string(),
                        line: line_start,
                        extra: json!({"rescript_import_kind": "external_module"}),
                    });
                }
            }
        }
    }
}

fn collect_rescript_type_nodes(
    nodes: &mut Vec<ParsedNode>,
    file_path: &str,
    cleaned: &str,
    line_starts: &[usize],
    modules: &[RescriptModule],
    depth: &[usize],
) {
    for capture in RESCRIPT_TYPE_RE.captures_iter(cleaned) {
        let Some(name_match) = capture.get(1) else {
            continue;
        };
        let name = name_match.as_str();
        if rescript_is_keyword(name) {
            continue;
        }
        let off = name_match.start();
        let parent = rescript_enclosing_module(modules, off);
        if !rescript_is_top_level(off, parent.as_deref(), modules, depth) {
            continue;
        }
        let line_start = offset_to_line(line_starts, off);
        nodes.push(ParsedNode {
            kind: "Type".to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start,
            line_end: line_start,
            language: "rescript".to_string(),
            parent_name: parent,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        });
    }
}

fn strip_rescript_noise(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        if c == '/' && next == '/' {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if c == '/' && next == '*' {
            let mut depth = 1usize;
            out.push(' ');
            out.push(' ');
            i += 2;
            while i < chars.len() && depth > 0 {
                let c = chars[i];
                let next = chars.get(i + 1).copied().unwrap_or('\0');
                if c == '/' && next == '*' {
                    depth += 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else if c == '*' && next == '/' {
                    depth -= 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            continue;
        }
        if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            if i < chars.len() {
                out.push('"');
                i += 1;
            }
            continue;
        }
        if c == '`' {
            out.push('`');
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            if i < chars.len() {
                out.push('`');
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn rescript_brace_depth_array(cleaned: &str) -> Vec<usize> {
    let mut depth = vec![0; cleaned.len() + 1];
    let mut current = 0usize;
    for (idx, ch) in cleaned.char_indices() {
        depth[idx] = current;
        if ch == '{' {
            current += 1;
        } else if ch == '}' {
            current = current.saturating_sub(1);
        }
    }
    depth[cleaned.len()] = current;
    depth
}

fn scan_rescript_modules(cleaned: &str, line_starts: &[usize]) -> Vec<RescriptModule> {
    let alias_starts = RESCRIPT_MODULE_ALIAS_RE
        .captures_iter(cleaned)
        .filter_map(|capture| capture.get(0).map(|matched| matched.start()))
        .collect::<HashSet<_>>();
    let mut modules = Vec::new();
    for capture in RESCRIPT_MODULE_RE.captures_iter(cleaned) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        if alias_starts.contains(&full.start()) {
            continue;
        }
        let Some(name) = capture.get(1) else {
            continue;
        };
        let Some(brace_rel) = cleaned[full.end()..].find('{') else {
            continue;
        };
        let brace_open = full.end() + brace_rel;
        if RESCRIPT_DEFINITION_START_RE.is_match(&cleaned[full.end()..brace_open]) {
            continue;
        }
        let mut brace_depth = 1usize;
        let mut brace_close = cleaned.len().saturating_sub(1);
        for (idx, ch) in cleaned[brace_open + 1..].char_indices() {
            let absolute = brace_open + 1 + idx;
            if ch == '{' {
                brace_depth += 1;
            } else if ch == '}' {
                brace_depth = brace_depth.saturating_sub(1);
                if brace_depth == 0 {
                    brace_close = absolute;
                    break;
                }
            }
        }
        modules.push(RescriptModule {
            name: name.as_str().to_string(),
            start_off: full.start(),
            end_off: brace_close,
            body_start_off: brace_open + 1,
            start_line: offset_to_line(line_starts, full.start()),
            end_line: offset_to_line(line_starts, brace_close),
            parent: None,
        });
    }
    modules
}

fn assign_rescript_module_parents(modules: &mut [RescriptModule]) {
    let snapshot = modules.to_vec();
    for module in modules {
        let mut parent_name = None;
        let mut parent_start = 0usize;
        let mut found_parent = false;
        for other in &snapshot {
            if other.start_off < module.start_off
                && other.end_off > module.end_off
                && (!found_parent || other.start_off > parent_start)
            {
                parent_name = Some(other.name.clone());
                parent_start = other.start_off;
                found_parent = true;
            }
        }
        module.parent = parent_name;
    }
}

fn collect_rescript_lets(
    file_path: &str,
    cleaned: &str,
    line_starts: &[usize],
    modules: &[RescriptModule],
    depth: &[usize],
) -> Vec<RescriptLet> {
    let mut entries = Vec::new();
    for capture in RESCRIPT_LET_RE.captures_iter(cleaned) {
        let Some(name_match) = capture.get(1) else {
            continue;
        };
        let name = name_match.as_str();
        if rescript_is_keyword(name) {
            continue;
        }
        let off = name_match.start();
        let parent = rescript_enclosing_module(modules, off);
        if !rescript_is_top_level(off, parent.as_deref(), modules, depth) {
            continue;
        }
        entries.push(RescriptLet {
            name: name.to_string(),
            start_off: off,
            line_start: offset_to_line(line_starts, off),
            parent,
            is_test: rescript_is_test_function(name, file_path),
            end_off: off + 1,
            line_end: offset_to_line(line_starts, off),
        });
    }
    entries.sort_by_key(|entry| entry.start_off);
    entries
}

fn fill_rescript_let_ends(
    entries: &mut [RescriptLet],
    cleaned: &str,
    line_starts: &[usize],
    modules: &[RescriptModule],
) {
    for idx in 0..entries.len() {
        let mut next = entries
            .get(idx + 1)
            .map(|entry| entry.start_off)
            .unwrap_or(cleaned.len());
        if let Some(parent) = entries[idx].parent.as_deref() {
            if let Some(module) = modules.iter().find(|module| {
                module.name == parent
                    && module.start_off <= entries[idx].start_off
                    && entries[idx].start_off <= module.end_off
            }) {
                next = next.min(module.end_off);
            }
        }
        entries[idx].end_off = next.max(entries[idx].start_off + 1);
        entries[idx].line_end = offset_to_line(line_starts, entries[idx].end_off - 1);
    }
}

fn rescript_enclosing_module(modules: &[RescriptModule], off: usize) -> Option<String> {
    let mut innermost_name = None;
    let mut innermost_start = 0usize;
    let mut found = false;
    for module in modules {
        if module.start_off <= off
            && off <= module.end_off
            && (!found || module.start_off > innermost_start)
        {
            innermost_name = Some(module.name.clone());
            innermost_start = module.start_off;
            found = true;
        }
    }
    innermost_name
}

fn rescript_is_top_level(
    off: usize,
    parent: Option<&str>,
    modules: &[RescriptModule],
    depth: &[usize],
) -> bool {
    let current_depth = depth.get(off).copied().unwrap_or(0);
    let Some(parent) = parent else {
        return current_depth == 0;
    };
    modules
        .iter()
        .find(|module| module.name == parent && module.start_off <= off && off <= module.end_off)
        .is_some_and(|module| {
            current_depth == depth.get(module.body_start_off).copied().unwrap_or(0)
        })
}

fn rescript_enclosing_let(entries: &[RescriptLet], off: usize) -> Option<&RescriptLet> {
    let mut found = None;
    for entry in entries {
        if entry.start_off <= off && off < entry.end_off {
            found = Some(entry);
        } else if entry.start_off > off {
            break;
        }
    }
    found
}

fn expand_rescript_call_target(cleaned: &str, target: &str, off: usize) -> (String, usize) {
    let bytes = cleaned.as_bytes();
    if off == 0 || bytes.get(off.wrapping_sub(1)) != Some(&b'.') {
        return (target.to_string(), off);
    }
    let mut start = off - 1;
    while start > 0 {
        let byte = bytes[start - 1];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'\'' | b'.') {
            start -= 1;
        } else {
            break;
        }
    }
    let expanded = cleaned
        .get(start..off + target.len())
        .filter(|candidate| {
            candidate
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        })
        .unwrap_or(target);
    (expanded.to_string(), start)
}

fn tag_rescript_js_binding_modules(nodes: &mut [ParsedNode]) {
    let mut member_funcs: HashMap<String, Vec<bool>> = HashMap::new();
    for node in nodes.iter() {
        if node.kind == "Function" {
            if let Some(parent) = node.parent_name.as_deref() {
                member_funcs.entry(parent.to_string()).or_default().push(
                    node.extra.get("rescript_external").and_then(Value::as_bool) == Some(true),
                );
            }
        }
    }
    for node in nodes {
        if node.kind != "Class" {
            continue;
        }
        if member_funcs
            .get(&node.name)
            .is_some_and(|members| !members.is_empty() && members.iter().all(|value| *value))
        {
            node.extra = json!({"rescript_kind": "js_binding"});
        }
    }
}

fn dedupe_rescript_imports(edges: Vec<ParsedEdge>) -> Vec<ParsedEdge> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(edges.len());
    for edge in edges {
        if edge.kind == "IMPORTS_FROM" {
            let key = (edge.source.clone(), edge.target.clone());
            if !seen.insert(key) {
                continue;
            }
        }
        deduped.push(edge);
    }
    deduped
}

fn rescript_is_keyword(name: &str) -> bool {
    matches!(
        name,
        "let"
            | "rec"
            | "and"
            | "type"
            | "module"
            | "open"
            | "include"
            | "external"
            | "if"
            | "else"
            | "switch"
            | "when"
            | "match"
            | "fun"
            | "true"
            | "false"
            | "for"
            | "while"
            | "mutable"
            | "try"
            | "catch"
            | "throw"
            | "assert"
            | "lazy"
            | "do"
            | "in"
            | "of"
            | "as"
            | "exception"
            | "private"
            | "constraint"
            | "with"
            | "downto"
            | "to"
            | "unpack"
            | "async"
            | "await"
    )
}

fn rescript_is_test_file(file_path: &str) -> bool {
    is_test_file(file_path)
        || ends_with_ascii_ignore_case(file_path, "_test.res")
        || ends_with_ascii_ignore_case(file_path, "_test.resi")
        || contains_ascii_ignore_case(file_path, ".test.res")
        || contains_ascii_ignore_case(file_path, ".test.resi")
}

fn rescript_is_test_function(name: &str, file_path: &str) -> bool {
    starts_with_ascii_ignore_case(name, "test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_spec")
        || (rescript_is_test_file(file_path)
            && matches!(
                name,
                "describe" | "it" | "test" | "beforeEach" | "afterEach" | "beforeAll" | "afterAll"
            ))
}

fn safe_str_slice(value: &str, start: usize, end: usize) -> Option<&str> {
    let start = previous_char_boundary(value, start.min(value.len()));
    let end = previous_char_boundary(value, end.min(value.len()));
    (start <= end).then(|| &value[start..end])
}

fn previous_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn offset_to_line(line_starts: &[usize], off: usize) -> i64 {
    match line_starts.binary_search(&off) {
        Ok(idx) => idx as i64 + 1,
        Err(idx) => idx as i64,
    }
}
