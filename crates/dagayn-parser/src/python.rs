use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::documentation_directives::{
    extract_line_comment_dagayn_directives, nearest_documentation_source,
    push_documentation_directive_edge,
};
use super::member_calls::MemberCallBindings;
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text};
use super::{qualify, resolve_rust_call_targets};

static NOTEBOOK_SQL_TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:FROM|JOIN|INTO|CREATE\s+(?:OR\s+REPLACE\s+)?(?:TABLE|VIEW)|INSERT\s+OVERWRITE)\s+((?:`[^`]+`|\w+)(?:\.(?:`[^`]+`|\w+))*)",
    )
    .unwrap()
});
static NOTEBOOK_R_FUNCTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z_.][A-Za-z0-9_.]*)\s*<-\s*function\s*(\([^)]*\))").unwrap()
});
static NOTEBOOK_R_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z_.][A-Za-z0-9_.]*)\s*\(").unwrap());
static MARIMO_MD_PYTHON_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{.*python.*\}").unwrap());
static MARIMO_MD_SQL_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{.*sql.*\}").unwrap());
static MARIMO_MD_MARIMO_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{.*marimo.*\}").unwrap());
static MARIMO_MD_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(\w+)="([^"]*)""#).unwrap());
static MARIMO_MD_ATTR_SQL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[.\s])sql(?:[.\s]|$)").unwrap());
static MARIMO_MD_ATTR_MARKDOWN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[.\s])markdown(?:[.\s]|$)").unwrap());

pub(super) fn parse_python_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    if is_databricks_py_source(source) {
        return parse_databricks_py_with_parser(&file_path, source, parser, repo_root);
    }
    if looks_like_marimo_py(source) {
        return parse_marimo_py_with_parser(&file_path, source, parser, repo_root);
    }

    parse_python_module_with_parser(&file_path, source, parser, repo_root)
}

fn parse_python_module_with_parser(
    file_path: &FilePath,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path.as_str()),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let root = tree.root_node();
        let (import_map, top_level_defined_names, protocol_names) =
            collect_python_file_scope(root, source);
        let class_names = collect_python_class_names(root, source);
        let context = PythonParseContext {
            source,
            file_path: file_path.clone(),
            repo_root,
            import_map: &import_map,
            top_level_defined_names: &top_level_defined_names,
            protocol_names: &protocol_names,
            bindings: RefCell::new(MemberCallBindings::with_types(class_names)),
        };
        python_walk_children(root, &context, None, None, &mut nodes, &mut edges);
        extract_python_documentation_directives(file_path, source, &nodes, &mut edges);
        let edges = resolve_python_call_targets(&nodes, edges, file_path);
        let edges = add_python_tested_by_edges(&nodes, edges, file_path);
        return (nodes, edges);
    }

    (nodes, edges)
}

fn extract_python_documentation_directives(
    file_path: &FilePath,
    source: &[u8],
    nodes: &[ParsedNode],
    edges: &mut Vec<ParsedEdge>,
) {
    let text = String::from_utf8_lossy(source);
    for directive in extract_line_comment_dagayn_directives(&text, &["#"]) {
        let source = nearest_documentation_source(file_path, nodes, directive.line);
        push_documentation_directive_edge(
            edges,
            source,
            file_path,
            "python",
            &directive,
            "comment_directive",
        );
    }
}

pub(super) fn parse_notebook_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let Ok(notebook) = serde_json::from_slice::<Value>(source) else {
        return (Vec::new(), Vec::new());
    };
    let Some(default_language) = notebook_kernel_language(&notebook) else {
        // Kernel language is not natively parsed (Julia, Scala, SQL, ...).
        // Keep the file discoverable instead of silently dropping it: an
        // empty parse causes the store to delete the file's previous nodes.
        let kernel = notebook
            .pointer("/metadata/kernelspec/language")
            .and_then(Value::as_str)
            .unwrap_or("notebook");
        return (
            vec![notebook_file_node(
                &file_path,
                1,
                kernel,
                is_test_file(&file_path),
                None,
            )],
            Vec::new(),
        );
    };
    let cells = collect_notebook_cells(&notebook, default_language);
    if cells.is_empty() {
        return (
            vec![notebook_file_node(
                &file_path,
                1,
                default_language,
                is_test_file(&file_path),
                None,
            )],
            Vec::new(),
        );
    }
    parse_notebook_cells_with_parser(
        &file_path,
        &cells,
        default_language,
        None,
        parser,
        repo_root,
    )
}

struct PythonParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    repo_root: Option<&'a Path>,
    import_map: &'a HashMap<String, String>,
    top_level_defined_names: &'a HashSet<String>,
    protocol_names: &'a HashSet<String>,
    bindings: RefCell<MemberCallBindings>,
}

#[derive(Clone)]
struct NotebookCell {
    cell_index: i64,
    language: &'static str,
    source: String,
    name: Option<String>,
    refs: Vec<String>,
    defs: Vec<String>,
}

impl NotebookCell {
    fn new(cell_index: i64, language: &'static str, source: String) -> Self {
        Self {
            cell_index,
            language,
            source,
            name: None,
            refs: Vec::new(),
            defs: Vec::new(),
        }
    }
}

fn is_databricks_py_source(source: &[u8]) -> bool {
    let first_line = source
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    first_line.trim_ascii() == b"# Databricks notebook source"
}

fn looks_like_marimo_py(source: &[u8]) -> bool {
    contains_bytes(source, b"marimo")
        && (contains_bytes(source, b"@app.cell")
            || contains_bytes(source, b"@app.function")
            || contains_bytes(source, b"@app.class_definition")
            || contains_bytes(source, b"app.setup"))
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

pub(super) fn looks_like_marimo_md(source: &[u8]) -> bool {
    let text = String::from_utf8_lossy(source);
    has_marimo_version_frontmatter(&text) || !collect_marimo_md_cells(&text).is_empty()
}

pub(super) fn parse_marimo_md_with_parser(
    file_path: &str,
    source: &[u8],
    markdown_parser: Option<&mut tree_sitter::Parser>,
    python_parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let (mut nodes, mut edges) =
        super::markdown::parse_markdown_with_parser(file_path, source, markdown_parser);
    if let Some(file) = nodes
        .iter_mut()
        .find(|node| node.kind == crate::core::types::NodeKind::File)
    {
        set_node_extra_str(file, "notebook_format", "marimo");
    }
    let file_path = FilePath::new(file_path);
    let text = String::from_utf8_lossy(source);
    let cells = collect_marimo_md_cells(&text);
    if cells.is_empty() {
        return (nodes, edges);
    }
    let (cell_nodes, mut cell_edges) = parse_notebook_cells_with_parser(
        &file_path,
        &cells,
        "python",
        Some("marimo"),
        python_parser,
        repo_root,
    );
    nodes.extend(
        cell_nodes
            .into_iter()
            .filter(|node| node.kind != crate::core::types::NodeKind::File),
    );
    edges.append(&mut cell_edges);
    (nodes, edges)
}

fn has_marimo_version_frontmatter(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text.strip_prefix("---") else {
        return false;
    };
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        return false;
    };
    rest[..end].lines().any(|line| {
        let line = line.trim();
        line == "marimo-version" || line.starts_with("marimo-version:")
    })
}

fn collect_marimo_md_cells(text: &str) -> Vec<NotebookCell> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut cells = Vec::new();
    let mut cell_index = 0_i64;
    scan_markdown_fences(&lines, |opener, body_lines| {
        if !is_marimo_md_code_tag(&opener.info) {
            return;
        }
        let language = marimo_md_fence_language(&opener.info);
        if language == "markdown" {
            return;
        }
        let attrs = marimo_md_fence_attrs(&opener.info);
        let name = attrs
            .get("name")
            .cloned()
            .filter(|name| !is_default_marimo_cell_name(name));
        let defs = attrs.get("query").cloned().into_iter().collect::<Vec<_>>();
        let body = body_lines.join("\n");
        if !body.trim().is_empty() {
            cells.push(NotebookCell {
                cell_index,
                language,
                source: with_trailing_newline(body),
                name,
                refs: Vec::new(),
                defs,
            });
        }
        cell_index += 1;
    });
    cells
}

