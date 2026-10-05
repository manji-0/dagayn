use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::line_count;

/// PowerShell gets a File node only: the graph has no PowerShell symbols, so
/// nothing is parsed.
pub(super) fn parse_powershell(
    file_path: &str,
    source: &[u8],
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    (
        vec![ParsedNode::file(
            &file_path,
            line_count(source),
            "powershell",
        )],
        Vec::new(),
    )
}
