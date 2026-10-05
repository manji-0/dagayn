use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::line_count;

/// A File node and nothing else.
///
/// PowerShell always gets this: the graph has no PowerShell symbols, so
/// nothing is parsed. So does a file whose language's grammar this build
/// left out (its `lang-*` Cargo feature is off; see
/// `RustOwnedPathKind::grammar_enabled`): the file stays in the graph, with
/// no symbols, instead of being dropped or handed to another parser.
pub(super) fn parse_file_only(
    file_path: &str,
    source: &[u8],
    language: &str,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    (
        vec![ParsedNode::file(&file_path, line_count(source), language)],
        Vec::new(),
    )
}