struct MdFenceOpener {
    marker: u8,
    len: usize,
    indent: usize,
    info: String,
}

fn scan_markdown_fences(lines: &[&str], mut on_fence: impl FnMut(&MdFenceOpener, &[&str])) {
    let mut index = 0;
    while index < lines.len() {
        let Some(opener) = parse_md_fence_opener(lines[index]) else {
            index += 1;
            continue;
        };
        let mut end = index + 1;
        let mut closed = false;
        while end < lines.len() {
            if is_md_fence_closer(lines[end], &opener) {
                closed = true;
                break;
            }
            end += 1;
        }
        if closed {
            on_fence(&opener, &lines[index + 1..end]);
            index = end + 1;
        } else {
            index += 1;
        }
    }
}

fn parse_md_fence_opener(line: &str) -> Option<MdFenceOpener> {
    let indent = leading_ws_len(line);
    let rest = &line[indent..];
    let bytes = rest.as_bytes();
    let marker = *bytes.first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = bytes.iter().take_while(|byte| **byte == marker).count();
    if len < 3 {
        return None;
    }
    let info = rest[len..].trim();
    if marker == b'`' && info.contains('`') {
        return None;
    }
    Some(MdFenceOpener {
        marker,
        len,
        indent,
        info: info.to_string(),
    })
}

fn is_md_fence_closer(line: &str, opener: &MdFenceOpener) -> bool {
    let indent = leading_ws_len(line);
    if indent > opener.indent + 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    let bytes = rest.as_bytes();
    let fence_len = bytes
        .iter()
        .take_while(|byte| **byte == opener.marker)
        .count();
    fence_len >= opener.len && bytes[fence_len..].iter().all(u8::is_ascii_whitespace)
}

fn is_marimo_md_code_tag(info: &str) -> bool {
    MARIMO_MD_PYTHON_TAG_RE.is_match(info)
        || MARIMO_MD_SQL_TAG_RE.is_match(info)
        || MARIMO_MD_MARIMO_TAG_RE.is_match(info)
}

fn marimo_md_fence_language(info: &str) -> &'static str {
    let trimmed = info.trim();
    let lang = trimmed
        .split_once('{')
        .map(|(lang, _)| lang.trim())
        .unwrap_or(trimmed);
    if lang.eq_ignore_ascii_case("sql") {
        return "sql";
    }
    if lang.eq_ignore_ascii_case("markdown") || lang.eq_ignore_ascii_case("md") {
        return "markdown";
    }
    if let Some((_, attrs)) = trimmed.split_once('{') {
        let attrs = attrs.trim_end_matches('}').trim();
        if MARIMO_MD_ATTR_SQL_RE.is_match(attrs) {
            return "sql";
        }
        if MARIMO_MD_ATTR_MARKDOWN_RE.is_match(attrs) {
            return "markdown";
        }
    }
    "python"
}

fn marimo_md_fence_attrs(info: &str) -> HashMap<String, String> {
    MARIMO_MD_ATTR_RE
        .captures_iter(info)
        .filter_map(|captures| {
            Some((
                captures.get(1)?.as_str().to_string(),
                captures.get(2)?.as_str().to_string(),
            ))
        })
        .collect()
}

fn is_default_marimo_cell_name(name: &str) -> bool {
    name.is_empty() || name == "_" || name == "__"
}

fn parse_marimo_py_with_parser(
    file_path: &FilePath,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let Some(parser) = parser else {
        return parse_python_module_with_parser(file_path, source, None, repo_root);
    };
    let Some(tree) = parser.parse(source, None) else {
        return parse_python_module_with_parser(file_path, source, None, repo_root);
    };
    let root = tree.root_node();
    if !is_marimo_notebook(root, source) {
        return parse_python_module_with_parser(file_path, source, Some(parser), repo_root);
    }
    let (cells, mut sql_edges) = collect_marimo_cells(file_path, root, source);
    drop(tree);
    if cells.is_empty() {
        return (
            vec![notebook_file_node(
                file_path,
                line_count(source),
                "python",
                is_test_file(file_path),
                Some("marimo"),
            )],
            sql_edges,
        );
    }
    let (nodes, mut edges) = parse_notebook_cells_with_parser(
        file_path,
        &cells,
        "python",
        Some("marimo"),
        Some(parser),
        repo_root,
    );
    edges.append(&mut sql_edges);
    (nodes, edges)
}

fn is_marimo_notebook(root: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut has_import = false;
    let mut has_cell = false;
    for child in collect_named_children(root) {
        if !has_import && is_marimo_import(child, source) {
            has_import = true;
        }
        if !has_cell && is_marimo_cell_construct(child, source) {
            has_cell = true;
        }
        if has_import && has_cell {
            return true;
        }
    }
    false
}

fn is_marimo_import(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    match node.kind() {
        "import_statement" | "import_from_statement" => {
            marimo_import_text_matches(&node_text(node, source))
        }
        _ => false,
    }
}

fn marimo_import_text_matches(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == "import marimo"
        || trimmed.starts_with("import marimo as ")
        || trimmed.starts_with("import marimo,")
        || trimmed.starts_with("import marimo.")
        || trimmed.starts_with("from marimo import ")
        || trimmed.starts_with("from marimo.")
}

fn is_marimo_cell_construct(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    marimo_cell_kind(node, source).is_some() || is_marimo_unparsable_cell(node, source)
}

fn marimo_cell_kind(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<MarimoCellKind> {
    match node.kind() {
        "with_statement" if is_marimo_setup_with(node, source) => Some(MarimoCellKind::Setup),
        "decorated_definition" => marimo_decorator_kind(node, source),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarimoCellKind {
    Setup,
    Cell,
    Function,
    Class,
}

fn marimo_decorator_kind(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<MarimoCellKind> {
    python_decorator_names(node, source)
        .into_iter()
        .find_map(|name| match name.as_str() {
            "app.cell" => Some(MarimoCellKind::Cell),
            "app.function" => Some(MarimoCellKind::Function),
            "app.class_definition" => Some(MarimoCellKind::Class),
            _ => None,
        })
}

fn is_marimo_setup_with(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| match child.kind() {
        "attribute" => node_text(child, source) == "app.setup",
        "call" => {
            python_first_child(child).is_some_and(|callee| node_text(callee, source) == "app.setup")
        }
        "with_clause" | "with_item" => is_marimo_setup_with(child, source),
        _ => false,
    })
}

fn is_marimo_unparsable_cell(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    marimo_unparsable_call(node, source).is_some()
}

fn marimo_unparsable_call<'tree>(
    node: tree_sitter::Node<'tree>,
    source: &[u8],
) -> Option<tree_sitter::Node<'tree>> {
    match node.kind() {
        "call" if is_marimo_unparsable_callee(node, source) => Some(node),
        "expression_statement" => {
            let call = expression_statement_call(node)?;
            is_marimo_unparsable_callee(call, source).then_some(call)
        }
        _ => None,
    }
}

fn is_marimo_unparsable_callee(call: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    python_first_child(call).is_some_and(|callee| {
        let text = node_text(callee, source);
        text == "app._unparsable_cell" || text.ends_with("._unparsable_cell")
    }) || python_call_name(call, source).as_deref() == Some("_unparsable_cell")
}

fn marimo_unparsable_source(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut strings = Vec::new();
    collect_call_string_args(call, source, &mut strings);
    strings
        .into_iter()
        .next()
        .filter(|text| !text.trim().is_empty())
}

fn marimo_unparsable_name(call: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = call.walk();
    let args = call
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() != "keyword_argument" {
            continue;
        }
        if python_identifier_child(child, source).as_deref() != Some("name") {
            continue;
        }
        let mut strings = Vec::new();
        collect_string_literals(child, source, &mut strings);
        if let Some(name) = strings
            .into_iter()
            .next()
            .filter(|name| !is_default_marimo_cell_name(name))
        {
            return Some(name);
        }
    }
    None
}

