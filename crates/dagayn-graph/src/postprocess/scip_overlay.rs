//! Call targets from a SCIP index (`docs/plans/SCIP-CALL-RESOLUTION.md`).
//!
//! A compiler-grade indexer (`rust-analyzer scip`, dagayn's own
//! `dagayn-scip-typescript`) says which definition each reference names. A `CALLS` edge is matched to the
//! reference at its called name's position: the edge's line, or one of the
//! next eight for a chain the extractor records at the expression's first
//! line. Its answer then settles the edge:
//!
//! - a definition of the repository: the node whose range holds it, `HIGH`;
//!   an edge already at that node (or at a node containing it, a class for
//!   its constructor) is confirmed;
//! - a symbol no index of the language defines: its package, `external`,
//!   `HIGH`; an edge already external is confirmed and keeps its package
//!   name. The definitions of every index of the language are pooled before
//!   any edge is settled: a project's index names a workspace package's
//!   symbols without defining them (its files are not the project's), and
//!   the package's own index, or the root's, holds the definition;
//! - a local (a closure, a callable parameter): an unresolved edge records
//!   `callee_local`; other edges are left;
//! - no reference, or several: the edge is left as resolution left it.
//!
//! Every edge the index settles records `resolved_by: "scip"`. Files whose
//! content no longer matches what the graph parsed are skipped.

use std::path::{Path, PathBuf};

use ::scip::types::Index;
use protobuf::Message;
use sha2::{Digest, Sha256};

use crate::helpers::*;
use crate::*;

use super::bare_names::DIRECT_IMPORT_CONFIDENCE;
use super::sync_tested_by_with_calls;

/// Lines after an edge's own searched for its called name.
const CHAIN_LINES: i64 = 8;

/// What an overlay run did, edge by edge.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ScipOverlayStats {
    /// Documents in the index.
    pub documents: i64,
    /// `CALLS` edges in indexed files.
    pub calls: i64,
    /// Edges in files changed since the graph parsed them.
    pub stale_skipped: i64,
    /// Edges the index agrees with.
    pub confirmed: i64,
    /// Edges moved to the node of the definition the index names.
    pub rewritten_to_node: i64,
    /// Edges moved to the package the index names.
    pub rewritten_to_package: i64,
    /// Unresolved edges whose callee is a local.
    pub marked_local: i64,
    /// Edges whose name has several references on the line.
    pub ambiguous: i64,
    /// Edges with no reference at their name.
    pub unmatched: i64,
    /// Edges whose definition lies outside every node (or is a local left
    /// alone).
    pub left: i64,
}

#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

struct Reference {
    start: usize,
    end: usize,
    symbol: String,
}

/// Symbol -> where it is defined. rust-analyzer gives items of one
/// package's targets (`src/lib.rs::answer` and `tests/tools.rs::answer`)
/// and same-named nested functions of one module the same symbol.
type Definitions = HashMap<String, Vec<(String, i64)>>;

#[derive(Default)]
struct ScipIndex {
    documents: HashMap<String, Encoding>,
    references: HashMap<(String, i64), Vec<Reference>>,
    definitions: Definitions,
}

/// One index of a build: where it is, the prefix that turns its document
/// paths into the graph's (`frontend/` for an index made in that directory),
/// and whether its answer replaces resolution's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipIndexInput {
    pub path: PathBuf,
    pub prefix: String,
    pub authoritative: bool,
}

const ROLE_DEFINITION: i32 = 1;
const ROLE_IMPORT: i32 = 2;

