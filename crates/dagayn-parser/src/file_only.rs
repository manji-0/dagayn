use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count};

pub(super) fn parse_zig_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_tree_sitter_file_only_with_parser(file_path, source, "zig", parser)
}

pub(super) fn parse_powershell_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_tree_sitter_file_only_with_parser(file_path, source, "powershell", parser)
}

fn parse_tree_sitter_file_only_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    if let Some(parser) = parser {
        let _ = parser.parse(source, None);
    }
    let line_end = line_count(source);
    (
        vec![ParsedNode {
            kind: "File".to_string(),
            name: file_path.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end,
            language: language.to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: is_test_file(file_path),
            extra: json!({}),
        }],
        Vec::new(),
    )
}
