//! Rust members of a type declared in another file.
//!
//! `impl GraphStore { fn f() {} }` in `analysis.rs` yields the member
//! `analysis.rs::GraphStore.f`, but `GraphStore` is declared in `lib.rs`. The
//! extractor sees one file, so it contains the member in its File node
//! (`contain_in_file`) and starts `IMPLEMENTS` at `analysis.rs::GraphStore`,
//! which is no node. This pass finds the type's node and moves both edges'
//! source to it, recording the source they had in `extra.impl_owner_from`.
//! A moved edge whose type node is gone gets that source back, so a later run
//! can link it again.

use crate::helpers::*;
use crate::*;

use super::bare_names::{import_targets_tx, resolve_via_imports, symbol_visibility};

const MOVED_FROM: &str = "impl_owner_from";

impl GraphStore {
    /// Links Rust `impl` members and `IMPLEMENTS` edges to the type's node
    /// when it is declared in another file (or in another module of the same
    /// file). Returns how many edges were moved.
    pub fn link_foreign_impl_members(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        restore_orphaned_edges(&tx)?;
        let import_targets = import_targets_tx(&tx)?;
        let visibility = symbol_visibility(&tx)?;
        let types = rust_type_index(&tx)?;
        let mut moved = 0_i64;
        for (file, owner) in foreign_owners(&tx)? {
            // `serde.PythonVersion`: an impl inside `mod serde` names the
            // type by its own name.
            let name = owner.rsplit('.').next().unwrap_or(&owner);
            let Some(candidates) = types.get(name) else {
                continue;
            };
            let in_file: Vec<&String> = candidates
                .iter()
                .filter(|qualified| file_of(qualified) == file)
                .collect();
            let owner_node = match in_file.as_slice() {
                [only] => Some((*only).clone()),
                [] => resolve_via_imports(candidates, &file, &import_targets, &visibility)
                    .map(|(qualified, _, _)| qualified)
                    .or_else(|| only_in_named_module(candidates, &owner))
                    .or_else(|| only_in_package(candidates, &file)),
                _ => None,
            };
            if let Some(owner_node) = owner_node {
                moved += move_owner_edges(&tx, &file, &owner, &owner_node)?;
            }
        }
        tx.commit()?;
        Ok(moved)
    }
}

/// The one candidate in the module the owner names: `onepass.DFA`, from
/// `impl Remappable for onepass::DFA` or an impl inside `mod onepass`, is
/// the `DFA` of `onepass.rs` or `onepass/mod.rs`.
fn only_in_named_module(candidates: &[String], owner: &str) -> Option<String> {
    let (path, _) = owner.rsplit_once('.')?;
    let module = path.rsplit('.').next()?;
    let in_module = |qualified: &&String| {
        let file = file_of(qualified);
        let stem = file
            .strip_suffix("/mod.rs")
            .or_else(|| file.strip_suffix(".rs"))
            .unwrap_or(file);
        stem.rsplit('/').next() == Some(module)
    };
    let mut matching = candidates.iter().filter(in_module);
    let only = matching.next()?;
    matching.next().is_none().then(|| only.clone())
}

/// The one candidate under the same source directory of the same package,
/// when imports do not tell (`use super::Lite` from a `tests/` module, which
/// no module tree reaches).
fn only_in_package(candidates: &[String], file: &str) -> Option<String> {
    let root = source_root_of(file)?;
    let mut in_root = candidates
        .iter()
        .filter(|qualified| source_root_of(file_of(qualified)) == Some(root));
    let only = in_root.next()?;
    in_root.next().is_none().then(|| only.clone())
}

/// A Rust file's package directory with its first `src`, `tests`,
/// `benches`, or `examples` directory: `crates/x/src`, `tests`. A file under
/// none of them has no root.
fn source_root_of(file: &str) -> Option<&str> {
    let mut offset = 0_usize;
    for component in file.split('/') {
        offset += component.len();
        if matches!(component, "src" | "tests" | "benches" | "examples") {
            return Some(&file[..offset]);
        }
        offset += 1;
    }
    None
}

