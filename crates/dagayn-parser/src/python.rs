use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::documentation_directives::{
    extract_line_comment_dagayn_directives, nearest_documentation_source,
    push_documentation_directive_edge,
};
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
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let (import_map, top_level_defined_names) = collect_python_file_scope(root, source);
            let context = PythonParseContext {
                source,
                file_path: file_path.clone(),
                repo_root,
                import_map: &import_map,
                top_level_defined_names: &top_level_defined_names,
            };
            python_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            extract_python_documentation_directives(&file_path, source, &nodes, &mut edges);
            let edges = resolve_python_call_targets(&nodes, edges, &file_path);
            let edges = add_python_tested_by_edges(&nodes, edges, &file_path);
            return (nodes, edges);
        }
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
}

#[derive(Clone)]
struct NotebookCell {
    cell_index: i64,
    language: &'static str,
    source: String,
}

fn is_databricks_py_source(source: &[u8]) -> bool {
    let first_line = source
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    first_line.trim_ascii() == b"# Databricks notebook source"
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
    let edges = resolve_python_call_targets(&nodes, edges, file_path);
    let edges = add_python_tested_by_edges(&nodes, edges, file_path);
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
        out.push(NotebookCell {
            cell_index: cell_index as i64,
            language: cell_language,
            source: filtered.join(""),
        });
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
            cells.push(NotebookCell {
                cell_index: cell_index as i64,
                language,
                source: stripped.join("\n"),
            });
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
        cells.push(NotebookCell {
            cell_index: cell_index as i64,
            language: "python",
            source,
        });
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
    let (nodes, edges) = parse_python_with_parser(file_path, source.as_bytes(), parser, repo_root);
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
                        extra: json!({"type_role": "class"}),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: context.file_path.to_string(),
                        target: qualified.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    python_emit_bases(child, context.source, &context.file_path, &qualified, edges);
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
                        extra: json!({}),
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
                    python_walk_children(
                        child,
                        context,
                        enclosing_class,
                        Some(&qualified),
                        nodes,
                        edges,
                    );
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
                    let target = python_resolve_imported_call_target(&call_name, context)
                        .unwrap_or_else(|| call_name.clone());
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
            if let Some(value_node) = python_last_value_child(node) {
                if value_node.kind() == "identifier" {
                    python_emit_reference_if_known(value_node, context, caller, edges);
                }
            }
        }
        "assignment" => {
            let Some(lhs) = python_first_child(node) else {
                return;
            };
            if !matches!(lhs.kind(), "attribute" | "subscript") {
                return;
            }
            if let Some(rhs) = python_last_value_child(node) {
                if rhs.kind() == "identifier" {
                    python_emit_reference_if_known(rhs, context, caller, edges);
                }
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
    let first = node.children(&mut cursor).next();
    first
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
) -> (HashMap<String, String>, HashSet<String>) {
    let mut import_map = HashMap::new();
    let mut defined_names = HashSet::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let target = if child.kind() == "decorated_definition" {
            python_decorated_target(child)
        } else {
            Some(child)
        };
        if let Some(target) = target {
            match target.kind() {
                "class_definition" | "function_definition" => {
                    if let Some(name) = python_identifier_child(target, source) {
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
    (import_map, defined_names)
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
                if let Some(module) = &module {
                    if let Some(name) = python_aliased_import_name(child, source) {
                        import_map.insert(name, module.clone());
                    }
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
    let has_decorator = parent.children(&mut cursor).any(|child| {
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
    });
    has_decorator
}

fn python_emit_bases(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    qualified: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "argument_list" {
            continue;
        }
        let mut arg_cursor = child.walk();
        for arg in child.children(&mut arg_cursor) {
            if matches!(arg.kind(), "identifier" | "attribute") {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::Inherits,
                    source: qualified.to_string(),
                    target: node_text(arg, source),
                    file_path: file_path.clone(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({
                        "relationship_role": "extends",
                        "syntax_source": node.kind(),
                    }),
                });
            }
        }
    }
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
            if child.kind() == "dotted_name" {
                let target = node_text(child, source);
                imports.push(
                    python_resolve_module_to_file(&target, file_path, repo_root).unwrap_or(target),
                );
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
    let signature = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty());
    signature
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
