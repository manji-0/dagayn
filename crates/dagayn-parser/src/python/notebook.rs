//! Notebook formats: Jupyter `.ipynb`, marimo (`.py` apps and `.md`), and
//! Databricks `.py` / `.r` exports, parsed cell by cell with the Python
//! extractor, plus SQL table imports and cell dataflow edges.

use super::*;

mod marimo;

use marimo::*;
pub(crate) use marimo::{
    looks_like_marimo_md, looks_like_marimo_py, parse_marimo_md_with_parser,
    parse_marimo_py_with_parser,
};

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

pub(crate) fn parse_notebook_with_parser(
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

pub(super) fn is_databricks_py_source(source: &[u8]) -> bool {
    let first_line = source
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    first_line.trim_ascii() == b"# Databricks notebook source"
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn expression_statement_call(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if node.kind() != "expression_statement" {
        return None;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == "call")
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

pub(super) fn parse_databricks_py_with_parser(
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
