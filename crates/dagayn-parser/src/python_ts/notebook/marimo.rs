//! marimo notebooks: `.py` apps (`@app.cell`, `with app.setup`, unparsable cells) and `.md` notebooks (fenced cells), with cell refs / defs dataflow edges and SQL table imports.

use super::*;

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

pub(crate) fn looks_like_marimo_py(source: &[u8]) -> bool {
    contains_bytes(source, b"marimo")
        && (contains_bytes(source, b"@app.cell")
            || contains_bytes(source, b"@app.function")
            || contains_bytes(source, b"@app.class_definition")
            || contains_bytes(source, b"app.setup"))
}

pub(crate) fn looks_like_marimo_md(source: &[u8]) -> bool {
    let text = String::from_utf8_lossy(source);
    has_marimo_version_frontmatter(&text) || !collect_marimo_md_cells(&text).is_empty()
}

pub(crate) fn parse_marimo_md_with_parser(
    file_path: &str,
    source: &[u8],
    markdown_parser: Option<&mut tree_sitter::Parser>,
    python_parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let (mut nodes, mut edges) = super::super::super::markdown::parse_markdown_with_parser(
        file_path,
        source,
        markdown_parser,
    );
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

pub(crate) fn is_default_marimo_cell_name(name: &str) -> bool {
    name.is_empty() || name == "_" || name == "__"
}

pub(crate) fn parse_marimo_py_with_parser(
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

pub(crate) fn add_marimo_cell_dataflow_edges(
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
