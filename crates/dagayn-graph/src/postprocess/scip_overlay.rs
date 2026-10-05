//! Call targets from a SCIP index (`docs/plans/SCIP-CALL-RESOLUTION.md`).
//!
//! A compiler-grade indexer (`rust-analyzer scip`, `scip-typescript`) says
//! which definition each reference names. A `CALLS` edge is matched to the
//! reference at its called name's position: the edge's line, or one of the
//! next eight for a chain the extractor records at the expression's first
//! line. Its answer then settles the edge:
//!
//! - a definition of the repository: the node whose range holds it, `HIGH`;
//!   an edge already at that node (or at a node containing it, a class for
//!   its constructor) is confirmed;
//! - a symbol defined elsewhere: its package, `external`, `HIGH`; an edge
//!   already external is confirmed and keeps its package name;
//! - a local (a closure, a callable parameter): an unresolved edge records
//!   `callee_local`; other edges are left;
//! - no reference, or several: the edge is left as resolution left it.
//!
//! Every edge the index settles records `resolved_by: "scip"`. Files whose
//! content no longer matches what the graph parsed are skipped.

use std::path::Path;

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

#[derive(Default)]
struct ScipIndex {
    documents: HashMap<String, Encoding>,
    references: HashMap<(String, i64), Vec<Reference>>,
    definitions: HashMap<String, (String, i64)>,
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
        let file = format!("{prefix}{}", document.relative_path);
        let encoding = match document.position_encoding.value() {
            1 => Encoding::Utf8,
            3 => Encoding::Utf32,
            // scip-typescript leaves it unspecified and counts UTF-16 units.
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
                read.definitions
                    .entry(occurrence.symbol.clone())
                    .or_insert_with(|| (file.clone(), line));
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
    /// Settles `CALLS` edges by the SCIP index at `index_path` (see the
    /// module docs). `prefix` turns its document paths into the graph's
    /// (`dagayn-vscode/` for an index made in that directory); `repo_root`
    /// holds the files the graph parsed. An `authoritative` index moves any
    /// edge it disagrees with; otherwise it only settles edges resolution
    /// left unresolved and confirms the ones it agrees with.
    pub fn apply_scip_overlay(
        &mut self,
        index_path: &Path,
        prefix: &str,
        repo_root: &Path,
        authoritative: bool,
    ) -> Result<ScipOverlayStats> {
        let index = read_index(index_path, prefix)?;
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
                "SELECT id, target_qualified, file_path, line, confidence_tier, extra \
                 FROM edges WHERE kind = 'CALLS' ORDER BY file_path, line",
            )?;
            let rows = stmt.query_map([], |row| {
                <(i64, String, String, i64, Option<String>, Option<String>)>::try_from(row)
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        // File -> its lines, or `None` when it changed since the graph
        // parsed it.
        let mut sources: HashMap<String, Option<Vec<String>>> = HashMap::new();
        for (id, target, file, line, tier, extra) in edges {
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
            if let Some((definition_file, definition_line)) = index.definitions.get(symbol) {
                let holds_definition = at_node.is_some_and(|(node_file, start, end)| {
                    node_file == definition_file
                        && *start <= *definition_line
                        && *definition_line <= *end
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
                let Some(node) = node_at(definition_file, *definition_line, &defined_names(symbol))
                else {
                    stats.left += 1;
                    continue;
                };
                settle(&mut extra);
                if let Some(map) = extra.as_object_mut() {
                    for key in ["external", "external_package", "external_symbol", "stdlib"] {
                        map.remove(key);
                    }
                }
                write_edge(&tx, id, &node, &extra, Some(ConfidenceTier::High))?;
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
