use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::stdlib::elixir::{elixir_kernel_module, is_elixir_stdlib_module, is_erlang_otp_module};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child, direct_child_text, last_direct_child_text, line_count, line_of, node_text,
    set_namespaces_from_type_names,
};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_elixir_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "elixir")];
    let mut edges = Vec::new();
    let context = ElixirParseContext {
        source,
        file_path: file_path.clone(),
        imports: RefCell::default(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        elixir_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        set_namespaces_from_type_names(&mut nodes);
        elixir_mark_stdlib_calls(&nodes, &mut edges, &context.imports.borrow());
        let mut edges = resolve_elixir_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct ElixirParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    imports: RefCell<ElixirImports>,
}

/// What the file's `alias` / `import` directives bring into scope.
#[derive(Default)]
struct ElixirImports {
    /// Functions imported by name, with their module (`map` → `Enum` after
    /// `import Enum, only: [map: 2]`).
    names: HashMap<String, String>,
    /// Names an `alias` binds to a module of this repository (`String`
    /// after `alias MyApp.String`), which no longer name the stdlib's.
    shadowing_aliases: HashSet<String>,
}

fn elixir_walk_children(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call"
            && elixir_handle_call(
                child,
                context,
                enclosing_module,
                enclosing_func,
                nodes,
                edges,
            )
        {
            continue;
        }
        elixir_walk_children(
            child,
            context,
            enclosing_module,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn elixir_handle_call(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(ident) = elixir_call_identifier(node, context.source) else {
        return false;
    };
    match ident.as_str() {
        "defmodule" => {
            let Some(arguments) = direct_child(node, &["arguments"]) else {
                return false;
            };
            let Some(module_name) = elixir_module_name(arguments, context.source) else {
                return false;
            };
            elixir_emit_module(node, context, &module_name, enclosing_module, nodes, edges);
            let scope = match enclosing_module {
                Some(parent) => format!("{parent}.{module_name}"),
                None => module_name,
            };
            if let Some(do_block) = direct_child(node, &["do_block"]) {
                elixir_walk_children(do_block, context, Some(&scope), None, nodes, edges);
            }
            true
        }
        "def" | "defp" | "defmacro" | "defmacrop" => {
            let Some(arguments) = direct_child(node, &["arguments"]) else {
                return false;
            };
            let Some((function_name, params)) =
                elixir_function_name_and_params(arguments, context.source)
            else {
                return false;
            };
            elixir_emit_function(
                node,
                context,
                &function_name,
                params.as_deref(),
                enclosing_module,
                nodes,
                edges,
            );
            if let Some(do_block) = direct_child(node, &["do_block"]) {
                elixir_walk_children(
                    do_block,
                    context,
                    enclosing_module,
                    Some(&function_name),
                    nodes,
                    edges,
                );
            }
            // `def f(x), do: body` keeps its body in a keyword pair.
            if let Some(keywords) = direct_child(arguments, &["keywords"]) {
                elixir_walk_children(
                    keywords,
                    context,
                    enclosing_module,
                    Some(&function_name),
                    nodes,
                    edges,
                );
            }
            true
        }
        "alias" | "import" | "require" | "use" => {
            if let Some(arguments) = direct_child(node, &["arguments"]) {
                for module_name in elixir_import_targets(arguments, context.source) {
                    let mut edge = ParsedEdge::new(
                        crate::core::types::EdgeKind::ImportsFrom,
                        context.file_path.to_string(),
                        module_name,
                        context.file_path.clone(),
                        line_of(node),
                    );
                    elixir_record_import(&ident, arguments, context, &mut edge);
                    edges.push(edge);
                }
            }
            true
        }
        _ => {
            elixir_emit_call(node, context, enclosing_module, enclosing_func, edges);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if matches!(child.kind(), "arguments" | "do_block") {
                    elixir_walk_children(
                        child,
                        context,
                        enclosing_module,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            true
        }
    }
}

/// Records what an `alias` / `import` brings into scope, and marks a
/// directive naming an Elixir module (`require Logger`, `import Enum`)
/// certain. An alias of a repository module shadows the stdlib module of
/// its last name (`alias MyApp.String` makes `String.x` the repository's).
fn elixir_record_import(
    directive: &str,
    arguments: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    edge: &mut ParsedEdge,
) {
    let mut imports = context.imports.borrow_mut();
    let alias_as = elixir_keyword_value(arguments, context.source, "as")
        .map(|value| node_text(value, context.source).replace(' ', ""));
    if !is_elixir_stdlib_module(&edge.target) {
        if directive == "alias" {
            let short = alias_as.unwrap_or_else(|| {
                edge.target
                    .rsplit('.')
                    .next()
                    .unwrap_or(&edge.target)
                    .to_string()
            });
            imports.shadowing_aliases.insert(short);
        }
        return;
    }
    if directive == "import"
        && let Some(only) = elixir_keyword_value(arguments, context.source, "only")
        && only.kind() == "list"
    {
        for name in elixir_keyword_keys(only, context.source) {
            imports.names.insert(name, edge.target.clone());
        }
    }
    let module = edge.target.clone();
    mark_stdlib_edge(
        &mut edge.target,
        &mut edge.extra,
        &module,
        StdlibEvidence::Certain,
    );
}

/// The value of `key:` in a directive's trailing keyword list
/// (`only: [map: 2]`, `as: Str`).
fn elixir_keyword_value<'a>(
    arguments: tree_sitter::Node<'a>,
    source: &[u8],
    key: &str,
) -> Option<tree_sitter::Node<'a>> {
    let keywords = direct_child(arguments, &["keywords"])?;
    let mut cursor = keywords.walk();
    keywords
        .named_children(&mut cursor)
        .filter(|pair| pair.kind() == "pair")
        .find(|pair| {
            pair.child_by_field_name("key")
                .is_some_and(|name| node_text(name, source).trim().trim_end_matches(':') == key)
        })
        .and_then(|pair| pair.child_by_field_name("value"))
}

/// The keys of a keyword list (`[map: 2, filter: 2]` → `map`, `filter`).
fn elixir_keyword_keys(list: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(keywords) = direct_child(list, &["keywords"]) else {
        return Vec::new();
    };
    let mut cursor = keywords.walk();
    keywords
        .named_children(&mut cursor)
        .filter_map(|pair| pair.child_by_field_name("key"))
        .map(|key| {
            node_text(key, source)
                .trim()
                .trim_end_matches(':')
                .to_string()
        })
        .collect()
}

/// Points the calls into the standard library at their module: a remote
/// call on an Elixir module (`Enum.map` at `Enum`, `IO.ANSI.red` at
/// `IO.ANSI`) or an Erlang/OTP one (`:lists.reverse` at `:lists`), and a
/// function imported with `import Enum, only: [...]`, certainly; a bare
/// `Kernel` function or macro (`is_nil`, `raise`, `if`), likely. A module
/// this file defines or aliases to its own, and a function it defines,
/// are the repository's.
fn elixir_mark_stdlib_calls(
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
    imports: &ElixirImports,
) {
    let defined_functions = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    let defined_modules = nodes
        .iter()
        .filter(|node| node.kind.as_str() == "Class")
        .map(|node| node.name.rsplit('.').next().unwrap_or(&node.name))
        .collect::<HashSet<_>>();
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::Calls {
            continue;
        }
        let (module, evidence) = match edge.target.rsplit_once('.') {
            Some((module, _)) => {
                if let Some(erlang) = module.strip_prefix(':') {
                    if !is_erlang_otp_module(erlang) {
                        continue;
                    }
                } else {
                    let root = module.split('.').next().unwrap_or(module);
                    if !is_elixir_stdlib_module(module)
                        || defined_modules.contains(root)
                        || imports.shadowing_aliases.contains(root)
                    {
                        continue;
                    }
                }
                (module.to_string(), StdlibEvidence::Certain)
            }
            None if defined_functions.contains(edge.target.as_str()) => continue,
            None => {
                if let Some(module) = imports.names.get(&edge.target) {
                    edge.target = format!("{module}.{}", edge.target);
                    (module.clone(), StdlibEvidence::Certain)
                } else if let Some(module) = elixir_kernel_module(&edge.target) {
                    (module.to_string(), StdlibEvidence::Likely)
                } else {
                    continue;
                }
            }
        };
        mark_stdlib_edge(&mut edge.target, &mut edge.extra, &module, evidence);
    }
}

fn elixir_emit_module(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    name: &str,
    enclosing_module: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, enclosing_module);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "elixir".to_string(),
        parent_name: enclosing_module.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_module
            .map(|module| qualify(&context.file_path, module, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn elixir_emit_function(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    name: &str,
    params: Option<&str>,
    enclosing_module: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, enclosing_module);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "elixir".to_string(),
        parent_name: enclosing_module.map(str::to_string),
        params: params.map(str::to_string),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        enclosing_module
            .map(|module| qualify(&context.file_path, module, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn elixir_emit_call(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = elixir_call_target(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_module))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Calls,
        caller,
        target,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn elixir_call_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = elixir_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "dot" => last_direct_child_text(first, source, &["identifier"]),
        _ => None,
    }
}

fn elixir_call_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = elixir_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        // `fun.(x)` calls an anonymous function bound to a variable: a dot
        // with no right-hand name, nothing a declaration could match.
        "dot" if first.child_by_field_name("right").is_none() => None,
        "dot" => Some(node_text(first, source).replace(' ', "")),
        _ => None,
    }
}

fn elixir_module_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "alias" | "dot") {
            return Some(node_text(child, source).replace(' ', ""));
        }
    }
    None
}

