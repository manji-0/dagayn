//! Symbols named through a module that only re-exports them.
//!
//! `from dagayn.parser import NodeInfo` resolves to
//! `dagayn/parser/__init__.py::NodeInfo`, but `__init__.py` declares no
//! `NodeInfo`: it imports it (`from .types import NodeInfo`) for its
//! importers. The extractor sees one file, so the edge names the package;
//! this pass follows the package's own imports to the declaration.

use crate::helpers::*;
use crate::*;

use super::bare_names::DIRECT_IMPORT_CONFIDENCE;

/// How many re-exporting modules a name is followed through.
const MAX_HOPS: usize = 8;

/// What a module's imports bring in: bound name -> (module file, name
/// there), and the modules it star-imports (`from .x import *`, Rust `use
/// x::*`).
#[derive(Default)]
struct ModuleImports {
    names: HashMap<String, (String, String)>,
    stars: Vec<String>,
}

fn module_imports(
    tx: &Transaction<'_>,
    files: &HashSet<String>,
) -> Result<HashMap<String, ModuleImports>> {
    let mut stmt = tx.prepare(
        "SELECT file_path, target_qualified, json_extract(extra, '$.names'), \
                COALESCE(json_extract(extra, '$.glob'), 0) \
         FROM edges WHERE kind = 'IMPORTS_FROM' \
           AND (json_extract(extra, '$.names') IS NOT NULL \
                OR COALESCE(json_extract(extra, '$.glob'), 0) = 1)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)? != 0,
        ))
    })?;
    let mut imports: HashMap<String, ModuleImports> = HashMap::new();
    for row in rows {
        let (file, target, names, glob) = row?;
        if !files.contains(&target) {
            continue;
        }
        let names: Vec<(String, String)> = names
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let entry = imports.entry(file).or_default();
        // Python writes a star import as an import that binds no name.
        if glob || names.is_empty() {
            entry.stars.push(target);
            continue;
        }
        for (name, alias) in names {
            entry.names.insert(alias, (target.clone(), name));
        }
    }
    Ok(imports)
}

/// The declaration `file::symbol` stands for when `file` only imports it:
/// `symbol`'s first segment is followed through the file's imports (named,
/// then star) until a node has the name.
fn follow(
    file: &str,
    symbol: &str,
    imports: &HashMap<String, ModuleImports>,
    nodes: &HashSet<String>,
    hops: usize,
) -> Option<String> {
    if hops == MAX_HOPS {
        return None;
    }
    // A module `__getattr__` (PEP 562) that defines what it returns:
    // `def __getattr__(name): ... class GraphStore: ...`.
    let lazy = format!("{file}::__getattr__.{symbol}");
    if nodes.contains(&lazy) {
        return Some(lazy);
    }
    let module = imports.get(file)?;
    let (head, rest) = match symbol.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (symbol, None),
    };
    if let Some((origin, name)) = module.names.get(head) {
        let symbol = match rest {
            Some(rest) => format!("{name}.{rest}"),
            None => name.clone(),
        };
        let qualified = format!("{origin}::{symbol}");
        return if nodes.contains(&qualified) {
            Some(qualified)
        } else {
            follow(origin, &symbol, imports, nodes, hops + 1)
        };
    }
    module.stars.iter().find_map(|origin| {
        let qualified = format!("{origin}::{symbol}");
        if nodes.contains(&qualified) {
            Some(qualified)
        } else {
            follow(origin, symbol, imports, nodes, hops + 1)
        }
    })
}

/// Rewrites `CALLS` / `REFERENCES` / `INHERITS` / `IMPLEMENTS` targets that
/// name a symbol of a file that declares none (`pkg/__init__.py::NodeInfo`)
/// to the declaration the file's imports lead to. Returns how many changed.
pub(crate) fn resolve_reexported_targets(tx: &Transaction<'_>) -> Result<i64> {
    let (nodes, files) = {
        let mut stmt = tx.prepare("SELECT qualified_name, kind FROM nodes")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut nodes = HashSet::new();
        let mut files = HashSet::new();
        for row in rows {
            let (qualified, kind) = row?;
            if kind == "File" {
                files.insert(qualified.clone());
            }
            nodes.insert(qualified);
        }
        (nodes, files)
    };
    let imports = module_imports(tx, &files)?;
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT e.id, e.target_qualified FROM edges e \
             WHERE e.kind IN ('CALLS', 'REFERENCES', 'INHERITS', 'IMPLEMENTS') \
               AND e.target_qualified LIKE '%::%' \
               AND NOT EXISTS (SELECT 1 FROM nodes n \
                               WHERE n.qualified_name = e.target_qualified)",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut resolved = 0_i64;
    for (edge_id, target) in edges {
        let Some((file, symbol)) = target.split_once("::") else {
            continue;
        };
        if !files.contains(file) {
            continue;
        }
        let Some(qualified) = follow(file, symbol, &imports, &nodes, 0) else {
            continue;
        };
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                qualified,
                edge_target_name(&qualified),
                DIRECT_IMPORT_CONFIDENCE,
                ConfidenceTier::High.as_str(),
                edge_id
            ],
        )?;
        resolved += 1;
    }
    Ok(resolved)
}