fn expression_statement_call(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if node.kind() != "expression_statement" {
        return None;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == "call")
}

fn collect_marimo_cells(
    file_path: &FilePath,
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> (Vec<NotebookCell>, Vec<ParsedEdge>) {
    let mut cells = Vec::new();
    let mut sql_edges = Vec::new();
    let mut cell_index = 0_i64;
    for child in collect_named_children(root) {
        if let Some(call) = marimo_unparsable_call(child, source) {
            if let Some(cell_source) = marimo_unparsable_source(call, source) {
                cells.push(NotebookCell {
                    cell_index,
                    language: "python",
                    source: with_trailing_newline(cell_source),
                    name: marimo_unparsable_name(call, source),
                    refs: Vec::new(),
                    defs: Vec::new(),
                });
            }
            cell_index += 1;
            continue;
        }
        let Some(kind) = marimo_cell_kind(child, source) else {
            continue;
        };
        if kind == MarimoCellKind::Cell && is_marimo_markdown_only(child, source) {
            cell_index += 1;
            continue;
        }
        if let Some(cell_source) = marimo_cell_source(child, kind, source)
            && !cell_source.trim().is_empty()
        {
            collect_marimo_sql_imports(file_path, child, source, &mut sql_edges);
            let name = if kind == MarimoCellKind::Cell {
                marimo_cell_function_name(child, source)
                    .filter(|name| !is_default_marimo_cell_name(name))
            } else {
                None
            };
            let (refs, defs) = match kind {
                MarimoCellKind::Cell => marimo_cell_refs_defs(child, source),
                MarimoCellKind::Function | MarimoCellKind::Class => (
                    Vec::new(),
                    marimo_cell_symbol_name(child, kind, source)
                        .into_iter()
                        .collect(),
                ),
                MarimoCellKind::Setup => (Vec::new(), Vec::new()),
            };
            cells.push(NotebookCell {
                cell_index,
                language: "python",
                source: with_trailing_newline(cell_source),
                name,
                refs,
                defs,
            });
        }
        cell_index += 1;
    }
    (cells, sql_edges)
}

fn collect_named_children(node: tree_sitter::Node<'_>) -> Vec<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(tree_sitter::Node::is_named)
        .collect()
}

fn with_trailing_newline(mut source: String) -> String {
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source
}

fn marimo_cell_source(
    node: tree_sitter::Node<'_>,
    kind: MarimoCellKind,
    source: &[u8],
) -> Option<String> {
    match kind {
        MarimoCellKind::Setup => {
            let body = node.child_by_field_name("body")?;
            Some(block_source_without_trailing_return(body, source, false))
        }
        MarimoCellKind::Cell => {
            let function = decorated_definition_target(node, "function_definition")?;
            let body = function.child_by_field_name("body")?;
            Some(block_source_without_trailing_return(body, source, true))
        }
        MarimoCellKind::Function => {
            let function = decorated_definition_target(node, "function_definition")?;
            Some(node_text(function, source))
        }
        MarimoCellKind::Class => {
            let class = decorated_definition_target(node, "class_definition")?;
            Some(node_text(class, source))
        }
    }
}

fn marimo_cell_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let function = decorated_definition_target(node, "function_definition")?;
    python_identifier_child(function, source)
}

fn marimo_cell_symbol_name(
    node: tree_sitter::Node<'_>,
    kind: MarimoCellKind,
    source: &[u8],
) -> Option<String> {
    match kind {
        MarimoCellKind::Cell | MarimoCellKind::Function => marimo_cell_function_name(node, source),
        MarimoCellKind::Class => {
            let class = decorated_definition_target(node, "class_definition")?;
            python_identifier_child(class, source)
        }
        MarimoCellKind::Setup => None,
    }
}

fn marimo_cell_refs_defs(node: tree_sitter::Node<'_>, source: &[u8]) -> (Vec<String>, Vec<String>) {
    let Some(function) = decorated_definition_target(node, "function_definition") else {
        return (Vec::new(), Vec::new());
    };
    (
        python_function_param_names(function, source),
        python_function_return_names(function, source),
    )
}

fn python_function_param_names(function: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(params) = function.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut cursor = params.walk();
    for child in params.children(&mut cursor) {
        match child.kind() {
            "identifier" => names.push(node_text(child, source)),
            "typed_parameter"
            | "default_parameter"
            | "typed_default_parameter"
            | "list_splat_pattern"
            | "dictionary_splat_pattern" => {
                if let Some(name) = python_identifier_child(child, source) {
                    names.push(name);
                }
            }
            _ => {}
        }
    }
    names.retain(|name| !is_default_marimo_cell_name(name) && name != "self" && name != "cls");
    names
}

fn python_function_return_names(function: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(body) = function.child_by_field_name("body") else {
        return Vec::new();
    };
    let statements = named_block_statements(body);
    let Some(ret) = statements
        .last()
        .copied()
        .filter(|statement| statement.kind() == "return_statement")
    else {
        return Vec::new();
    };
    let mut names = Vec::new();
    python_collect_export_identifiers(ret, source, &mut names);
    names.retain(|name| !is_default_marimo_cell_name(name));
    names
}

fn python_collect_export_identifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<String>,
) {
    match node.kind() {
        "identifier" => out.push(node_text(node, source)),
        "call" | "attribute" | "subscript" => {}
        "return_statement"
        | "tuple"
        | "list"
        | "set"
        | "parenthesized_expression"
        | "expression_list"
        | "pattern_list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    python_collect_export_identifiers(child, source, out);
                }
            }
        }
        _ => {}
    }
}

fn decorated_definition_target<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
) -> Option<tree_sitter::Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn block_source_without_trailing_return(
    body: tree_sitter::Node<'_>,
    source: &[u8],
    strip_return: bool,
) -> String {
    let statements = named_block_statements(body);
    let kept = if strip_return
        && statements
            .last()
            .is_some_and(|statement| statement.kind() == "return_statement")
    {
        &statements[..statements.len().saturating_sub(1)]
    } else {
        statements.as_slice()
    };
    if kept.is_empty() {
        return String::new();
    }
    let start = kept[0].start_byte();
    let end = kept[kept.len() - 1].end_byte();
    if start >= end || end > source.len() {
        return String::new();
    }
    dedent_source(std::str::from_utf8(&source[start..end]).unwrap_or_default())
}

fn named_block_statements<'tree>(body: tree_sitter::Node<'tree>) -> Vec<tree_sitter::Node<'tree>> {
    let mut statements = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.is_named() && child.kind() != "comment" {
            statements.push(child);
        }
    }
    statements
}

fn dedent_source(text: &str) -> String {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let indent = lines
        .iter()
        .copied()
        .filter(|line| !trim_line_end(line).trim().is_empty())
        .map(leading_ws_len)
        .min()
        .unwrap_or(0);
    lines
        .into_iter()
        .map(|line| {
            if indent == 0 {
                return line.to_string();
            }
            let skip = indent.min(leading_ws_len(line));
            line[skip..].to_string()
        })
        .collect()
}