fn read_index(path: &Path, prefix: &str) -> Result<ScipIndex> {
    let bytes = std::fs::read(path)
        .map_err(|err| GraphError::Scip(format!("{}: {err}", path.display())))?;
    let index = Index::parse_from_bytes(&bytes)
        .map_err(|err| GraphError::Scip(format!("{}: {err}", path.display())))?;
    let mut read = ScipIndex::default();
    for document in index.documents {
        let file = graph_path(prefix, &document.relative_path);
        let encoding = match document.position_encoding.value() {
            1 => Encoding::Utf8,
            2 => Encoding::Utf16,
            3 => Encoding::Utf32,
            // scip-typescript left the encoding unspecified and counted
            // UTF-16 units.
            _ => Encoding::Utf16,
        };
        read.documents.insert(file.clone(), encoding);
        for occurrence in document.occurrences {
            let range = &occurrence.range;
            if range.len() < 3 || occurrence.symbol.is_empty() {
                continue;
            }
            let line = i64::from(range[0]) + 1;
            if occurrence.symbol_roles & ROLE_DEFINITION != 0 {
                let definitions = read
                    .definitions
                    .entry(occurrence.symbol.clone())
                    .or_default();
                if !definitions.contains(&(file.clone(), line)) {
                    definitions.push((file.clone(), line));
                }
            }
            if occurrence.symbol_roles & (ROLE_DEFINITION | ROLE_IMPORT) != 0 {
                continue;
            }
            let start = usize::try_from(range[1]).unwrap_or_default();
            // A four-element range spans lines: it covers the rest of the first.
            let end = match range.len() {
                3 => usize::try_from(range[2]).unwrap_or_default(),
                _ => usize::MAX,
            };
            read.references
                .entry((file.clone(), line))
                .or_default()
                .push(Reference {
                    start,
                    end,
                    symbol: occurrence.symbol,
                });
        }
    }
    Ok(read)
}

/// A tsconfig may include a file outside its directory
/// (`../../worker-configuration.d.ts`); the graph parsed it at the resolved
/// path, and so did the root's index.
fn graph_path(prefix: &str, relative_path: &str) -> String {
    let path = format!("{prefix}{relative_path}");
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." if segments.last().is_some_and(|last| *last != "..") => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    segments.join("/")
}

/// The called name of an edge: the last segment of its external symbol, of
/// what follows the package of a JavaScript target, or of the target.
fn called_name(target: &str, extra: &Value) -> String {
    let external = extra.get("external").and_then(Value::as_bool) == Some(true);
    let text = extra
        .get("external_symbol")
        .and_then(Value::as_str)
        .or_else(|| {
            external
                .then(|| target.split_once("::").map(|(_, symbol)| symbol))
                .flatten()
        })
        .unwrap_or(target);
    let text = text.trim_end_matches('!');
    let last = text.rsplit("::").next().unwrap_or(text);
    let last = last.rsplit('.').next().unwrap_or(last);
    last.split(['(', '<']).next().unwrap_or(last).to_string()
}