fn file_of(qualified: &str) -> &str {
    qualified
        .split_once("::")
        .map_or(qualified, |(file, _)| file)
}

/// Rust types by name: structs, enums, and traits (`Class` nodes other than
/// modules), and type aliases outside `impl` and `trait` blocks (`Type`
/// nodes at the top level or in a module; associated types such as
/// `type Value = ...` are left out).
fn rust_type_index(tx: &Transaction<'_>) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = tx.prepare(
        "SELECT n.name, n.qualified_name FROM nodes n \
         WHERE n.language = 'rust' AND ( \
             (n.kind = 'Class' \
              AND COALESCE(json_extract(n.extra, '$.type_role'), '') <> 'module') \
             OR (n.kind = 'Type' AND (n.parent_name IS NULL OR EXISTS ( \
                 SELECT 1 FROM nodes m \
                 WHERE m.qualified_name = n.file_path || '::' || n.parent_name \
                   AND json_extract(m.extra, '$.type_role') = 'module'))))",
    )?;
    let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
    let mut index = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let (name, qualified) = row?;
        index.entry(name).or_default().push(qualified);
    }
    Ok(index)
}

/// (file, owner) of members and `IMPLEMENTS` edges whose owner `file::owner`
/// is no node.
fn foreign_owners(tx: &Transaction<'_>) -> Result<Vec<(String, String)>> {
    let mut stmt = tx.prepare(
        "SELECT DISTINCT n.file_path, n.parent_name FROM nodes n \
         WHERE n.language = 'rust' AND n.kind IN ('Function', 'Test') \
           AND n.parent_name IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM nodes p \
                           WHERE p.qualified_name = n.file_path || '::' || n.parent_name) \
         UNION \
         SELECT DISTINCT e.file_path, substr(e.source_qualified, length(e.file_path) + 3) \
         FROM edges e \
         WHERE e.kind = 'IMPLEMENTS' AND e.file_path LIKE '%.rs' \
           AND substr(e.source_qualified, 1, length(e.file_path) + 2) = e.file_path || '::' \
           AND NOT EXISTS (SELECT 1 FROM nodes p WHERE p.qualified_name = e.source_qualified) \
         ORDER BY 1, 2",
    )?;
    let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Moves the File's CONTAINS edges to the owner's members, and the owner's
/// `IMPLEMENTS` edges, to `owner_node`. An `impl` the extractor found to be
/// for a type parameter or a type of another crate (`impl_target`) stays.
fn move_owner_edges(
    tx: &Transaction<'_>,
    file: &str,
    owner: &str,
    owner_node: &str,
) -> Result<i64> {
    let from = format!("{file}::{owner}");
    let changed = tx.execute(
        "UPDATE edges SET source_qualified = ?1, \
             extra = json_set(COALESCE(NULLIF(extra, ''), '{}'), '$.impl_owner_from', \
                              source_qualified) \
         WHERE file_path = ?2 AND ( \
             (kind = 'CONTAINS' AND source_qualified = ?2 \
              AND target_qualified IN ( \
                  SELECT qualified_name FROM nodes \
                  WHERE file_path = ?2 AND parent_name = ?3 \
                    AND json_extract(extra, '$.impl_target') IS NULL)) \
             OR (kind = 'IMPLEMENTS' AND source_qualified = ?4 \
                 AND json_extract(extra, '$.impl_target') IS NULL))",
        params![owner_node, file, owner, from],
    )?;
    Ok(changed as i64)
}

/// Gives a moved edge whose owner node no longer exists the source it had.
fn restore_orphaned_edges(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        &format!(
            "UPDATE edges SET source_qualified = json_extract(extra, '$.{MOVED_FROM}'), \
                 extra = json_remove(extra, '$.{MOVED_FROM}') \
             WHERE json_extract(extra, '$.{MOVED_FROM}') IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM nodes \
                               WHERE qualified_name = edges.source_qualified)"
        ),
        [],
    )?;
    Ok(())
}