fn trim_line_end(line: &str) -> &str {
    line.strip_suffix('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .unwrap_or(line)
}

fn leading_ws_len(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn is_marimo_markdown_only(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let Some(function) = decorated_definition_target(node, "function_definition") else {
        return false;
    };
    let Some(body) = function.child_by_field_name("body") else {
        return false;
    };
    let mut statements = named_block_statements(body);
    if statements
        .last()
        .is_some_and(|statement| statement.kind() == "return_statement")
    {
        statements.pop();
    }
    !statements.is_empty()
        && statements
            .iter()
            .all(|statement| is_marimo_md_expression(*statement, source))
}

fn is_marimo_md_expression(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let call = if node.kind() == "call" {
        node
    } else if let Some(call) = expression_statement_call(node) {
        call
    } else {
        return false;
    };
    python_call_name(call, source).as_deref() == Some("md")
}

fn collect_marimo_sql_imports(
    file_path: &FilePath,
    node: tree_sitter::Node<'_>,
    source: &[u8],
    edges: &mut Vec<ParsedEdge>,
) {
    let mut sql_sources = Vec::new();
    collect_attribute_sql_strings(node, source, &mut sql_sources);
    for sql in sql_sources {
        push_sql_table_imports(file_path, &sql, edges);
    }
}

fn collect_attribute_sql_strings(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<String>,
) {
    if node.kind() == "call"
        && python_first_child(node).is_some_and(|callee| callee.kind() == "attribute")
        && python_call_name(node, source).as_deref() == Some("sql")
    {
        collect_call_string_args(node, source, out);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor).collect::<Vec<_>>() {
        collect_attribute_sql_strings(child, source, out);
    }
}

fn collect_call_string_args(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            collect_string_literals(child, source, out);
        }
    }
}

fn collect_string_literals(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<String>) {
    if node.kind() == "string"
        && let Some(text) = python_string_literal_text(node, source)
        && !text.trim().is_empty()
    {
        out.push(text);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_string_literals(child, source, out);
    }
}

fn python_string_literal_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            parts.push(node_text(child, source));
        }
    }
    if !parts.is_empty() {
        return Some(parts.concat());
    }
    let raw = node_text(node, source);
    Some(unquote_python_string(&raw))
}

fn unquote_python_string(raw: &str) -> String {
    let trimmed = raw.trim();
    let prefixes = [
        "fr", "Fr", "fR", "FR", "rf", "Rf", "rF", "RF", "f", "F", "r", "R", "b", "B", "u", "U",
    ];
    let mut body = trimmed;
    for prefix in prefixes {
        if let Some(rest) = body.strip_prefix(prefix) {
            body = rest;
            break;
        }
    }
    for quote in ["\"\"\"", "'''", "\"", "'"] {
        if let Some(inner) = body.strip_prefix(quote)
            && let Some(inner) = inner.strip_suffix(quote)
        {
            return inner.to_string();
        }
    }
    body.to_string()
}

fn push_sql_table_imports(file_path: &FilePath, sql: &str, edges: &mut Vec<ParsedEdge>) {
    for captures in NOTEBOOK_SQL_TABLE_RE.captures_iter(sql) {
        let Some(target) = captures.get(1).map(|capture| capture.as_str()) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: file_path.to_string(),
            target: target.replace('`', ""),
            file_path: file_path.clone(),
            line: 1,
            extra: json!({}),
        });
    }
}

fn parse_databricks_py_with_parser(
    file_path: &FilePath,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let cells = collect_databricks_py_cells(&text);
    if cells.is_empty() {
        return (
            vec![databricks_file_node(file_path, 1, is_test_file(file_path))],
            Vec::new(),
        );
    }
    parse_notebook_cells_with_parser(
        file_path,
        &cells,
        "python",
        Some("databricks_py"),
        parser,
        repo_root,
    )
}

fn parse_notebook_cells_with_parser(
    file_path: &FilePath,
    cells: &[NotebookCell],
    default_language: &'static str,
    notebook_format: Option<&'static str>,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut cell_offsets = Vec::new();
    let mut max_line = 1_i64;
    let mut parser = parser;
    let mut languages = Vec::<&'static str>::new();
    for cell in cells {
        if !languages.contains(&cell.language) {
            languages.push(cell.language);
        }
    }

    for language in languages {
        let lang_cells = cells
            .iter()
            .filter(|cell| cell.language == language)
            .cloned()
            .collect::<Vec<_>>();
        match language {
            "python" => {
                let (mut parsed_nodes, parsed_edges, offsets, current_line) =
                    parse_databricks_python_cells(
                        file_path,
                        &lang_cells,
                        parser.as_deref_mut(),
                        repo_root,
                    );
                nodes.append(&mut parsed_nodes);
                edges.extend(parsed_edges);
                cell_offsets.extend(offsets);
                max_line = max_line.max(current_line);
            }
            "sql" => {
                for cell in &lang_cells {
                    extract_databricks_sql_imports(file_path, cell, &mut edges);
                }
            }
            "r" => {
                let (mut parsed_nodes, parsed_edges, offsets, current_line) =
                    parse_databricks_r_cells(file_path, &lang_cells);
                nodes.append(&mut parsed_nodes);
                edges.extend(parsed_edges);
                cell_offsets.extend(offsets);
                max_line = max_line.max(current_line);
            }
            _ => {}
        }
    }

    let file_node = notebook_file_node(
        file_path,
        max_line,
        default_language,
        is_test_file(file_path),
        notebook_format,
    );
    nodes.insert(0, file_node);
    tag_notebook_cell_indices(&mut nodes, &cell_offsets);
    tag_notebook_cell_names(&mut nodes, cells);
    synthesize_named_notebook_cells(file_path, cells, &cell_offsets, &mut nodes, &mut edges);
    let edges = resolve_python_call_targets(&nodes, edges, file_path);
    let mut edges = add_python_tested_by_edges(&nodes, edges, file_path);
    add_marimo_cell_dataflow_edges(file_path, cells, &nodes, &mut edges);
    (nodes, edges)
}

fn databricks_file_node(file_path: &FilePath, line_end: i64, is_test: bool) -> ParsedNode {
    notebook_file_node(
        file_path,
        line_end,
        "python",
        is_test,
        Some("databricks_py"),
    )
}

fn notebook_kernel_language(notebook: &Value) -> Option<&'static str> {
    let language = notebook
        .pointer("/metadata/kernelspec/language")
        .and_then(Value::as_str)
        .or_else(|| {
            notebook
                .pointer("/metadata/language_info/name")
                .and_then(Value::as_str)
        })
        .unwrap_or("python")
        .to_ascii_lowercase();
    match language.as_str() {
        "python" => Some("python"),
        "r" => Some("r"),
        _ => None,
    }
}