/// Columns, in the document's units, where `name` stands as a whole word in
/// `line`; for a member call, only after `.`, `::`, or `->`
/// (`map.into_iter().map(..)` names the method, not the variable `map`).
fn name_columns(line: &str, name: &str, encoding: Encoding, member: bool) -> Vec<usize> {
    if name.is_empty() {
        return Vec::new();
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    line.match_indices(name)
        .filter(|(at, _)| {
            !line[..*at].chars().next_back().is_some_and(is_word)
                && !line[at + name.len()..].chars().next().is_some_and(is_word)
                && (!member || {
                    let before = line[..*at].trim_end();
                    before.ends_with('.') || before.ends_with("::") || before.ends_with("->")
                })
        })
        .map(|(at, _)| match encoding {
            Encoding::Utf8 => at,
            Encoding::Utf16 => line[..at].encode_utf16().count(),
            Encoding::Utf32 => line[..at].chars().count(),
        })
        .collect()
}

/// The package a SCIP symbol belongs to, as dagayn names it, whether it is
/// the standard library, and the symbol as `Type::method` / `Type.method`.
fn external_target(symbol: &str, separator: &str) -> Option<(String, bool, String)> {
    let parsed = ::scip::symbol::parse_symbol(symbol).ok()?;
    let package = parsed.package.into_option()?;
    // scip-typescript names a package it resolved through a pnpm realpath
    // `.` (#219): no package is better than an edge to `''`.
    if package.name.is_empty() || package.name == "." {
        return None;
    }
    let descriptors = parsed.descriptors;
    let (name, stdlib) = match (package.manager.as_str(), package.name.as_str()) {
        ("cargo", "std" | "core" | "alloc" | "proc_macro" | "test") => ("std".to_string(), true),
        ("npm", "typescript") => ("globalThis".to_string(), true),
        ("npm", "@types/node") => {
            let module = descriptors
                .first()
                .and_then(|first| first.name.strip_suffix(".d.ts"))
                .map(|module| format!("node:{module}"))
                .unwrap_or_else(|| "node".to_string());
            (module, true)
        }
        ("cargo" | "npm", name) => (
            name.strip_prefix("@types/").unwrap_or(name).to_string(),
            false,
        ),
        // scip-go: the import path is the first namespace (`net/http`), the
        // standard library the `github.com/golang/go/src` module.
        ("gomod", module) => (
            descriptors.first()?.name.clone(),
            module == "github.com/golang/go/src",
        ),
        // scip-python: the module is the first namespace (`os.path`), named
        // by its top-level package as dagayn names it (`os`, `yaml`).
        ("python", distribution) => {
            let module = &descriptors.first()?.name;
            (
                module.split('.').next().unwrap_or(module).to_string(),
                distribution == "python-stdlib",
            )
        }
        // Other indexers name packages in ways not mapped yet.
        _ => return None,
    };
    // Namespaces and files are the path; the type and member the symbol.
    let named = descriptors
        .iter()
        .filter(|descriptor| matches!(descriptor.suffix.value(), 2 | 3 | 4 | 9))
        .map(|descriptor| descriptor.name.as_str())
        .collect::<Vec<_>>();
    let tail = named.len().saturating_sub(2);
    Some((name, stdlib, named[tail..].join(separator)))
}

/// Descriptor suffixes of a value named in a function: a type parameter, a
/// parameter, a local.
const LOCAL_SUFFIXES: [i32; 3] = [5, 6, 8];

/// Whether a SCIP symbol names a local, a parameter, or a type parameter: a
/// callee nothing static settles (`onExit(code)` of an `onExit` parameter).
fn is_callee_local(symbol: &str) -> bool {
    ::scip::symbol::is_local_symbol(symbol)
        || ::scip::symbol::parse_symbol(symbol)
            .ok()
            .and_then(|parsed| parsed.descriptors.last().map(|last| last.suffix.value()))
            .is_some_and(|suffix| LOCAL_SUFFIXES.contains(&suffix))
}

/// The node names a SCIP symbol's definition may carry: its last
/// descriptor's, and for a TypeScript constructor (`` Store#`<constructor>`(). ``)
/// `constructor` and its class's.
fn defined_names(symbol: &str) -> Vec<String> {
    let Ok(parsed) = ::scip::symbol::parse_symbol(symbol) else {
        return Vec::new();
    };
    let names = parsed
        .descriptors
        .iter()
        .map(|descriptor| descriptor.name.clone())
        .collect::<Vec<_>>();
    match names.as_slice() {
        [.., owner, last] if last == "<constructor>" => {
            vec!["constructor".to_string(), owner.clone()]
        }
        [.., last] => vec![last.clone()],
        [] => Vec::new(),
    }
}

/// A qualified name as its path, file, and symbol segments
/// (`a/b.rs::f.g` is `a`, `b.rs`, `::`, `f`, `g`).
fn scope_segments(qualified: &str) -> Vec<&str> {
    let (file, symbol) = qualified.split_once("::").unwrap_or((qualified, ""));
    let mut segments: Vec<&str> = file.split('/').collect();
    segments.push("::");
    segments.extend(symbol.split('.').filter(|segment| !segment.is_empty()));
    segments
}

/// Of the nodes defining one symbol, the one the caller `source` sees: the
/// one sharing the most leading scope segments with it (its own target's
/// `answer`, the `visit` nested in its own function). `None` on a tie.
fn visible_definition<'a>(source: &str, candidates: &'a [String]) -> Option<&'a String> {
    let source = scope_segments(source);
    let shared = |candidate: &String| {
        scope_segments(candidate)
            .iter()
            .zip(&source)
            .take_while(|(a, b)| a == b)
            .count()
    };
    let best = candidates.iter().map(shared).max()?;
    let mut nearest = candidates
        .iter()
        .filter(|candidate| shared(candidate) == best);
    let first = nearest.next()?;
    nearest.next().is_none().then_some(first)
}

