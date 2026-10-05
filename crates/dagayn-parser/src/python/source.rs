//! The source text of a Python file as Ruff's parser sees it: the text
//! (decoded lossily when it is not UTF-8), its parse, the line of a byte
//! offset, and the text of a node's range.

use std::borrow::Cow;

use ruff_python_ast::statement_visitor::{StatementVisitor, walk_stmt};
use ruff_python_ast::{Decorator, Expr, ModModule, PySourceType, Stmt};
use ruff_python_parser::{Parsed, parse_unchecked_source};
use ruff_text_size::{Ranged, TextRange, TextSize};

pub(crate) struct PySource<'s> {
    text: Cow<'s, str>,
    /// Byte offset of the start of each line. Lines end at `\n` only, as
    /// tree-sitter rows and `util::line_count` count them.
    line_starts: Vec<u32>,
}

impl<'s> PySource<'s> {
    pub(crate) fn new(source: &'s [u8]) -> Self {
        Self::from_text(String::from_utf8_lossy(source))
    }

    pub(crate) fn from_text(text: Cow<'s, str>) -> Self {
        let mut line_starts = Vec::with_capacity(text.len() / 32 + 1);
        line_starts.push(0);
        line_starts.extend(memchr::memchr_iter(b'\n', text.as_bytes()).map(|at| at as u32 + 1));
        Self { text, line_starts }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Parses the text as a module, recovering from syntax errors: the
    /// statements around an error are still in the tree.
    pub(crate) fn parse(&self) -> Parsed<ModModule> {
        parse_unchecked_source(&self.text, PySourceType::Python)
    }

    /// The 1-based line of a byte offset.
    pub(crate) fn line(&self, offset: TextSize) -> i64 {
        self.line_starts
            .partition_point(|start| *start <= offset.to_u32()) as i64
    }

    /// The 0-based byte column of an offset in its line.
    pub(crate) fn column(&self, offset: TextSize) -> u32 {
        let line = self.line(offset) as usize;
        offset.to_u32() - self.line_starts[line - 1]
    }

    pub(crate) fn start_line<T: Ranged>(&self, node: &T) -> i64 {
        self.line(node.start())
    }

    pub(crate) fn end_line<T: Ranged>(&self, node: &T) -> i64 {
        self.line(node.end())
    }

    pub(crate) fn slice<T: Ranged>(&self, node: &T) -> &str {
        self.text
            .get(node.range().to_std_range())
            .unwrap_or_default()
    }

    pub(crate) fn slice_range(&self, range: TextRange) -> &str {
        self.text.get(range.to_std_range()).unwrap_or_default()
    }

    /// Where a definition's `def` / `async` / `class` keyword starts. Ruff's
    /// range of a decorated definition starts at its first decorator; the
    /// graph keeps the line of the keyword, as it always has, and records
    /// the decorators in `extra`.
    pub(crate) fn definition_start(&self, range: TextRange, decorators: &[Decorator]) -> TextSize {
        let Some(last) = decorators.last() else {
            return range.start();
        };
        let bytes = self.text.as_bytes();
        let end = (range.end().to_usize()).min(bytes.len());
        let mut at = last.end().to_usize();
        while at < end {
            match bytes[at] {
                b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' => at += 1,
                b'\\' => at += 1,
                b'#' => {
                    at = memchr::memchr(b'\n', &bytes[at..end]).map_or(end, |offset| at + offset);
                }
                _ => break,
            }
        }
        TextSize::new(at.min(end) as u32)
    }
}

/// Calls `on_stmt` for every statement of `body`, nested ones included.
pub(crate) fn for_each_statement<'a>(body: &'a [Stmt], on_stmt: impl FnMut(&'a Stmt)) {
    struct Walker<F>(F);
    impl<'a, F: FnMut(&'a Stmt)> StatementVisitor<'a> for Walker<F> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            (self.0)(stmt);
            walk_stmt(self, stmt);
        }
    }
    Walker(on_stmt).visit_body(body);
}

/// The text of a string-like expression, as the extractor reads string
/// arguments: the decoded value of a string (implicit concatenation
/// joined), the literal parts of an f-string or t-string (interpolations
/// dropped), and a bytes literal decoded lossily.
pub(crate) fn string_like_text(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(string) => Some(string.value.to_str().to_string()),
        Expr::BytesLiteral(bytes) => {
            let value = bytes.value.bytes().collect::<Vec<_>>();
            Some(String::from_utf8_lossy(&value).into_owned())
        }
        Expr::FString(fstring) => {
            let mut text = String::new();
            for part in &fstring.value {
                match part {
                    ruff_python_ast::FStringPartRef::Literal(literal) => text.push_str(literal),
                    ruff_python_ast::FStringPartRef::FString(fstring) => {
                        for literal in fstring.elements.literals() {
                            text.push_str(literal);
                        }
                    }
                }
            }
            Some(text)
        }
        Expr::TString(tstring) => {
            let mut text = String::new();
            for tstring in &tstring.value {
                for literal in tstring.elements.literals() {
                    text.push_str(literal);
                }
            }
            Some(text)
        }
        _ => None,
    }
}

/// Whether an f-string or t-string interpolates a value.
pub(crate) fn has_interpolation(expr: &Expr) -> bool {
    match expr {
        Expr::FString(fstring) => fstring
            .value
            .f_strings()
            .any(|fstring| fstring.elements.interpolations().next().is_some()),
        Expr::TString(tstring) => tstring
            .value
            .iter()
            .any(|tstring| tstring.elements.interpolations().next().is_some()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_count_newlines_only() {
        let source = PySource::new(b"a\r\nb\rc\nd");
        assert_eq!(source.line(TextSize::new(0)), 1);
        assert_eq!(source.line(TextSize::new(3)), 2);
        assert_eq!(source.line(TextSize::new(6)), 2);
        assert_eq!(source.line(TextSize::new(7)), 3);
    }

    #[test]
    fn non_utf8_source_is_decoded_lossily() {
        let source = PySource::new(b"x = '\xff'\ndef f():\n    pass\n");
        let parsed = source.parse();
        assert_eq!(parsed.syntax().body.len(), 2);
        assert_eq!(source.start_line(&parsed.syntax().body[1]), 2);
    }

    #[test]
    fn definition_start_skips_decorators_and_comments() {
        let text = "@a\n# note\n@b(1)  # c\nasync def f():\n    pass\n";
        let source = PySource::new(text.as_bytes());
        let parsed = source.parse();
        let Stmt::FunctionDef(function) = &parsed.syntax().body[0] else {
            panic!("not a function");
        };
        let start = source.definition_start(function.range, &function.decorator_list);
        assert_eq!(source.line(start), 4);
        assert!(text[start.to_usize()..].starts_with("async def"));
    }
}