fn collect_notebook_cells(notebook: &Value, default_language: &'static str) -> Vec<NotebookCell> {
    let Some(cells) = notebook.get("cells").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (cell_index, cell) in cells.iter().enumerate() {
        if cell.get("cell_type").and_then(Value::as_str) != Some("code") {
            continue;
        }
        let lines = notebook_source_lines(cell.get("source"));
        if lines.is_empty() {
            continue;
        }
        let first_line = lines[0].trim();
        let mut cell_language = default_language;
        let mut cell_lines = lines.as_slice();
        if first_line == "%python" || first_line.starts_with("%python ") {
            cell_language = "python";
            cell_lines = &lines[1..];
        } else if first_line == "%sql" || first_line.starts_with("%sql ") {
            cell_language = "sql";
            cell_lines = &lines[1..];
        } else if first_line == "%r" || first_line.starts_with("%r ") {
            cell_language = "r";
            cell_lines = &lines[1..];
        } else if first_line == "%scala"
            || first_line.starts_with("%scala ")
            || first_line == "%md"
            || first_line.starts_with("%md ")
            || first_line == "%sh"
            || first_line.starts_with("%sh ")
        {
            continue;
        }

        let filtered = if matches!(cell_language, "python" | "r") {
            cell_lines
                .iter()
                .filter(|line| {
                    let trimmed = line.trim_start();
                    !trimmed.starts_with('%') && !trimmed.starts_with('!')
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            cell_lines.to_vec()
        };
        if filtered.is_empty() {
            continue;
        }
        out.push(NotebookCell::new(
            cell_index as i64,
            cell_language,
            filtered.join(""),
        ));
    }
    out
}

fn notebook_source_lines(source: Option<&Value>) -> Vec<String> {
    match source {
        Some(Value::Array(lines)) => lines
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => split_lines_keepends(text),
        _ => Vec::new(),
    }
}

fn split_lines_keepends(text: &str) -> Vec<String> {
    let mut lines = text
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() && !text.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

fn notebook_file_node(
    file_path: &FilePath,
    line_end: i64,
    language: &str,
    is_test: bool,
    notebook_format: Option<&str>,
) -> ParsedNode {
    ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: notebook_format
            .map(|format| json!({"notebook_format": format}))
            .unwrap_or_else(|| json!({})),
    }
}

fn collect_databricks_py_cells(text: &str) -> Vec<NotebookCell> {
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if lines
        .first()
        .is_some_and(|line| line.trim() == "# Databricks notebook source")
    {
        lines.remove(0);
    }

    let mut chunks = vec![Vec::<&str>::new()];
    for line in lines {
        if is_databricks_command_line(line) {
            chunks.push(Vec::new());
        } else if let Some(chunk) = chunks.last_mut() {
            chunk.push(line);
        }
    }

    let mut cells = Vec::new();
    for (cell_index, chunk) in chunks.into_iter().enumerate() {
        let non_empty = chunk
            .iter()
            .copied()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if non_empty.is_empty() {
            continue;
        }
        let first_line = non_empty[0];
        let all_magic = non_empty.iter().all(|line| line.starts_with("# MAGIC "));
        let magic_language = if all_magic && first_line.starts_with("# MAGIC %sql") {
            Some("sql")
        } else if all_magic && first_line.starts_with("# MAGIC %r") {
            Some("r")
        } else {
            None
        };
        if let Some(language) = magic_language {
            let mut stripped = chunk
                .iter()
                .map(|line| line.strip_prefix("# MAGIC ").unwrap_or(line).to_string())
                .collect::<Vec<_>>();
            if let Some(first_directive) = stripped
                .iter()
                .find(|line| !line.trim().is_empty())
                .filter(|line| line.trim().starts_with('%'))
                .cloned()
            {
                stripped.retain(|line| line != &first_directive);
            }
            cells.push(NotebookCell::new(
                cell_index as i64,
                language,
                stripped.join("\n"),
            ));
            continue;
        }
        if all_magic
            && (first_line.starts_with("# MAGIC %md") || first_line.starts_with("# MAGIC %sh"))
        {
            continue;
        }
        let source = chunk
            .iter()
            .copied()
            .filter(|line| !line.starts_with("# MAGIC "))
            .collect::<Vec<_>>()
            .join("\n");
        cells.push(NotebookCell::new(cell_index as i64, "python", source));
    }
    cells
}

fn is_databricks_command_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("# COMMAND") && trimmed[9..].trim().bytes().all(|byte| byte == b'-')
}

type NotebookOffsets = Vec<(i64, i64, i64)>;

fn parse_databricks_python_cells(
    file_path: &FilePath,
    cells: &[NotebookCell],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>, NotebookOffsets, i64) {
    let (source, offsets, current_line) = concatenate_notebook_cells(cells);
    let (nodes, edges) =
        parse_python_module_with_parser(file_path, source.as_bytes(), parser, repo_root);
    (
        nodes
            .into_iter()
            .filter(|node| node.kind != "File")
            .collect(),
        edges,
        offsets,
        current_line,
    )
}

fn parse_databricks_r_cells(
    file_path: &FilePath,
    cells: &[NotebookCell],
) -> (Vec<ParsedNode>, Vec<ParsedEdge>, NotebookOffsets, i64) {
    let (source, offsets, current_line) = concatenate_notebook_cells(cells);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut current_function: Option<String> = None;
    for (index, line) in lines.iter().enumerate() {
        let line_no = index as i64 + 1;
        if let Some(captures) = NOTEBOOK_R_FUNCTION_RE.captures(line) {
            let Some(name) = captures.get(1).map(|capture| capture.as_str()) else {
                continue;
            };
            let params = captures.get(2).map(|capture| capture.as_str().to_string());
            let line_end = find_r_function_end(&lines, index);
            let qualified = qualify(file_path, name, None);
            nodes.push(ParsedNode {
                kind: crate::core::types::NodeKind::Function,
                name: name.to_string(),
                file_path: file_path.clone(),
                line_start: line_no,
                line_end,
                language: "r".to_string(),
                parent_name: None,
                params,
                return_type: None,
                modifiers: None,
                is_test: false,
                extra: json!({}),
            });
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::Contains,
                source: file_path.to_string(),
                target: qualified.clone(),
                file_path: file_path.clone(),
                line: line_no,
                extra: json!({}),
            });
            current_function = Some(qualified);
            continue;
        }
        let Some(caller) = current_function.as_ref() else {
            continue;
        };
        for captures in NOTEBOOK_R_CALL_RE.captures_iter(line) {
            let Some(name) = captures.get(1).map(|capture| capture.as_str()) else {
                continue;
            };
            if name == "function" {
                continue;
            }
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::Calls,
                source: caller.clone(),
                target: name.to_string(),
                file_path: file_path.clone(),
                line: line_no,
                extra: json!({}),
            });
        }
    }
    (nodes, edges, offsets, current_line)
}

fn concatenate_notebook_cells(cells: &[NotebookCell]) -> (String, NotebookOffsets, i64) {
    let mut chunks = Vec::new();
    let mut offsets = Vec::new();
    let mut current_line = 1_i64;
    for cell in cells {
        let line_count = cell.source.matches('\n').count() as i64
            + if cell.source.ends_with('\n') { 0 } else { 1 };
        offsets.push((cell.cell_index, current_line, current_line + line_count - 1));
        chunks.push(cell.source.clone());
        current_line += line_count + 1;
    }
    (chunks.join("\n"), offsets, current_line)
}

fn find_r_function_end(lines: &[&str], start: usize) -> i64 {
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, line)| line.trim() == "}")
        .map(|(index, _)| index as i64 + 1)
        .unwrap_or(start as i64 + 1)
}

fn extract_databricks_sql_imports(
    file_path: &FilePath,
    cell: &NotebookCell,
    edges: &mut Vec<ParsedEdge>,
) {
    for captures in NOTEBOOK_SQL_TABLE_RE.captures_iter(&cell.source) {
        let Some(target) = captures.get(1).map(|capture| capture.as_str()) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::ImportsFrom,
            source: file_path.to_string(),
            target: target.replace('`', ""),
            file_path: file_path.clone(),
            line: 1,
            extra: json!({}),
        });
    }
}

fn tag_notebook_cell_indices(nodes: &mut [ParsedNode], offsets: &[(i64, i64, i64)]) {
    for node in nodes {
        if node.kind == "File" {
            continue;
        }
        let mut best = None;
        let mut best_overlap = -1_i64;
        for (cell_index, start, end) in offsets {
            let overlap = node.line_end.min(*end) - node.line_start.max(*start) + 1;
            if overlap > best_overlap && overlap > 0 {
                best_overlap = overlap;
                best = Some(*cell_index);
            }
        }
        if let Some(cell_index) = best {
            set_node_extra_i64(node, "cell_index", cell_index);
        }
    }
}

fn set_node_extra_i64(node: &mut ParsedNode, key: &str, value: i64) {
    if !node.extra.is_object() {
        node.extra = json!({});
    }
    if let Some(map) = node.extra.as_object_mut() {
        map.insert(key.to_string(), json!(value));
    }
}

fn set_node_extra_str(node: &mut ParsedNode, key: &str, value: &str) {
    if !node.extra.is_object() {
        node.extra = json!({});
    }
    if let Some(map) = node.extra.as_object_mut() {
        map.insert(key.to_string(), json!(value));
    }
}

fn node_cell_index(node: &ParsedNode) -> Option<i64> {
    node.extra
        .get("cell_index")
        .and_then(serde_json::Value::as_i64)
}