fn settle(extra: &mut Value) {
    if !extra.is_object() {
        *extra = json!({});
    }
    if let Some(map) = extra.as_object_mut() {
        for key in ["receiver_unknown", "inferred_from"] {
            map.remove(key);
        }
    }
    extra["resolved_by"] = json!("scip");
}

/// Writes an edge's target and metadata; with a tier, also its confidence,
/// recorded in `extra` too, which endpoint demotion reads to keep an
/// external edge's tier.
fn write_edge(
    tx: &Transaction<'_>,
    id: i64,
    target: &str,
    extra: &Value,
    tier: Option<ConfidenceTier>,
) -> Result<()> {
    let Some(tier) = tier else {
        tx.execute(
            "UPDATE edges SET extra = ? WHERE id = ?",
            params![serde_json::to_string(extra)?, id],
        )?;
        return Ok(());
    };
    let mut extra = extra.clone();
    extra["confidence"] = json!(DIRECT_IMPORT_CONFIDENCE);
    extra["confidence_tier"] = json!(tier.as_str());
    tx.execute(
        "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
         confidence = ?, confidence_tier = ? WHERE id = ?",
        params![
            target,
            edge_target_name(target),
            serde_json::to_string(&extra)?,
            DIRECT_IMPORT_CONFIDENCE,
            tier.as_str(),
            id
        ],
    )?;
    Ok(())
}

impl GraphStore {
    /// Settles `CALLS` edges by one language's SCIP indexes, in order (see
    /// the module docs); `repo_root` holds the files the graph parsed. An
    /// `authoritative` index moves any edge it disagrees with; otherwise it
    /// only settles edges resolution left unresolved and confirms the ones
    /// it agrees with. One stats entry per index.
    pub fn apply_scip_overlays(
        &mut self,
        indexes: &[ScipIndexInput],
        repo_root: &Path,
    ) -> Result<Vec<ScipOverlayStats>> {
        let mut read = indexes
            .iter()
            .map(|input| read_index(&input.path, &input.prefix))
            .collect::<Result<Vec<_>>>()?;
        let mut definitions = Definitions::new();
        for index in &mut read {
            for (symbol, places) in index.definitions.drain() {
                let pooled = definitions.entry(symbol).or_default();
                for place in places {
                    if !pooled.contains(&place) {
                        pooled.push(place);
                    }
                }
            }
        }
        indexes
            .iter()
            .zip(&read)
            .map(|(input, index)| {
                self.settle_by_index(index, &definitions, repo_root, input.authoritative)
            })
            .collect()
    }