/// `alias MyApp.{Repo, User}` names two modules.
fn elixir_import_targets(arguments: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = arguments.walk();
    let Some(target) = arguments
        .children(&mut cursor)
        .find(|child| matches!(child.kind(), "alias" | "dot"))
    else {
        return Vec::new();
    };
    if target.kind() == "dot"
        && let Some(tuple) = direct_child(target, &["tuple"])
    {
        let prefix = elixir_first_named_child(target)
            .map(|base| node_text(base, source).replace(' ', ""))
            .unwrap_or_default();
        let mut cursor = tuple.walk();
        return tuple
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "alias")
            .map(|child| format!("{prefix}.{}", node_text(child, source)))
            .collect();
    }
    vec![node_text(target, source).replace(' ', "")]
}

fn elixir_function_name_and_params(
    arguments: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "call" {
            let name = direct_child_text(child, source, &["identifier"])?;
            let mut params_text = node_text(child, source);
            if params_text.starts_with(&name) {
                params_text = params_text[name.len()..].to_string();
            }
            return Some((name, (!params_text.is_empty()).then_some(params_text)));
        }
        if child.kind() == "identifier" {
            return Some((node_text(child, source), None));
        }
        // `def f(x) when is_integer(x)`: the head is the guard's left operand.
        if child.kind() == "binary_operator"
            && let Some(head) = elixir_first_named_child(child)
            && matches!(head.kind(), "call" | "identifier")
        {
            let mut cursor = child.walk();
            let is_guard = child
                .children(&mut cursor)
                .any(|op| node_text(op, source) == "when");
            if is_guard {
                return elixir_function_name_and_params(child, source);
            }
        }
    }
    None
}