fn tag_notebook_cell_names(nodes: &mut [ParsedNode], cells: &[NotebookCell]) {
    for node in nodes {
        if node.kind == crate::core::types::NodeKind::File {
            continue;
        }
        let Some(cell_index) = node_cell_index(node) else {
            continue;
        };
        let Some(name) = cells
            .iter()
            .find(|cell| cell.cell_index == cell_index)
            .and_then(|cell| cell.name.as_deref())
        else {
            continue;
        };
        set_node_extra_str(node, "cell_name", name);
    }
}

fn synthesize_named_notebook_cells(
    file_path: &FilePath,
    cells: &[NotebookCell],
    offsets: &[(i64, i64, i64)],
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    for cell in cells {
        let Some(name) = cell.name.as_deref() else {
            continue;
        };
        if is_default_marimo_cell_name(name) {
            continue;
        }
        let already_present = nodes.iter().any(|node| {
            node.kind != crate::core::types::NodeKind::File
                && node_cell_index(node) == Some(cell.cell_index)
                && node.name == name
        });
        if already_present {
            continue;
        }
        let (line_start, line_end) = offsets
            .iter()
            .find(|(index, _, _)| *index == cell.cell_index)
            .map(|(_, start, end)| (*start, *end))
            .unwrap_or((1, 1));
        let is_test = python_name_matches_test_pattern(name);
        let qualified = qualify(file_path, name, None);
        let extra = json!({
            "cell_index": cell.cell_index,
            "cell_name": name,
            "synthesized_from": "marimo_cell_name",
        });
        nodes.push(ParsedNode {
            kind: if is_test {
                crate::core::types::NodeKind::Test
            } else {
                crate::core::types::NodeKind::Function
            },
            name: name.to_string(),
            file_path: file_path.clone(),
            line_start,
            line_end,
            language: cell.language.to_string(),
            parent_name: None,
            params: if cell.refs.is_empty() {
                None
            } else {
                Some(format!("({})", cell.refs.join(", ")))
            },
            return_type: None,
            modifiers: None,
            is_test,
            extra,
        });
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: file_path.to_string(),
            target: qualified,
            file_path: file_path.clone(),
            line: line_start,
            extra: json!({}),
        });
    }
}

fn add_marimo_cell_dataflow_edges(
    file_path: &FilePath,
    cells: &[NotebookCell],
    nodes: &[ParsedNode],
    edges: &mut Vec<ParsedEdge>,
) {
    if cells
        .iter()
        .all(|cell| cell.refs.is_empty() && cell.defs.is_empty())
    {
        return;
    }
    let mut def_targets = HashMap::new();
    for cell in cells {
        for def in &cell.defs {
            if let Some(node) = nodes.iter().find(|node| {
                node.kind != crate::core::types::NodeKind::File
                    && node.parent_name.is_none()
                    && node_cell_index(node) == Some(cell.cell_index)
                    && node.name == *def
            }) {
                def_targets.insert(
                    def.clone(),
                    qualify(file_path, &node.name, node.parent_name.as_deref()),
                );
            }
        }
    }
    for cell in cells {
        if cell.refs.is_empty() {
            continue;
        }
        let sources = nodes
            .iter()
            .filter(|node| {
                node.kind != crate::core::types::NodeKind::File
                    && node.parent_name.is_none()
                    && node_cell_index(node) == Some(cell.cell_index)
            })
            .map(|node| qualify(file_path, &node.name, node.parent_name.as_deref()))
            .collect::<Vec<_>>();
        if sources.is_empty() {
            continue;
        }
        for ref_name in &cell.refs {
            let Some(target) = def_targets.get(ref_name) else {
                continue;
            };
            for source in &sources {
                if source == target {
                    continue;
                }
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::DependsOn,
                    source: source.clone(),
                    target: target.clone(),
                    file_path: file_path.clone(),
                    line: 1,
                    extra: json!({"reason": "marimo_cell_ref"}),
                });
            }
        }
    }
}

fn python_walk_children(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_qualified: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_definition" => {
                if let Some(name) = python_identifier_child(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, enclosing_class);
                    let bases = python_class_base_names(child, context.source);
                    let decorators = python_parent_decorators(child, context.source);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Class,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: python_class_extra(&bases, &decorators),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: context.file_path.to_string(),
                        target: qualified.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    python_emit_bases(child, context, &qualified, &bases, edges);
                    python_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = python_identifier_child(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, enclosing_class);
                    let params = python_child_text(child, context.source, "parameters");
                    let return_type = python_return_type(child, context.source);
                    let is_test =
                        python_is_test_function(&name, &context.file_path, child, context.source);
                    let decorators = python_parent_decorators(child, context.source);
                    let mut extra = json!({});
                    if !decorators.is_empty() {
                        extra["decorators"] = json!(decorators);
                    }
                    if decorators
                        .iter()
                        .any(|decorator| decorator.rsplit('.').next() == Some("abstractmethod"))
                    {
                        extra["is_abstract"] = json!(true);
                    }
                    nodes.push(ParsedNode {
                        kind: if is_test {
                            crate::core::types::NodeKind::Test
                        } else {
                            crate::core::types::NodeKind::Function
                        },
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params,
                        return_type,
                        modifiers: None,
                        is_test,
                        extra,
                    });
                    let container = enclosing_class
                        .map(|name| qualify(&context.file_path, name, None))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: container,
                        target: qualified.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    let snapshot = context.bindings.borrow().snapshot();
                    if let Some(class_name) = enclosing_class {
                        context
                            .bindings
                            .borrow_mut()
                            .bind_implicit_receivers(class_name);
                    }
                    python_walk_children(
                        child,
                        context,
                        enclosing_class,
                        Some(&qualified),
                        nodes,
                        edges,
                    );
                    context.bindings.borrow_mut().restore(snapshot);
                    continue;
                }
            }
            "type_alias_statement" => {
                if let Some(name) = python_type_alias_name(child, context.source) {
                    let qualified = qualify(&context.file_path, &name, enclosing_class);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Type,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({"type_role": "alias"}),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: context.file_path.to_string(),
                        target: qualified,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    continue;
                }
            }
            "import_statement" | "import_from_statement" => {
                for target in python_import_targets(
                    child,
                    context.source,
                    &context.file_path,
                    context.repo_root,
                ) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            "call" => {
                if let Some(call_name) = python_call_name(child, context.source) {
                    let caller = enclosing_qualified.unwrap_or(&context.file_path);
                    let target = python_bound_member_target(child, context)
                        .or_else(|| python_resolve_imported_call_target(&call_name, context))
                        .unwrap_or(call_name);
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Calls,
                        source: caller.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    if let Some(edge) =
                        python_bridge_edge(child, context.source, &context.file_path, caller)
                    {
                        edges.push(edge);
                    }
                }
            }
            "pair" | "assignment" | "list" => {
                python_emit_value_references(
                    child,
                    context,
                    enclosing_qualified.unwrap_or(&context.file_path),
                    edges,
                );
            }
            _ => {}
        }
        python_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_qualified,
            nodes,
            edges,
        );
        python_bind_assignment(child, context);
    }
}

fn python_emit_value_references(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    caller: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    match node.kind() {
        "pair" => {
            if let Some(value_node) = python_last_value_child(node)
                && value_node.kind() == "identifier"
            {
                python_emit_reference_if_known(value_node, context, caller, edges);
            }
        }
        "assignment" => {
            let Some(lhs) = python_first_child(node) else {
                return;
            };
            if !matches!(lhs.kind(), "attribute" | "subscript") {
                return;
            }
            if let Some(rhs) = python_last_value_child(node)
                && rhs.kind() == "identifier"
            {
                python_emit_reference_if_known(rhs, context, caller, edges);
            }
        }
        "list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    python_emit_reference_if_known(child, context, caller, edges);
                }
            }
        }
        _ => {}
    }
}

