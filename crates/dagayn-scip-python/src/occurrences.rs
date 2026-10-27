//! Conservative mode, as for TypeScript: a name whose definitions have
//! several distinct symbols (a module imported under one name in two
//! branches) gets one occurrence per symbol at the same range, so the
//! overlay counts the call `ambiguous` and leaves it instead of choosing.

use protobuf::EnumOrUnknown;
use ruff_db::files::File;
use ruff_db::parsed::parsed_module;
use ruff_db::source::source_text;
use ruff_python_ast::visitor::source_order::{self, SourceOrderVisitor, TraversalSignal};
use ruff_python_ast::{AnyNodeRef, Expr, Identifier};
use ruff_source_file::LineIndex;
use ruff_text_size::{Ranged, TextRange, TextSize};
use scip::types::{Document, Occurrence, PositionEncoding};
use ty_project::{ProjectDatabase, SemanticDb as _};

use crate::symbols::BindingNamer;

const ROLE_DEFINITION: i32 = 1;
const ROLE_IMPORT: i32 = 2;
const ROLE_READ_ACCESS: i32 = 8;

#[derive(Default)]
pub(crate) struct Counts {
    pub(crate) definitions: usize,
    pub(crate) references: usize,
    pub(crate) unresolved: usize,
    /// References emitted with several symbols.
    pub(crate) ambiguous: usize,
}

impl Counts {
    pub(crate) fn add(&mut self, other: &Counts) {
        self.definitions += other.definitions;
        self.references += other.references;
        self.unresolved += other.unresolved;
        self.ambiguous += other.ambiguous;
    }
}

/// Every name of a file in source order, and whether an import statement
/// holds it.
struct Names {
    ranges: Vec<(TextRange, bool)>,
    import_depth: usize,
}

impl Names {
    fn push(&mut self, range: TextRange) {
        self.ranges.push((range, self.import_depth > 0));
    }
}

impl<'a> SourceOrderVisitor<'a> for Names {
    fn enter_node(&mut self, node: AnyNodeRef<'a>) -> TraversalSignal {
        if matches!(
            node,
            AnyNodeRef::StmtImport(_) | AnyNodeRef::StmtImportFrom(_)
        ) {
            self.import_depth += 1;
        }
        TraversalSignal::Traverse
    }

    fn leave_node(&mut self, node: AnyNodeRef<'a>) {
        if matches!(
            node,
            AnyNodeRef::StmtImport(_) | AnyNodeRef::StmtImportFrom(_)
        ) {
            self.import_depth -= 1;
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr {
            self.push(name.range());
        }
        source_order::walk_expr(self, expr);
    }

    fn visit_identifier(&mut self, identifier: &'a Identifier) {
        self.push(identifier.range());
    }
}

/// `[line, start column, end column]` with columns in UTF-16 code units, as
/// `PositionEncoding::UTF16CodeUnitOffsetFromLineStart` promises.
fn range_utf16(text: &str, index: &LineIndex, range: TextRange) -> Vec<i32> {
    let line = index.line_index(range.start());
    let line_start = usize::from(index.line_start(line, text));
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let c0 = text[line_start..start].encode_utf16().count();
    let c1 = c0 + text[start..end].encode_utf16().count();
    let as_i32 = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    vec![as_i32(line.to_zero_indexed()), as_i32(c0), as_i32(c1)]
}

fn occurrence(range: Vec<i32>, symbol: String, symbol_roles: i32) -> Occurrence {
    let mut o = Occurrence::new();
    o.range = range;
    o.symbol = symbol;
    o.symbol_roles = symbol_roles;
    o
}

pub(crate) fn document(
    db: &ProjectDatabase,
    file: File,
    relative_path: String,
    namer: &mut BindingNamer<'_>,
    counts: &mut Counts,
) -> Document {
    namer.start_document(file);
    let text = source_text(db, file);
    let text = text.as_str();
    let index = LineIndex::from_source_text(text);
    let program_file = db.program_file(file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    let mut names = Names {
        ranges: Vec::new(),
        import_depth: 0,
    };
    names.visit_body(&parsed.syntax().body);

    let mut occurrences: Vec<Occurrence> = Vec::new();
    if let Some(module) = namer.module(db, file) {
        occurrences.push(occurrence(vec![0, 0, 0], module, ROLE_DEFINITION));
    }
    for (mut range, import) in names.ranges {
        // `import os.path` and `from a.b import c` name a dotted module:
        // the occurrence is its last segment.
        if let Some(dot) = text[range].rfind('.') {
            let offset = TextSize::try_from(dot + 1).unwrap_or_default();
            range = TextRange::new(range.start() + offset, range.end());
        }
        if range.is_empty() {
            continue;
        }
        if !import && namer.is_binding(db, file, range.start()) {
            if let Some(symbol) = namer.binding(db, file, range.start()) {
                counts.definitions += 1;
                occurrences.push(occurrence(
                    range_utf16(text, &index, range),
                    symbol,
                    ROLE_DEFINITION,
                ));
            }
            continue;
        }
        let Some(targets) = ty_ide::goto_definition(db, program_file, range.start()) else {
            counts.unresolved += 1;
            continue;
        };
        let mut symbols: Vec<String> = Vec::new();
        for target in &targets.value {
            let focus = target.focus_range();
            let symbol = if focus.is_empty() && focus.start() == TextSize::default() {
                namer.module(db, target.file())
            } else {
                namer.binding(db, target.file(), focus.start())
            };
            if let Some(symbol) = symbol
                && !symbols.contains(&symbol)
            {
                symbols.push(symbol);
            }
        }
        if symbols.is_empty() {
            counts.unresolved += 1;
            continue;
        }
        counts.references += 1;
        if symbols.len() > 1 {
            counts.ambiguous += 1;
        }
        let roles = if import {
            ROLE_IMPORT
        } else {
            ROLE_READ_ACCESS
        };
        for symbol in symbols {
            occurrences.push(occurrence(range_utf16(text, &index, range), symbol, roles));
        }
    }
    let mut d = Document::new();
    d.relative_path = relative_path;
    d.language = "python".to_string();
    d.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF16CodeUnitOffsetFromLineStart);
    d.occurrences = occurrences;
    d
}

#[cfg(test)]
mod tests {
    use ruff_source_file::LineIndex;
    use ruff_text_size::{TextRange, TextSize};

    use super::range_utf16;

    #[test]
    fn columns_count_utf16_units() {
        let text = "a = 1\ns = \"\u{1F6D2}\"; total()\n";
        let index = LineIndex::from_source_text(text);
        let start = TextSize::try_from(text.find("total").unwrap()).unwrap();
        let range = TextRange::at(start, TextSize::from(5));
        assert_eq!(range_utf16(text, &index, range), vec![1, 10, 15]);
    }
}
