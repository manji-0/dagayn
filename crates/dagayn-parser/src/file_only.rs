use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::line_count;

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
    let file_path = FilePath::new(file_path);
    if let Some(parser) = parser {
        let _ = parser.parse(source, None);
    }
    let line_end = line_count(source);
    (
        vec![ParsedNode::file(&file_path, line_end, language)],
        Vec::new(),
    )
}