fn python_last_value_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut last = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !matches!(
            child.kind(),
            ":" | "," | "=" | "comment" | "type_annotation"
        ) {
            last = Some(child);
        }
    }
    last
}

fn python_first_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();

    node.children(&mut cursor).next()
}

fn python_emit_reference_if_known(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    caller: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let name = node_text(node, context.source);
    let Some(target) = python_resolve_reference_target(&name, context) else {
        return;
    };
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::References,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn python_resolve_reference_target(name: &str, context: &PythonParseContext<'_>) -> Option<String> {
    if python_skip_value_reference_name(name) {
        return None;
    }
    if context.top_level_defined_names.contains(name) {
        return Some(qualify(&context.file_path, name, None));
    }
    let module = context.import_map.get(name)?;
    Some(
        python_resolve_module_to_file(module, &context.file_path, context.repo_root)
            .map(|resolved| qualify(&resolved, name, None))
            .unwrap_or_else(|| name.to_string()),
    )
}

fn python_skip_value_reference_name(name: &str) -> bool {
    name.is_empty()
        || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
        || matches!(
            name,
            "true"
                | "false"
                | "null"
                | "undefined"
                | "None"
                | "True"
                | "False"
                | "self"
                | "this"
                | "cls"
                | "super"
        )
}

fn collect_python_file_scope(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> (HashMap<String, String>, HashSet<String>, HashSet<String>) {
    let mut import_map = HashMap::new();
    let mut defined_names = HashSet::new();
    let mut protocol_names = HashSet::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let target = if child.kind() == "decorated_definition" {
            python_decorated_target(child)
        } else {
            Some(child)
        };
        if let Some(target) = target {
            match target.kind() {
                "class_definition" => {
                    if let Some(name) = python_identifier_child(target, source) {
                        if python_class_base_names(target, source)
                            .iter()
                            .any(|base| python_is_protocol_marker(base))
                        {
                            protocol_names.insert(name.clone());
                        }
                        defined_names.insert(name);
                    }
                }
                "function_definition" | "type_alias_statement" => {
                    if let Some(name) = python_identifier_child(target, source)
                        .or_else(|| python_type_alias_name(target, source))
                    {
                        defined_names.insert(name);
                    }
                }
                "import_statement" | "import_from_statement" => {
                    collect_python_import_names(target, source, &mut import_map);
                }
                _ => {}
            }
        }
    }
    (import_map, defined_names, protocol_names)
}

fn python_decorated_target(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_definition"
                | "function_definition"
                | "import_statement"
                | "import_from_statement"
        ) {
            return Some(child);
        }
    }
    None
}

fn collect_python_import_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &mut HashMap<String, String>,
) {
    if node.kind() != "import_from_statement" {
        return;
    }

    let mut module = None;
    let mut seen_import = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" if !seen_import => {
                module = Some(node_text(child, source));
            }
            "import" => {
                seen_import = true;
            }
            "identifier" | "dotted_name" if seen_import => {
                if let Some(module) = &module {
                    import_map.insert(node_text(child, source), module.clone());
                }
            }
            "aliased_import" if seen_import => {
                if let Some(module) = &module
                    && let Some(name) = python_aliased_import_name(child, source)
                {
                    import_map.insert(name, module.clone());
                }
            }
            _ => {}
        }
    }
}

fn python_aliased_import_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| matches!(child.kind(), "identifier" | "dotted_name"))
        .map(|child| node_text(child, source))
        .last()
}

fn python_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn python_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn python_return_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for (index, child) in children.iter().enumerate() {
        if child.kind() == "->" {
            return children
                .get(index + 1)
                .map(|return_node| node_text(*return_node, source));
        }
    }
    None
}

fn python_is_test_function(
    name: &str,
    file_path: &FilePath,
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> bool {
    python_name_matches_test_pattern(name)
        || (is_test_file(file_path) && python_is_test_runner_name(name))
        || python_has_test_annotation(node, source)
}

fn python_name_matches_test_pattern(name: &str) -> bool {
    name.starts_with("test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_spec")
}

fn python_is_test_runner_name(name: &str) -> bool {
    matches!(
        name,
        "describe" | "it" | "test" | "beforeEach" | "afterEach" | "beforeAll" | "afterAll"
    )
}

fn python_has_test_annotation(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "decorated_definition" {
        return false;
    }
    let mut cursor = parent.walk();

    parent.children(&mut cursor).any(|child| {
        if child.kind() != "decorator" {
            return false;
        }
        let text = node_text(child, source);
        matches!(
            text.trim_start_matches('@').trim(),
            "Test"
                | "ParameterizedTest"
                | "RepeatedTest"
                | "TestFactory"
                | "org.junit.Test"
                | "org.junit.jupiter.api.Test"
        )
    })
}

fn python_emit_bases(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    qualified: &str,
    bases: &[String],
    edges: &mut Vec<ParsedEdge>,
) {
    for base in bases {
        let (kind, role) = if python_is_protocol_marker(base)
            || python_is_abc_marker(base)
            || python_is_typed_dict_marker(base)
        {
            (crate::core::types::EdgeKind::Inherits, "extends")
        } else if context.protocol_names.contains(base) {
            (crate::core::types::EdgeKind::Implements, "implements")
        } else {
            (crate::core::types::EdgeKind::Inherits, "extends")
        };
        edges.push(ParsedEdge {
            kind,
            source: qualified.to_string(),
            target: base.clone(),
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": role,
                "syntax_source": node.kind(),
            }),
        });
    }
}

fn python_class_base_names(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut bases = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "argument_list" {
            continue;
        }
        let mut arg_cursor = child.walk();
        for arg in child.children(&mut arg_cursor) {
            if let Some(name) = python_base_name(arg, source) {
                bases.push(name);
            }
        }
    }
    bases
}

fn python_base_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "attribute" => Some(node_text(node, source)),
        "subscript" => {
            let mut cursor = node.walk();
            let children = node.children(&mut cursor).collect::<Vec<_>>();
            children
                .into_iter()
                .find_map(|child| python_base_name(child, source))
        }
        _ => None,
    }
}

fn python_class_extra(bases: &[String], decorators: &[String]) -> Value {
    let is_protocol = bases.iter().any(|base| python_is_protocol_marker(base));
    let is_abc = bases.iter().any(|base| python_is_abc_marker(base));
    let is_typed_dict = bases.iter().any(|base| python_is_typed_dict_marker(base));
    let type_role = if is_protocol {
        "protocol"
    } else if is_abc {
        "abstract_class"
    } else if is_typed_dict {
        "typed_dict"
    } else {
        "class"
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_protocol || is_abc {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_protocol {
            map.insert("is_contract".to_string(), json!(true));
        }
        if !decorators.is_empty() {
            map.insert("decorators".to_string(), json!(decorators));
        }
    }
    extra
}

fn python_is_protocol_marker(name: &str) -> bool {
    name.rsplit('.').next().unwrap_or(name) == "Protocol"
}

fn python_is_abc_marker(name: &str) -> bool {
    matches!(name.rsplit('.').next().unwrap_or(name), "ABC" | "ABCMeta")
}

fn python_is_typed_dict_marker(name: &str) -> bool {
    name.rsplit('.').next().unwrap_or(name) == "TypedDict"
}

fn python_parent_decorators(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(parent) = node.parent() else {
        return Vec::new();
    };
    if parent.kind() != "decorated_definition" {
        return Vec::new();
    }
    python_decorator_names(parent, source)
}

fn python_decorator_names(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "decorator" {
            continue;
        }
        if let Some(name) = python_decorator_name(child, source) {
            names.push(name);
        }
    }
    names
}

fn python_decorator_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "attribute" => return Some(node_text(child, source)),
            "call" => {
                if let Some(callee) = python_first_child(child)
                    && matches!(callee.kind(), "identifier" | "attribute")
                {
                    return Some(node_text(callee, source));
                }
            }
            _ => {}
        }
    }
    None
}

