//! Conservative mode, the owner's call: an identifier whose symbol has
//! several declarations gets one occurrence per distinct declaration symbol
//! at the same range, so the overlay counts the call `ambiguous` and leaves
//! it instead of choosing a declaration.

use protobuf::EnumOrUnknown;
use scip::types::{Document, Occurrence, PositionEncoding};
use tsrs_ast::{self as ast, Kind, Node, SourceFile, SymbolFlags};
use tsrs_compiler::Program;
use tsrs_core::P;
use tsrs_core::context::Context;

use crate::symbols::SymbolNamer;

const ROLE_DEFINITION: i32 = 1;
const ROLE_IMPORT: i32 = 2;

#[derive(Default)]
pub(crate) struct Counts {
    pub(crate) definitions: usize,
    pub(crate) references: usize,
    pub(crate) unresolved: usize,
}

fn is_import_like(k: Kind) -> bool {
    matches!(
        k,
        Kind::ImportSpecifier
            | Kind::ImportClause
            | Kind::NamespaceImport
            | Kind::ImportEqualsDeclaration
            | Kind::ExportSpecifier
    )
}

fn walk(node: P<Node>, f: &mut dyn FnMut(P<Node>)) {
    f(node);
    node.for_each_child(&mut |child| {
        walk(child, f);
        false
    });
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

/// `[line, start column, end column]` with columns in UTF-16 code units,
/// as `PositionEncoding::UTF16CodeUnitOffsetFromLineStart` promises.
fn range_utf16(text: &str, starts: &[usize], start: usize, end: usize) -> Vec<i32> {
    let line = starts.partition_point(|&s| s <= start) - 1;
    let ls = starts[line];
    let c0 = text[ls..start].encode_utf16().count();
    let c1 = c0 + text[start..end].encode_utf16().count();
    vec![line as i32, c0 as i32, c1 as i32]
}

fn occurrence(range: Vec<i32>, symbol: String, symbol_roles: i32) -> Occurrence {
    let mut o = Occurrence::new();
    o.range = range;
    o.symbol = symbol;
    o.symbol_roles = symbol_roles;
    o
}

pub(crate) fn document(
    program: &Program,
    ctx: &Context,
    sf: P<SourceFile>,
    relative_path: String,
    namer: &mut SymbolNamer,
    counts: &mut Counts,
) -> Document {
    namer.start_document();
    let text = sf.text();
    let starts = line_starts(text);
    let mut checker = program.get_type_checker_for_file(ctx, sf);
    let mut occurrences: Vec<Occurrence> = Vec::new();
    if let Some(module) = namer.symbol_for(sf.as_node()) {
        occurrences.push(occurrence(vec![0, 0, 0], module, ROLE_DEFINITION));
    }
    walk(sf.as_node(), &mut |node: P<Node>| {
        if node.kind() != Kind::Identifier && node.kind() != Kind::PrivateIdentifier {
            return;
        }
        let start = tsrs_scanner::get_token_pos_of_node(node, sf, false) as usize;
        let end = node.end().expect_source("identifier end") as usize;
        if end <= start || end > text.len() {
            return;
        }
        let parent = node.parent();
        let import = parent.is_some_and(|p| is_import_like(p.kind()));
        if ast::is_declaration_name(node) && !import {
            let Some(symbol) = parent.and_then(|p| namer.symbol_for(p)) else {
                return;
            };
            counts.definitions += 1;
            occurrences.push(occurrence(
                range_utf16(text, &starts, start, end),
                symbol,
                ROLE_DEFINITION,
            ));
            return;
        }
        let Some(mut resolved) = checker.get_symbol_at_location_exported(node) else {
            counts.unresolved += 1;
            return;
        };
        if resolved.flags().contains(SymbolFlags::Alias) {
            resolved = checker.get_aliased_symbol(resolved);
        }
        let mut symbols: Vec<String> = Vec::new();
        for &decl in resolved.declarations() {
            if let Some(symbol) = namer.symbol_for(decl)
                && !symbols.contains(&symbol)
            {
                symbols.push(symbol);
            }
        }
        if symbols.is_empty() {
            counts.unresolved += 1;
            return;
        }
        counts.references += 1;
        let roles = if import { ROLE_IMPORT } else { 0 };
        for symbol in symbols {
            occurrences.push(occurrence(
                range_utf16(text, &starts, start, end),
                symbol,
                roles,
            ));
        }
    });
    let mut d = Document::new();
    d.relative_path = relative_path;
    d.language = "TypeScript".to_string();
    d.position_encoding = EnumOrUnknown::new(PositionEncoding::UTF16CodeUnitOffsetFromLineStart);
    d.occurrences = occurrences;
    d
}

#[cfg(test)]
mod tests {
    use super::{line_starts, range_utf16};

    #[test]
    fn columns_count_utf16_units() {
        let text = "a\nconst s = \"\u{1F6D2}\"; total()\n";
        let starts = line_starts(text);
        let start = text.find("total").unwrap();
        assert_eq!(
            range_utf16(text, &starts, start, start + 5),
            vec![1, 16, 21]
        );
    }
}
