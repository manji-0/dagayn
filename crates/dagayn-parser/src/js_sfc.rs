use std::path::Path;

use serde_json::json;

use super::js_like::parse_javascript_like_interned;
use super::js_modules::JavaScriptCaches;
use super::parsers::{
    new_javascript_parser, new_svelte_parser, new_typescript_parser, new_vue_parser,
};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text};

pub fn parse_vue(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut vue_parser = new_vue_parser();
    let mut javascript_parser = new_javascript_parser();
    let mut typescript_parser = new_typescript_parser();
    parse_vue_with_parsers(
        file_path,
        source,
        vue_parser.as_mut(),
        javascript_parser.as_mut(),
        typescript_parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

pub fn parse_svelte(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut svelte_parser = new_svelte_parser();
    let mut javascript_parser = new_javascript_parser();
    let mut typescript_parser = new_typescript_parser();
    parse_svelte_with_parsers(
        file_path,
        source,
        svelte_parser.as_mut(),
        javascript_parser.as_mut(),
        typescript_parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

pub(super) fn parse_vue_with_parsers(
    file_path: &str,
    source: &[u8],
    vue_parser: Option<&mut tree_sitter::Parser>,
    javascript_parser: Option<&mut tree_sitter::Parser>,
    typescript_parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_sfc_with_parsers(
        file_path,
        source,
        SfcParserInputs {
            language: "vue",
            sfc_parser: vue_parser,
            javascript_parser,
            typescript_parser,
            repo_root,
            caches,
        },
    )
}

pub(super) fn parse_svelte_with_parsers(
    file_path: &str,
    source: &[u8],
    svelte_parser: Option<&mut tree_sitter::Parser>,
    javascript_parser: Option<&mut tree_sitter::Parser>,
    typescript_parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_sfc_with_parsers(
        file_path,
        source,
        SfcParserInputs {
            language: "svelte",
            sfc_parser: svelte_parser,
            javascript_parser,
            typescript_parser,
            repo_root,
            caches,
        },
    )
}

struct SfcParserInputs<'a> {
    language: &'static str,
    sfc_parser: Option<&'a mut tree_sitter::Parser>,
    javascript_parser: Option<&'a mut tree_sitter::Parser>,
    typescript_parser: Option<&'a mut tree_sitter::Parser>,
    repo_root: Option<&'a Path>,
    caches: JavaScriptCaches<'a>,
}

fn parse_sfc_with_parsers(
    file_path: &str,
    source: &[u8],
    mut inputs: SfcParserInputs<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: inputs.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = inputs.sfc_parser
        && let Some(tree) = parser.parse(source, None)
    {
        let root = tree.root_node();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() != "script_element" {
                continue;
            }
            let Some(raw_text_node) = sfc_direct_child(child, "raw_text") else {
                continue;
            };
            let script_language = sfc_script_language(child, source);
            let script_parser = if script_language == "typescript" {
                inputs.typescript_parser.as_deref_mut()
            } else {
                inputs.javascript_parser.as_deref_mut()
            };
            let script_source = &source[raw_text_node.start_byte()..raw_text_node.end_byte()];
            let line_offset = raw_text_node.start_position().row as i64;
            let (script_nodes, script_edges) = parse_javascript_like_interned(
                &file_path,
                script_source,
                script_language,
                script_parser,
                inputs.repo_root,
                inputs.caches,
            );

            nodes.extend(script_nodes.into_iter().skip(1).map(|mut node| {
                node.line_start += line_offset;
                node.line_end += line_offset;
                node.language = inputs.language.to_string();
                node
            }));
            edges.extend(script_edges.into_iter().map(|mut edge| {
                edge.line += line_offset;
                edge
            }));
        }
    }

    (nodes, edges)
}

fn sfc_direct_child<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn sfc_script_language(node: tree_sitter::Node<'_>, source: &[u8]) -> &'static str {
    let Some(start_tag) = sfc_direct_child(node, "start_tag") else {
        return "javascript";
    };
    let mut cursor = start_tag.walk();
    for attr in start_tag.children(&mut cursor) {
        if attr.kind() != "attribute" {
            continue;
        }
        let Some(name) = sfc_child_text(attr, source, "attribute_name") else {
            continue;
        };
        if name != "lang" {
            continue;
        }
        if matches!(
            sfc_first_descendant_text(attr, source, &["attribute_value"]).as_deref(),
            Some("ts" | "typescript")
        ) {
            return "typescript";
        }
    }
    "javascript"
}

fn sfc_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
        .map(|child| node_text(child, source))
}

fn sfc_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = sfc_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}