    fn settle_by_index(
        &mut self,
        index: &ScipIndex,
        definitions: &Definitions,
        repo_root: &Path,
        authoritative: bool,
    ) -> Result<ScipOverlayStats> {
        let tx = write_tx(&mut self.conn)?;
        let mut stats = ScipOverlayStats {
            documents: i64::try_from(index.documents.len()).unwrap_or(i64::MAX),
            ..ScipOverlayStats::default()
        };
        let mut nodes: HashMap<String, (String, i64, i64)> = HashMap::new();
        let mut by_file: HashMap<String, Vec<(i64, i64, String)>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT qualified_name, file_path, line_start, line_end FROM nodes \
                 WHERE kind IN ('Function', 'Test', 'Class', 'Type')",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?;
            for row in rows {
                let (qualified, file, start, end) = row?;
                let end = end.unwrap_or(start);
                by_file
                    .entry(file.clone())
                    .or_default()
                    .push((start, end, qualified.clone()));
                nodes.insert(qualified, (file, start, end));
            }
        }
        // The innermost node holding a definition, of one of the names the
        // symbol gives: a field or a parameter defined in a function is not
        // that function.
        let node_at = |file: &str, line: i64, names: &[String]| {
            by_file
                .get(file)?
                .iter()
                .filter(|(start, end, qualified)| {
                    *start <= line
                        && line <= *end
                        && names
                            .iter()
                            .any(|name| qualified.rsplit(['.', ':']).next() == Some(name))
                })
                .min_by_key(|(start, end, _)| end - start)
                .map(|(_, _, qualified)| qualified.clone())
        };
        let hashes: HashMap<String, String> = {
            let mut stmt =
                tx.prepare("SELECT DISTINCT file_path, file_hash FROM nodes WHERE kind = 'File'")?;
            let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let edges = {
            let mut stmt = tx.prepare(
                "SELECT id, source_qualified, target_qualified, file_path, line, \
                 confidence_tier, extra FROM edges WHERE kind = 'CALLS' ORDER BY file_path, line",
            )?;
            let rows = stmt.query_map([], |row| {
                <(
                    i64,
                    String,
                    String,
                    String,
                    i64,
                    Option<String>,
                    Option<String>,
                )>::try_from(row)
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        // File -> its lines, or `None` when it changed since the graph
        // parsed it.
        let mut sources: HashMap<String, Option<Vec<String>>> = HashMap::new();
        for (id, source, target, file, line, tier, extra) in edges {
            let Some(&encoding) = index.documents.get(&file) else {
                continue;
            };
            stats.calls += 1;
            let lines = sources.entry(file.clone()).or_insert_with(|| {
                let bytes = std::fs::read(repo_root.join(&file)).ok()?;
                let digest = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                (hashes.get(&file) == Some(&digest)).then(|| {
                    String::from_utf8_lossy(&bytes)
                        .lines()
                        .map(str::to_string)
                        .collect()
                })
            });
            let Some(lines) = lines else {
                stats.stale_skipped += 1;
                continue;
            };
            let mut extra: Value = extra
                .as_deref()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or_else(|| json!({}));
            let name = called_name(&target, &extra);
            let member = ["receiver_unknown", "receiver_from", "receiver_type"]
                .iter()
                .any(|key| extra.get(key).is_some_and(|value| !value.is_null()));
            let mut symbols = Vec::<&str>::new();
            for at in line..=line + CHAIN_LINES {
                let Some(text) = usize::try_from(at - 1).ok().and_then(|i| lines.get(i)) else {
                    break;
                };
                for column in name_columns(text, &name, encoding, member) {
                    for reference in index
                        .references
                        .get(&(file.clone(), at))
                        .into_iter()
                        .flatten()
                    {
                        if reference.start <= column
                            && column < reference.end
                            && !symbols.contains(&reference.symbol.as_str())
                        {
                            symbols.push(&reference.symbol);
                        }
                    }
                }
                if !symbols.is_empty() {
                    break;
                }
            }
            let symbol = match symbols.as_slice() {
                [] => {
                    stats.unmatched += 1;
                    continue;
                }
                [symbol] => *symbol,
                _ => {
                    stats.ambiguous += 1;
                    continue;
                }
            };
            let tier = ConfidenceTier::from_raw(tier.as_deref());
            let at_node = nodes.get(&target);
            let external = extra.get("external").and_then(Value::as_bool) == Some(true);
            let raise = matches!(tier, ConfidenceTier::Medium | ConfidenceTier::Low)
                .then_some(ConfidenceTier::High);
            // Resolution's own answer, which only an authoritative index moves.
            let kept = !authoritative && (at_node.is_some() || external);
            if is_callee_local(symbol) {
                if at_node.is_none() && !external {
                    extra["callee_local"] = json!(true);
                    write_edge(&tx, id, &target, &extra, None)?;
                    stats.marked_local += 1;
                } else {
                    stats.left += 1;
                }
                continue;
            }
            if let Some(places) = definitions.get(symbol) {
                let names = defined_names(symbol);
                let defined: Vec<((&String, i64), Option<String>)> = places
                    .iter()
                    .map(|(file, line)| ((file, *line), node_at(file, *line, &names)))
                    .collect();
                let definition = match defined.as_slice() {
                    [only] => Some(only),
                    _ => {
                        let nodes: Vec<String> = defined
                            .iter()
                            .filter_map(|(_, node)| node.clone())
                            .collect();
                        visible_definition(&source, &nodes)
                            .and_then(|node| defined.iter().find(|(_, n)| n.as_ref() == Some(node)))
                    }
                };
                let Some(((definition_file, definition_line), node)) = definition else {
                    stats.ambiguous += 1;
                    continue;
                };
                let (definition_file, definition_line) = (*definition_file, *definition_line);
                let holds_definition = at_node.is_some_and(|(node_file, start, end)| {
                    node_file == definition_file
                        && *start <= definition_line
                        && definition_line <= *end
                });
                if holds_definition {
                    settle(&mut extra);
                    write_edge(&tx, id, &target, &extra, raise)?;
                    stats.confirmed += 1;
                    continue;
                }
                if kept {
                    stats.left += 1;
                    continue;
                }
                let Some(node) = node else {
                    stats.left += 1;
                    continue;
                };
                settle(&mut extra);
                if let Some(map) = extra.as_object_mut() {
                    for key in ["external", "external_package", "external_symbol", "stdlib"] {
                        map.remove(key);
                    }
                }
                write_edge(&tx, id, node, &extra, Some(ConfidenceTier::High))?;
                stats.rewritten_to_node += 1;
                continue;
            }
            if external {
                settle(&mut extra);
                write_edge(&tx, id, &target, &extra, raise)?;
                stats.confirmed += 1;
                continue;
            }
            if kept {
                stats.left += 1;
                continue;
            }
            let separator = match file.rsplit('.').next() {
                Some("rs" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "h") => "::",
                _ => ".",
            };
            let Some((package, stdlib, symbol_name)) = external_target(symbol, separator) else {
                stats.left += 1;
                continue;
            };
            settle(&mut extra);
            extra["external"] = json!(true);
            extra["external_package"] = json!(package);
            extra["external_symbol"] = json!(symbol_name);
            if stdlib {
                extra["stdlib"] = json!(true);
            } else if let Some(map) = extra.as_object_mut() {
                map.remove("stdlib");
            }
            write_edge(&tx, id, &package, &extra, Some(ConfidenceTier::High))?;
            stats.rewritten_to_package += 1;
        }
        sync_tested_by_with_calls(&tx)?;
        tx.commit()?;
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_and_symbols_are_named_as_dagayn_names_them() {
        let target =
            |symbol: &str, rust: bool| external_target(symbol, if rust { "::" } else { "." });
        assert_eq!(
            target("rust-analyzer cargo alloc 1.0.0 vec/Vec#push().", true),
            Some(("std".to_string(), true, "Vec::push".to_string()))
        );
        assert_eq!(
            target(
                "rust-analyzer cargo rusqlite 0.32.1 Connection#prepare().",
                true
            ),
            Some((
                "rusqlite".to_string(),
                false,
                "Connection::prepare".to_string()
            ))
        );
        assert_eq!(
            target(
                "scip-typescript npm @types/vscode 1.125.0 `vscode.d.ts`/`\"vscode\"`/window/createOutputChannel().",
                false
            ),
            // Namespaces are the path, as Rust modules are.
            Some((
                "vscode".to_string(),
                false,
                "createOutputChannel".to_string()
            ))
        );
        assert_eq!(
            target(
                "scip-typescript npm @types/node 22.0.0 `fs.d.ts`/readFileSync().",
                false
            ),
            Some(("node:fs".to_string(), true, "readFileSync".to_string()))
        );
        assert_eq!(
            target(
                "scip-typescript npm typescript 5.9.3 lib/`lib.es5.d.ts`/Array#map().",
                false
            ),
            Some(("globalThis".to_string(), true, "Array.map".to_string()))
        );
    }

    #[test]
    fn a_document_outside_its_project_is_the_file_the_graph_parsed() {
        assert_eq!(graph_path("", "src/a.ts"), "src/a.ts");
        assert_eq!(graph_path("web/", "src/a.ts"), "web/src/a.ts");
        assert_eq!(
            graph_path("packages/worker/", "../../worker-configuration.d.ts"),
            "worker-configuration.d.ts"
        );
        assert_eq!(graph_path("web/", "../../outside.ts"), "../outside.ts");
    }

    #[test]
    fn a_symbol_without_a_package_name_has_no_external_target() {
        // scip-typescript writes `npm . .` for a package it resolved through a
        // pnpm realpath (#219); the scip parser reads `.` as the empty name.
        let symbol = "scip-typescript npm . . `context.d.ts`/Context#json().";
        let parsed = ::scip::symbol::parse_symbol(symbol).unwrap();
        assert_eq!(parsed.package.into_option().unwrap().name, "");
        assert_eq!(external_target(symbol, "."), None);
        assert_eq!(
            external_target("scip-typescript npm  1.0.0 `a.ts`/f().", "."),
            None
        );
    }

    #[test]
    fn go_and_python_packages_are_their_import_paths() {
        let target = |symbol: &str| external_target(symbol, ".");
        assert_eq!(
            target("scip-go gomod github.com/golang/go/src go1.22 `net/http`/ListenAndServe()."),
            Some(("net/http".to_string(), true, "ListenAndServe".to_string()))
        );
        assert_eq!(
            target(
                "scip-go gomod github.com/spf13/cobra v1.8.0 `github.com/spf13/cobra`/Command#Execute()."
            ),
            Some((
                "github.com/spf13/cobra".to_string(),
                false,
                "Command.Execute".to_string()
            ))
        );
        assert_eq!(
            target("scip-python python python-stdlib 3.11 `os.path`/join()."),
            Some(("os".to_string(), true, "join".to_string()))
        );
        assert_eq!(
            target("scip-python python PyYAML 6.0.3 `yaml`/safe_load()."),
            Some(("yaml".to_string(), false, "safe_load".to_string()))
        );
        // An indexer whose package naming is not mapped names no package.
        assert_eq!(
            target("semanticdb maven jdk 17 java/lang/String#length()."),
            None
        );
    }

    #[test]
    fn parameters_are_locals_and_constructors_their_class() {
        let parameter = "scip-typescript npm demo 1.0.0 `cli.ts`/CliWrapper#spawnWatch().(onExit)";
        assert!(is_callee_local(parameter));
        assert!(is_callee_local("local 7"));
        assert!(!is_callee_local(
            "scip-typescript npm demo 1.0.0 `cli.ts`/CliWrapper#spawnWatch()."
        ));
        assert_eq!(
            defined_names(
                "scip-typescript npm demo 1.0.0 `sqlite.ts`/SqliteReader#`<constructor>`()."
            ),
            vec!["constructor".to_string(), "SqliteReader".to_string()]
        );
        assert_eq!(
            defined_names("rust-analyzer cargo demo 0.1.0 b/Bindings#snapshot()."),
            vec!["snapshot".to_string()]
        );
    }

    #[test]
    fn a_symbol_defined_twice_is_the_definition_its_caller_sees() {
        let answers = [
            "crates/t/tests/tools.rs::answer".to_string(),
            "crates/t/src/lib.rs::answer".to_string(),
        ];
        assert_eq!(
            visible_definition("crates/t/src/lib.rs::call", &answers),
            Some(&answers[1])
        );
        assert_eq!(
            visible_definition("crates/t/src/review.rs::Review.changes", &answers),
            Some(&answers[1])
        );
        assert_eq!(
            visible_definition("crates/t/tests/tools.rs::declines", &answers),
            Some(&answers[0])
        );
        let visits = [
            "src/m.rs::bindings.visit".to_string(),
            "src/m.rs::deref_targets.visit".to_string(),
        ];
        assert_eq!(
            visible_definition("src/m.rs::deref_targets", &visits),
            Some(&visits[1])
        );
        assert_eq!(
            visible_definition("src/m.rs::deref_targets.visit", &visits),
            Some(&visits[1])
        );
        assert_eq!(visible_definition("src/m.rs::other", &visits), None);
    }

    #[test]
    fn a_call_keeps_its_own_target_when_a_test_crate_defines_the_same_symbol() {
        use ::scip::types::{Document, Occurrence};

        let root = std::env::temp_dir().join(format!("dagayn-scip-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let lib = "fn call() {\n    answer();\n}\nfn answer() {}\n";
        let tools = "fn answer() {}\n";
        let mut store = GraphStore::open(root.join("graph.db")).expect("open graph store");
        for (file, text) in [("src/lib.rs", lib), ("tests/tools.rs", tools)] {
            std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            std::fs::write(root.join(file), text).unwrap();
            let hash = Sha256::digest(text.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let node = |kind: &str, name: &str, start: i64, end: i64| NodeInput {
                kind: kind.to_string(),
                name: name.to_string(),
                file_path: file.to_string(),
                line_start: start,
                line_end: end,
                language: "rust".to_string(),
                parent_name: None,
                params: None,
                return_type: None,
                modifiers: None,
                is_test: false,
                extra: json!({}),
            };
            let (nodes, edges) = if file == "src/lib.rs" {
                let call = EdgeInput {
                    kind: "CALLS".to_string(),
                    source: "src/lib.rs::call".to_string(),
                    target: "src/lib.rs::answer".to_string(),
                    file_path: file.to_string(),
                    line: 2,
                    extra: json!({}),
                };
                (
                    vec![
                        node("File", file, 1, 4),
                        node("Function", "call", 1, 3),
                        node("Function", "answer", 4, 4),
                    ],
                    vec![call],
                )
            } else {
                (
                    vec![node("File", file, 1, 1), node("Function", "answer", 1, 1)],
                    Vec::new(),
                )
            };
            store
                .store_file_nodes_edges(file, &nodes, &edges, &hash, 0)
                .expect("store file");
        }
        // rust-analyzer names both crate-root `answer`s of one package alike.
        let symbol = "rust-analyzer cargo demo 0.1.0 answer().";
        let occurrence = |line: i32, start: i32, roles: i32| Occurrence {
            range: vec![line, start, start + 6],
            symbol: symbol.to_string(),
            symbol_roles: roles,
            ..Occurrence::default()
        };
        let document = |path: &str, occurrences: Vec<Occurrence>| Document {
            relative_path: path.to_string(),
            occurrences,
            position_encoding: ::scip::types::PositionEncoding::UTF8CodeUnitOffsetFromLineStart
                .into(),
            ..Document::default()
        };
        let index = Index {
            documents: vec![
                document("tests/tools.rs", vec![occurrence(0, 3, ROLE_DEFINITION)]),
                document(
                    "src/lib.rs",
                    vec![occurrence(1, 4, 0), occurrence(3, 3, ROLE_DEFINITION)],
                ),
            ],
            ..Index::default()
        };
        let index_path = root.join("index.scip");
        std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

        store
            .apply_scip_overlays(
                &[ScipIndexInput {
                    path: index_path,
                    prefix: String::new(),
                    authoritative: true,
                }],
                &root,
            )
            .expect("apply overlay");
        let edges = store.get_edges_by_source("src/lib.rs::call").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_qualified, "src/lib.rs::answer");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn called_names_and_their_columns() {
        let external = json!({"external": true, "external_package": "vscode"});
        assert_eq!(
            called_name("vscode::window.createOutputChannel", &external),
            "createOutputChannel"
        );
        assert_eq!(called_name("src/a.rs::Store.save", &json!({})), "save");
        assert_eq!(
            called_name("std", &json!({"external_symbol": "format!"})),
            "format"
        );
        assert_eq!(
            called_name("std", &json!({"external_symbol": "Vec::<u8>::new"})),
            "new"
        );
        // `é` is two UTF-8 bytes and one UTF-16 unit.
        let line = "é.save(); resave(); save()";
        assert_eq!(
            name_columns(line, "save", Encoding::Utf8, false),
            vec![3, 21]
        );
        assert_eq!(
            name_columns(line, "save", Encoding::Utf16, false),
            vec![2, 20]
        );
        assert_eq!(name_columns(line, "save", Encoding::Utf16, true), vec![2]);
        assert_eq!(
            name_columns("    .map(|x| x)", "map", Encoding::Utf8, true),
            vec![5]
        );
        assert_eq!(
            name_columns("map.into_iter()", "map", Encoding::Utf8, true),
            Vec::<usize>::new()
        );
    }
}
