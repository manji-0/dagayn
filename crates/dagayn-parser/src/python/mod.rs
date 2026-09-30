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

mod notebook;

use notebook::*;
pub(super) use notebook::{
    looks_like_marimo_md, parse_marimo_md_with_parser, parse_notebook_with_parser,
};

mod bridges;

use bridges::*;

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
        let mut import_aliases = HashMap::new();
        collect_python_import_aliases(root, source, &mut import_aliases);
        let context = PythonParseContext {
            source,
            file_path: file_path.clone(),
            repo_root,
            import_map: &import_map,
            top_level_defined_names: &top_level_defined_names,
            protocol_names: &protocol_names,
            import_aliases: &import_aliases,
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

struct PythonParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    repo_root: Option<&'a Path>,
    import_map: &'a HashMap<String, String>,
    top_level_defined_names: &'a HashSet<String>,
    protocol_names: &'a HashSet<String>,
    /// Local names bound by an import anywhere in the file, including the
    /// function-level imports `import_map` leaves out. A call on one of them
    /// (`_core.parse(...)`) records the receiver so native-binding
    /// resolution can tell which module the attribute came from.
    import_aliases: &'a HashMap<String, String>,
    bindings: RefCell<MemberCallBindings>,
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
                    let scope = python_scope_path(&context.file_path, enclosing_qualified);
                    let qualified = qualify(&context.file_path, &name, scope);
                    let class_path = scope
                        .map(|scope| format!("{scope}.{name}"))
                        .unwrap_or_else(|| name.clone());
                    let bases = python_class_base_names(child, context.source);
                    let decorators = python_parent_decorators(child, context.source);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Class,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: scope.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: python_class_extra(&bases, &decorators),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: enclosing_qualified
                            .unwrap_or(&context.file_path)
                            .to_string(),
                        target: qualified.clone(),
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    python_emit_bases(child, context, &qualified, &bases, edges);
                    python_walk_children(
                        child,
                        context,
                        Some(&class_path),
                        Some(&qualified),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = python_identifier_child(child, context.source) {
                    let scope = python_scope_path(&context.file_path, enclosing_qualified);
                    let qualified = qualify(&context.file_path, &name, scope);
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
                        parent_name: scope.map(str::to_string),
                        params,
                        return_type,
                        modifiers: None,
                        is_test,
                        extra,
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: enclosing_qualified
                            .unwrap_or(&context.file_path)
                            .to_string(),
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
                    let scope = python_scope_path(&context.file_path, enclosing_qualified);
                    let qualified = qualify(&context.file_path, &name, scope);
                    nodes.push(ParsedNode {
                        kind: crate::core::types::NodeKind::Type,
                        name: name.clone(),
                        file_path: context.file_path.clone(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: scope.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({"type_role": "alias"}),
                    });
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Contains,
                        source: enclosing_qualified
                            .unwrap_or(&context.file_path)
                            .to_string(),
                        target: qualified,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    continue;
                }
            }
            "import_statement" | "import_from_statement" => {
                for (target, extra) in python_import_targets(
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
                        extra,
                    });
                }
            }
            "call" => {
                if let Some(call_name) = python_call_name(child, context.source) {
                    let caller = enclosing_qualified.unwrap_or(&context.file_path);
                    let target = python_bound_member_target(child, context)
                        .or_else(|| python_resolve_imported_call_target(&call_name, context))
                        .unwrap_or(call_name);
                    let extra = match python_import_receiver(child, context) {
                        Some(receiver) => json!({"receiver": receiver}),
                        None => json!({}),
                    };
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::Calls,
                        source: caller.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra,
                    });
                    if let Some(edge) = python_bridge_edge(
                        child,
                        context.source,
                        &context.file_path,
                        caller,
                        context.import_aliases,
                    ) {
                        edges.push(edge);
                    }
                }
            }
            "assignment"
                if python_emit_lambda_assignment(
                    child,
                    context,
                    enclosing_class,
                    enclosing_qualified,
                    nodes,
                    edges,
                ) =>
            {
                python_bind_assignment(child, context);
                continue;
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

/// Parent path (relative to the file) of the innermost enclosing class or
/// function, so nested definitions qualify under it.
fn python_scope_path<'a>(file_path: &str, enclosing_qualified: Option<&'a str>) -> Option<&'a str> {
    enclosing_qualified?
        .strip_prefix(file_path)?
        .strip_prefix("::")
        .filter(|scope| !scope.is_empty())
}