fn python_type_alias_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "type_alias_statement" {
        return None;
    }
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("left"))
        .map(|child| node_text(child, source))
        .filter(|name| !name.is_empty())
        .or_else(|| python_identifier_child(node, source))
}

fn python_import_targets(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Vec<String> {
    if node.kind() == "import_statement" {
        let mut imports = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "dotted_name" => {
                    let target = node_text(child, source);
                    imports.push(
                        python_resolve_module_to_file(&target, file_path, repo_root)
                            .unwrap_or(target),
                    );
                }
                "aliased_import" => {
                    if let Some(target) = python_child_text(child, source, "dotted_name") {
                        imports.push(
                            python_resolve_module_to_file(&target, file_path, repo_root)
                                .unwrap_or(target),
                        );
                    }
                }
                _ => {}
            }
        }
        return imports;
    }

    let mut module = None;
    let mut seen_import = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "relative_import" => {
                let text = node_text(child, source);
                module = Some(text);
                break;
            }
            "dotted_name" if !seen_import => {
                module = Some(node_text(child, source));
            }
            "import" => {
                seen_import = true;
            }
            _ => {}
        }
    }
    module
        .into_iter()
        .map(|target| {
            python_resolve_module_to_file(&target, file_path, repo_root).unwrap_or(target)
        })
        .collect()
}

fn python_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).next()?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "attribute" => rust_rightmost_identifier(first, source),
        _ => None,
    }
}

fn python_bound_member_target(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).next()?;
    if first.kind() != "attribute" {
        return None;
    }
    let method = rust_rightmost_identifier(first, context.source)?;
    let receiver = python_first_child(first)?;
    if receiver.kind() != "identifier" {
        return None;
    }
    context
        .bindings
        .borrow()
        .resolve_member(&node_text(receiver, context.source), &method)
}

fn python_bind_assignment(node: tree_sitter::Node<'_>, context: &PythonParseContext<'_>) {
    if !matches!(node.kind(), "assignment" | "augmented_assignment") {
        return;
    }
    let Some(lhs) = python_first_child(node) else {
        return;
    };
    if lhs.kind() != "identifier" {
        return;
    }
    let var = node_text(lhs, context.source);
    if let Some(rhs) = python_last_value_child(node)
        && rhs.kind() == "call"
        && let Some(call_name) = python_call_name(rhs, context.source)
    {
        let type_name = context
            .bindings
            .borrow()
            .constructor_type(&call_name)
            .map(str::to_string);
        if let Some(type_name) = type_name {
            context.bindings.borrow_mut().bind(var.clone(), type_name);
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !matches!(child.kind(), "type" | "type_annotation") {
            continue;
        }
        if let Some(type_name) = python_identifier_child(child, context.source)
            .or_else(|| python_base_name(child, context.source))
        {
            context.bindings.borrow_mut().bind(var, type_name);
            return;
        }
    }
}

fn collect_python_class_names(node: tree_sitter::Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_python_class_names_into(node, source, &mut names);
    names
}

fn collect_python_class_names_into(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    let target = if node.kind() == "decorated_definition" {
        python_decorated_target(node)
    } else {
        Some(node)
    };
    if let Some(target) = target
        && target.kind() == "class_definition"
        && let Some(name) = python_identifier_child(target, source)
    {
        names.insert(name);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_python_class_names_into(child, source, names);
    }
}

fn rust_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = rust_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn resolve_python_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    resolve_rust_call_targets(nodes, edges, file_path)
}

fn python_resolve_imported_call_target(
    call_name: &str,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    if context.top_level_defined_names.contains(call_name) {
        return None;
    }
    let module = context.import_map.get(call_name)?;
    let resolved = python_resolve_module_to_file(module, &context.file_path, context.repo_root)?;
    Some(qualify(&resolved, call_name, None))
}

fn add_python_tested_by_edges(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    if !is_test_file(file_path) {
        return edges;
    }
    let test_qnames = nodes
        .iter()
        .filter(|node| node.is_test)
        .map(|node| qualify(file_path, &node.name, node.parent_name.as_deref()))
        .collect::<HashSet<_>>();
    if test_qnames.is_empty() {
        return edges;
    }
    let mut out = edges;
    let tested_by_edges = out
        .iter()
        .filter(|edge| edge.kind == "CALLS" && test_qnames.contains(&edge.source))
        .map(|edge| ParsedEdge {
            kind: crate::core::types::EdgeKind::TestedBy,
            source: edge.target.clone(),
            target: edge.source.clone(),
            file_path: edge.file_path.clone(),
            line: edge.line,
            extra: json!({}),
        })
        .collect::<Vec<_>>();
    out.extend(tested_by_edges);
    out
}

fn python_resolve_module_to_file(
    module: &str,
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Option<String> {
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let candidates_for = |base: PathBuf, rel: &str| {
        [
            base.join(format!("{rel}.py")),
            base.join(rel).join("__init__.py"),
        ]
    };

    if module.starts_with('.') {
        let leading_dots = module.bytes().take_while(|byte| *byte == b'.').count();
        let remainder = &module[leading_dots..];
        let mut base = caller_dir.to_path_buf();
        for _ in 0..leading_dots.saturating_sub(1) {
            base = base.parent().unwrap_or(Path::new("")).to_path_buf();
        }
        let candidates = if remainder.is_empty() {
            vec![base.join("__init__.py")]
        } else {
            let rel = remainder.replace('.', "/");
            candidates_for(base, &rel).into_iter().collect()
        };
        return candidates
            .into_iter()
            .find(|candidate| python_module_candidate_is_file(candidate, repo_root))
            .and_then(|candidate| python_module_candidate_path(candidate, repo_root));
    }

    let rel = module.replace('.', "/");
    let mut current = caller_dir.to_path_buf();
    loop {
        for candidate in candidates_for(current.clone(), &rel) {
            if python_module_candidate_is_file(&candidate, repo_root) {
                return python_module_candidate_path(candidate, repo_root);
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    None
}

fn python_module_candidate_is_file(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_file())
        .unwrap_or_else(|| candidate.is_file())
}

fn python_module_candidate_path(candidate: PathBuf, repo_root: Option<&Path>) -> Option<String> {
    if repo_root.is_some() {
        return Some(candidate.to_string_lossy().to_string());
    }
    candidate
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

fn python_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    caller: &str,
) -> Option<ParsedEdge> {
    let signature = python_call_signature(node, source)?;
    let (relationship_role, bridge_kind) = python_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match python_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "python",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn python_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() != "argument_list")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn python_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "subprocess.run"
        | "subprocess.Popen"
        | "subprocess.call"
        | "subprocess.check_call"
        | "subprocess.check_output"
        | "os.system"
        | "os.popen"
        | "os.execv"
        | "os.execvp"
        | "os.execvpe"
        | "os.execve"
        | "os.execl"
        | "os.execlp"
        | "os.execlpe"
        | "os.execle"
        | "os.spawnv"
        | "os.spawnvp" => Some(("invokes_binary", "subprocess")),
        "ctypes.CDLL"
        | "ctypes.cdll.LoadLibrary"
        | "ctypes.WinDLL"
        | "ctypes.PyDLL"
        | "cffi.FFI().dlopen" => Some(("loads_shared_library", "ffi")),
        "open" | "io.open" => Some(("opens_file", "file_io")),
        _ => None,
    }
}

fn python_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if child.kind() == "string" {
            return Some(decode_python_string_literal(child, source));
        }
        if matches!(child.kind(), "list" | "tuple") {
            return python_first_string_in_sequence(child, source);
        }
        return None;
    }
    None
}

fn python_first_string_in_sequence(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if child.kind() == "string" {
            return Some(decode_python_string_literal(child, source));
        }
    }
    None
}

fn decode_python_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_content" | "string_fragment") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .to_string()
}