fn elixir_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor).find(|child| child.is_named())
}

fn resolve_elixir_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    let mut module_functions = HashMap::<(String, String), String>::new();
    let mut dotted_functions = HashMap::<String, String>::new();
    let mut bare_functions = HashMap::<String, String>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        bare_functions
            .entry(node.name.clone())
            .or_insert_with(|| qualified.clone());
        if let Some(module) = &node.parent_name {
            module_functions.insert((module.clone(), node.name.clone()), qualified.clone());
            dotted_functions.insert(format!("{module}.{}", node.name), qualified);
        }
    }

    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") && edge.extra["external"] != true
            {
                let nested = elixir_source_module(&edge.source, file_path)
                    .map(|module| format!("{module}.{}", edge.target))
                    .and_then(|dotted| dotted_functions.get(&dotted));
                if let Some(target) = dotted_functions.get(&edge.target).or(nested) {
                    edge.target = target.clone();
                } else if edge.target.contains('.') {
                    edge.target = edge
                        .target
                        .rsplit('.')
                        .next()
                        .unwrap_or(edge.target.as_str())
                        .to_string();
                } else if let Some(module) = elixir_source_module(&edge.source, file_path) {
                    if let Some(target) =
                        module_functions.get(&(module.to_string(), edge.target.clone()))
                    {
                        edge.target = target.clone();
                    } else if let Some(target) = bare_functions.get(&edge.target) {
                        edge.target = target.clone();
                    }
                } else if let Some(target) = bare_functions.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

fn elixir_source_module<'a>(source: &'a str, file_path: &str) -> Option<&'a str> {
    let suffix = source.strip_prefix(file_path)?.strip_prefix("::")?;
    suffix.rsplit_once('.').map(|(module, _)| module)
}