/// Emits `name = lambda ...` as a function so calls in the lambda body have a
/// caller. Returns false when the assignment is not a plain lambda binding.
fn python_emit_lambda_assignment(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_qualified: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return false;
    };
    if left.kind() != "identifier" || right.kind() != "lambda" {
        return false;
    }
    let name = node_text(left, context.source);
    let scope = python_scope_path(&context.file_path, enclosing_qualified);
    let qualified = qualify(&context.file_path, &name, scope);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name,
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "python".to_string(),
        parent_name: scope.map(str::to_string),
        params: python_child_text(right, context.source, "parameters"),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"python_kind": "lambda"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_qualified
            .unwrap_or(&context.file_path)
            .to_string(),
        target: qualified.clone(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    python_walk_children(
        right,
        context,
        enclosing_class,
        Some(&qualified),
        nodes,
        edges,
    );
    true
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

/// IMPORTS_FROM targets of one import statement, each with the raw module
/// and the names it binds in `extra`: the target may already be resolved to
/// a file, and native-binding resolution needs the module as written
/// (`from pkg import _core` names `pkg._core`, a module with no `.py`).
fn python_import_targets(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &FilePath,
    repo_root: Option<&Path>,
) -> Vec<(String, serde_json::Value)> {
    if node.kind() == "import_statement" {
        let mut imports = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let (module, alias) = match child.kind() {
                "dotted_name" => (node_text(child, source), None),
                "aliased_import" => {
                    let Some(module) = python_child_text(child, source, "dotted_name") else {
                        continue;
                    };
                    let alias = child
                        .child_by_field_name("alias")
                        .map(|alias| node_text(alias, source));
                    (module, alias)
                }
                _ => continue,
            };
            let mut extra = json!({"module": module});
            if let Some(alias) = alias {
                extra["alias"] = json!(alias);
            }
            imports.push((
                python_resolve_module_to_file(&module, file_path, repo_root).unwrap_or(module),
                extra,
            ));
        }
        return imports;
    }

    let Some(module_node) = node.child_by_field_name("module_name") else {
        return Vec::new();
    };
    let module = node_text(module_node, source);
    // `from pkg import sub` imports the submodule `pkg/sub.py`, the same way
    // `import pkg.sub` does; only names that are not submodules (or `*`)
    // make it an import of `pkg` itself.
    let mut submodules = Vec::new();
    let mut imports_package = false;
    let mut bound_names = Vec::new();
    let mut cursor = node.walk();
    let names = node
        .children_by_field_name("name", &mut cursor)
        .collect::<Vec<_>>();
    if names.is_empty() {
        imports_package = true;
    }
    for name_node in names {
        let (dotted, alias) = if name_node.kind() == "aliased_import" {
            (
                name_node.child_by_field_name("name"),
                name_node
                    .child_by_field_name("alias")
                    .map(|alias| node_text(alias, source)),
            )
        } else {
            (Some(name_node), None)
        };
        let Some(dotted) = dotted else {
            imports_package = true;
            continue;
        };
        let name = node_text(dotted, source);
        bound_names.push(json!([name, alias.unwrap_or_else(|| name.clone())]));
        let submodule = if module.ends_with('.') {
            format!("{module}{name}")
        } else {
            format!("{module}.{name}")
        };
        match python_resolve_module_to_file(&submodule, file_path, repo_root) {
            Some(path) if !submodules.contains(&path) => submodules.push(path),
            Some(_) => {}
            None => imports_package = true,
        }
    }
    let extra = json!({"module": module, "names": bound_names});
    let mut imports = Vec::new();
    if imports_package {
        imports.push((
            python_resolve_module_to_file(&module, file_path, repo_root).unwrap_or(module),
            extra.clone(),
        ));
    }
    imports.extend(submodules.into_iter().map(|path| (path, extra.clone())));
    imports
}

/// Every local name an import statement binds, at any depth of the file,
/// mapped to what it names: `import a.b` binds `a` -> `a`, `import a as b`
/// binds `b` -> `a`, `from m import n as k` binds `k` -> `m.n`.
fn collect_python_import_aliases(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    aliases: &mut HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_statement" => {
                let mut inner = child.walk();
                for name in child.children(&mut inner) {
                    match name.kind() {
                        "dotted_name" => {
                            let text = node_text(name, source);
                            if let Some(head) = text.split('.').next() {
                                aliases.insert(head.to_string(), head.to_string());
                            }
                        }
                        "aliased_import" => {
                            if let (Some(module), Some(alias)) = (
                                name.child_by_field_name("name"),
                                name.child_by_field_name("alias"),
                            ) {
                                aliases.insert(node_text(alias, source), node_text(module, source));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "import_from_statement" => {
                let module = child
                    .child_by_field_name("module_name")
                    .map(|module| node_text(module, source))
                    .unwrap_or_default();
                let mut inner = child.walk();
                for name in child.children_by_field_name("name", &mut inner) {
                    let (imported, bound) = if name.kind() == "aliased_import" {
                        (
                            name.child_by_field_name("name"),
                            name.child_by_field_name("alias"),
                        )
                    } else {
                        (Some(name), Some(name))
                    };
                    if let (Some(imported), Some(bound)) = (imported, bound) {
                        let imported = node_text(imported, source);
                        let origin = if module.is_empty() || module.ends_with('.') {
                            format!("{module}{imported}")
                        } else {
                            format!("{module}.{imported}")
                        };
                        aliases.insert(node_text(bound, source), origin);
                    }
                }
            }
            _ => collect_python_import_aliases(child, source, aliases),
        }
    }
}

/// The receiver of `alias.attr(...)` when `alias` was bound by an import.
fn python_import_receiver(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).next()?;
    if first.kind() != "attribute" {
        return None;
    }
    let receiver = first.child_by_field_name("object")?;
    if receiver.kind() != "identifier" {
        return None;
    }
    let receiver = node_text(receiver, context.source);
    context
        .import_aliases
        .contains_key(&receiver)
        .then_some(receiver)
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
