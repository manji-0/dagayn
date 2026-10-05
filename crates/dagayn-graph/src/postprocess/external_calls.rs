//! Calls into an external package that only the whole graph gives away.
//!
//! - Method calls on a receiver of unknown type whose name only the
//!   standard library defines. `names.iter()`, `line.strip()`,
//!   `items.map(...)`: the extractor cannot type the receiver, so the call
//!   stays a bare `receiver_unknown` name that resolution rightly leaves
//!   alone. When no function of that name is visible to the calling file
//!   (declared there or in a file it imports), the only method it can call
//!   is the standard library's: a value of a repository class declared in a
//!   file the caller never imports seldom reaches it.
//! - Rust names a glob brings in from a module of the repository that
//!   imports them from a crate: `mod tests` in `core_tests/x.rs` with `use
//!   super::*;` calls `HashMap::new()` or `node.kind()` (`Node` of
//!   `tree_sitter`) that the parent module imported.
//!
//! Both point the edge at the package, `MEDIUM`, like any external call
//! inferred rather than written.

use crate::helpers::*;
use crate::*;

use super::bare_names::{DIRECT_IMPORT_CONFIDENCE, INFERRED_CONFIDENCE, language_family};
use super::returned::ResolveContext;

/// Methods of Rust's standard types and traits (`Iterator`, `Option`,
/// `Result`, `Vec`, slices, `str`, `String`, maps, `Path`).
const RUST_STD_METHODS: &[&str] = &[
    "abs",
    "all",
    "and_then",
    "any",
    "as_bytes",
    "as_deref",
    "as_mut",
    "as_mut_ptr",
    "as_nanos",
    "as_os_str",
    "as_ptr",
    "as_ref",
    "as_secs_f64",
    "as_slice",
    "as_str",
    "binary_search",
    "borrow",
    "borrow_mut",
    "bytes",
    "ceil",
    "chain",
    "char_indices",
    "chars",
    "checked_add",
    "checked_sub",
    "chunks",
    "chunks_exact",
    "clamp",
    "clear",
    "clone",
    "cloned",
    "cmp",
    "collect",
    "concat",
    "contains",
    "contains_key",
    "copied",
    "count",
    "dedup",
    "drain",
    "ends_with",
    "entry",
    "enumerate",
    "eq_ignore_ascii_case",
    "expect",
    "extend",
    "extension",
    "file_name",
    "fill",
    "filter",
    "filter_map",
    "find",
    "find_map",
    "first",
    "flat_map",
    "flatten",
    "fold",
    "for_each",
    "get",
    "get_mut",
    "get_or_insert",
    "get_or_insert_with",
    "get_unchecked",
    "insert",
    "into_boxed_slice",
    "into_iter",
    "is_alphanumeric",
    "is_ascii_alphanumeric",
    "is_ascii_digit",
    "is_ascii_lowercase",
    "is_ascii_uppercase",
    "is_ascii_whitespace",
    "is_dir",
    "is_empty",
    "is_err",
    "is_file",
    "is_none",
    "is_ok",
    "is_some",
    "is_some_and",
    "is_whitespace",
    "iter",
    "iter_mut",
    "join",
    "keys",
    "last",
    "len",
    "lines",
    "lock",
    "map",
    "map_err",
    "map_or",
    "max",
    "max_by_key",
    "min",
    "min_by_key",
    "next",
    "nth",
    "ok",
    "ok_or",
    "ok_or_else",
    "or_default",
    "or_insert",
    "or_insert_with",
    "parent",
    "parse",
    "partial_cmp",
    "peekable",
    "pop",
    "position",
    "push",
    "push_str",
    "remove",
    "repeat",
    "replace",
    "retain",
    "rev",
    "rfind",
    "round",
    "rsplit",
    "rsplit_once",
    "saturating_sub",
    "skip",
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "sort_unstable_by",
    "split",
    "split_at",
    "split_first",
    "split_inclusive",
    "split_once",
    "split_whitespace",
    "splitn",
    "sqrt",
    "starts_with",
    "strip_prefix",
    "strip_suffix",
    "sum",
    "take",
    "then",
    "then_some",
    "then_with",
    "to_ascii_lowercase",
    "to_ascii_uppercase",
    "to_lowercase",
    "to_owned",
    "to_path_buf",
    "to_string",
    "to_string_lossy",
    "to_uppercase",
    "to_vec",
    "total_cmp",
    "trim",
    "trim_end",
    "trim_end_matches",
    "trim_start",
    "truncate",
    "unwrap",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "values",
    "windows",
    "zip",
];

/// Methods of Python's builtin types (`str`, `list`, `dict`, `set`,
/// `bytes`).
const PYTHON_BUILTIN_METHODS: &[&str] = &[
    "add",
    "append",
    "capitalize",
    "casefold",
    "center",
    "clear",
    "copy",
    "count",
    "decode",
    "difference",
    "discard",
    "encode",
    "endswith",
    "expandtabs",
    "extend",
    "find",
    "format",
    "fromkeys",
    "get",
    "index",
    "insert",
    "intersection",
    "isalnum",
    "isalpha",
    "isdigit",
    "isidentifier",
    "islower",
    "isspace",
    "issubset",
    "issuperset",
    "isupper",
    "items",
    "join",
    "keys",
    "ljust",
    "lower",
    "lstrip",
    "partition",
    "pop",
    "popitem",
    "remove",
    "removeprefix",
    "removesuffix",
    "replace",
    "reverse",
    "rfind",
    "rindex",
    "rjust",
    "rpartition",
    "rsplit",
    "rstrip",
    "setdefault",
    "sort",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "swapcase",
    "symmetric_difference",
    "title",
    "union",
    "update",
    "upper",
    "values",
    "zfill",
];

/// Methods of `pathlib.Path` no builtin type has.
const PYTHON_PATHLIB_METHODS: &[&str] = &[
    "as_posix",
    "glob",
    "is_dir",
    "is_file",
    "is_symlink",
    "iterdir",
    "mkdir",
    "read_bytes",
    "read_text",
    "relative_to",
    "rglob",
    "rmdir",
    "touch",
    "unlink",
    "with_name",
    "with_stem",
    "with_suffix",
    "write_bytes",
    "write_text",
];

/// Methods of JavaScript's builtin objects (`Array`, `String`, `Promise`,
/// `Map`, `Set`, `Number`).
const JAVASCRIPT_BUILTIN_METHODS: &[&str] = &[
    "at",
    "catch",
    "charAt",
    "charCodeAt",
    "concat",
    "endsWith",
    "entries",
    "every",
    "fill",
    "filter",
    "finally",
    "find",
    "findIndex",
    "findLast",
    "flat",
    "flatMap",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "lastIndexOf",
    "localeCompare",
    "map",
    "match",
    "matchAll",
    "padEnd",
    "padStart",
    "pop",
    "push",
    "reduce",
    "reduceRight",
    "repeat",
    "replace",
    "replaceAll",
    "reverse",
    "shift",
    "slice",
    "some",
    "sort",
    "splice",
    "split",
    "startsWith",
    "substring",
    "then",
    "toFixed",
    "toLowerCase",
    "toString",
    "toUpperCase",
    "trim",
    "trimEnd",
    "trimStart",
    "unshift",
];

/// The standard-library package that defines `method` in `language`.
fn std_method_package(language: &str, method: &str) -> Option<&'static str> {
    match language {
        "rust" => RUST_STD_METHODS.contains(&method).then_some("std"),
        "python" if PYTHON_BUILTIN_METHODS.contains(&method) => Some("builtins"),
        "python" => PYTHON_PATHLIB_METHODS
            .contains(&method)
            .then_some("pathlib"),
        "javascript" => JAVASCRIPT_BUILTIN_METHODS
            .contains(&method)
            .then_some("globalThis"),
        _ => None,
    }
}

/// Points `receiver_unknown` calls whose method only the standard library
/// defines, as far as the calling file can see (no function of that name in
/// the file or in a file it imports), at the package, `MEDIUM`. Returns how
/// many changed.
pub(crate) fn mark_stdlib_method_calls(
    tx: &Transaction<'_>,
    context: &ResolveContext,
) -> Result<i64> {
    // The files declaring each function name of the repository, per
    // language family.
    let defined = context.defined();
    let import_targets = context.import_targets();
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, target_qualified, file_path, extra FROM edges \
             WHERE kind = 'CALLS' AND target_qualified NOT LIKE '%::%' \
               AND COALESCE(json_extract(extra, '$.receiver_unknown'), 0) = 1 \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0",
        )?;
        let rows = stmt.query_map([], |row| <(i64, String, String, String)>::try_from(row))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut marked = 0_i64;
    for (edge_id, method, file_path, extra) in edges {
        let Some(family) = language_family(&file_path) else {
            continue;
        };
        let Some(package) = std_method_package(family, &method) else {
            continue;
        };
        // A same-named function the calling file can see (its own, or one
        // of a file it imports or reaches through re-exports) may be the one
        // called; one of a file it cannot see is not.
        let visible = defined.get(&(family, method.clone())).is_some_and(|files| {
            files.iter().any(|declaring| {
                *declaring == file_path
                    || import_targets
                        .get(&file_path)
                        .is_some_and(|targets| targets.contains(declaring))
            })
        });
        if visible {
            continue;
        }
        let mut extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        if !extra.is_object() {
            extra = json!({});
        }
        extra["external"] = json!(true);
        extra["external_package"] = json!(package);
        extra["external_symbol"] = json!(method);
        extra["stdlib"] = json!(true);
        extra["confidence"] = json!(INFERRED_CONFIDENCE);
        extra["confidence_tier"] = json!("MEDIUM");
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                package,
                package,
                serde_json::to_string(&extra)?,
                INFERRED_CONFIDENCE,
                ConfidenceTier::Medium.as_str(),
                edge_id
            ],
        )?;
        marked += 1;
    }
    Ok(marked)
}

/// Module file -> local name -> (package, standard library, path), from the
/// `use` edges of external crates (`paths`).
type CrateImports = HashMap<String, HashMap<String, (String, bool, String)>>;

fn crate_imports(tx: &Transaction<'_>) -> Result<CrateImports> {
    let mut stmt = tx.prepare(
        "SELECT file_path, json_extract(extra, '$.external_package'), \
                COALESCE(json_extract(extra, '$.stdlib'), 0), json_extract(extra, '$.paths') \
         FROM edges WHERE kind = 'IMPORTS_FROM' \
           AND COALESCE(json_extract(extra, '$.external'), 0) = 1 \
           AND json_extract(extra, '$.paths') IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut imports = CrateImports::new();
    for row in rows {
        let (file, package, stdlib, paths) = row?;
        let paths: Vec<String> = serde_json::from_str(&paths).unwrap_or_default();
        let names = imports.entry(file).or_default();
        for path in paths {
            let Some(name) = path.rsplit("::").next().filter(|name| *name != "*") else {
                continue;
            };
            names.insert(name.to_string(), (package.clone(), stdlib, path.clone()));
        }
    }
    Ok(imports)
}

/// File -> the module files it glob-imports (`use super::*`), transitively.
fn glob_imports(tx: &Transaction<'_>) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = tx.prepare(
        "SELECT DISTINCT file_path, target_qualified FROM edges \
         WHERE kind = 'IMPORTS_FROM' AND COALESCE(json_extract(extra, '$.glob'), 0) = 1",
    )?;
    let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
    let mut direct: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (file, module) = row?;
        direct.entry(file).or_default().push(module);
    }
    let mut globs = HashMap::new();
    for file in direct.keys() {
        let mut seen: Vec<String> = Vec::new();
        let mut pending = direct[file].clone();
        while let Some(module) = pending.pop() {
            if module == *file || seen.contains(&module) {
                continue;
            }
            pending.extend(direct.get(&module).cloned().unwrap_or_default());
            seen.push(module);
        }
        globs.insert(file.clone(), seen);
    }
    Ok(globs)
}

/// Points Rust calls through a name that a glob-imported module of the
/// repository imports from a crate (`HashMap::new()`, `node.kind()` with
/// `receiver_type: "Node"`, `json!`) at that crate, `MEDIUM`. Returns how
/// many changed.
pub(crate) fn mark_glob_imported_external_calls(tx: &Transaction<'_>) -> Result<i64> {
    let globs = glob_imports(tx)?;
    if globs.is_empty() {
        return Ok(0);
    }
    let imports = crate_imports(tx)?;
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, file_path, target_qualified, json_extract(extra, '$.receiver_type'), \
                    extra FROM edges \
             WHERE kind = 'CALLS' AND file_path LIKE '%.rs' \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0 \
               AND NOT EXISTS (SELECT 1 FROM nodes n \
                               WHERE n.qualified_name = edges.target_qualified)",
        )?;
        let rows = stmt.query_map([], |row| {
            <(i64, String, String, Option<String>, String)>::try_from(row)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut marked = 0_i64;
    for (edge_id, file, target, receiver_type, extra) in edges {
        let Some(modules) = globs.get(&file) else {
            continue;
        };
        // The name the call is written through: the receiver's type, or the
        // path's first segment (`HashMap::new`, `json!`, `take`).
        let (name, symbol) = match &receiver_type {
            Some(type_name) => (type_name.clone(), format!("{type_name}::{target}")),
            None if target.contains('/') => continue,
            None => {
                let first = target.split("::").next().unwrap_or(&target);
                (first.trim_end_matches('!').to_string(), target.clone())
            }
        };
        let Some((package, stdlib, _)) = modules
            .iter()
            .find_map(|module| imports.get(module).and_then(|names| names.get(&name)))
        else {
            continue;
        };
        let mut extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        if !extra.is_object() {
            extra = json!({});
        }
        extra["external"] = json!(true);
        extra["external_package"] = json!(package);
        extra["external_symbol"] = json!(symbol);
        if *stdlib {
            extra["stdlib"] = json!(true);
        }
        extra["confidence"] = json!(INFERRED_CONFIDENCE);
        extra["confidence_tier"] = json!("MEDIUM");
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                package,
                package,
                serde_json::to_string(&extra)?,
                INFERRED_CONFIDENCE,
                ConfidenceTier::Medium.as_str(),
                edge_id
            ],
        )?;
        marked += 1;
    }
    Ok(marked)
}

/// Standard types a repository type may dereference to (`Deref<Target =
/// str>`): their methods are the standard library's.
const RUST_STD_DEREF_TARGETS: &[&str] = &[
    "str", "String", "slice", "Vec", "Path", "PathBuf", "OsStr", "OsString", "HashMap", "HashSet",
    "BTreeMap", "BTreeSet", "VecDeque", "Box", "Rc", "Arc", "RefCell", "Cell", "Mutex", "RwLock",
    "Option", "Result", "CStr", "CString",
];

/// Methods of `Result` / `Option`. The extractor types `Store::open(p)` as
/// a `Store`, but a constructor that can fail returns a `Result`, so
/// `Store::open(p).expect(..)` is the `Result`'s `expect`.
const RUST_RESULT_OPTION_METHODS: &[&str] = &[
    "and_then",
    "as_deref",
    "as_mut",
    "as_ref",
    "expect",
    "expect_err",
    "is_err",
    "is_none",
    "is_ok",
    "is_some",
    "map_err",
    "map_or",
    "map_or_else",
    "ok",
    "ok_or",
    "ok_or_else",
    "or_else",
    "unwrap",
    "unwrap_err",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
];

/// Points Rust calls on a repository type that the type itself does not
/// have at `std`, `MEDIUM`: a method of the standard type it dereferences
/// to (`impl Deref for FilePath { type Target = str; }`,
/// `file_path.as_str()`), or of the `Result` / `Option` a fallible
/// constructor returns (`GraphStore::open(p).expect(..)`). Returns how many
/// changed.
pub(crate) fn mark_deref_calls(tx: &Transaction<'_>) -> Result<i64> {
    let targets: HashMap<String, String> = {
        let mut stmt = tx.prepare(
            "SELECT name, json_extract(extra, '$.deref_target') FROM nodes \
             WHERE kind = 'Class' AND file_path LIKE '%.rs' \
               AND json_extract(extra, '$.deref_target') IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, target_qualified, json_extract(extra, '$.receiver_type'), extra \
             FROM edges \
             WHERE kind = 'CALLS' AND file_path LIKE '%.rs' \
               AND target_qualified NOT LIKE '%::%' \
               AND json_extract(extra, '$.receiver_type') IS NOT NULL \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0",
        )?;
        let rows = stmt.query_map([], |row| <(i64, String, String, String)>::try_from(row))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut marked = 0_i64;
    for (edge_id, method, receiver_type, extra) in edges {
        let target = match targets.get(&receiver_type) {
            Some(target) if RUST_STD_DEREF_TARGETS.contains(&target.as_str()) => target.clone(),
            _ if RUST_RESULT_OPTION_METHODS.contains(&method.as_str()) => receiver_type.clone(),
            _ => continue,
        };
        let mut extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        if !extra.is_object() {
            extra = json!({});
        }
        extra["external"] = json!(true);
        extra["external_package"] = json!("std");
        extra["external_symbol"] = json!(format!("{target}::{method}"));
        extra["stdlib"] = json!(true);
        extra["confidence"] = json!(INFERRED_CONFIDENCE);
        extra["confidence_tier"] = json!("MEDIUM");
        tx.execute(
            "UPDATE edges SET target_qualified = 'std', target_name = 'std', extra = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                serde_json::to_string(&extra)?,
                INFERRED_CONFIDENCE,
                ConfidenceTier::Medium.as_str(),
                edge_id
            ],
        )?;
        marked += 1;
    }
    Ok(marked)
}

/// Calls of a method on a receiver of known type, by `(language family,
/// method)`: how many reach each package, and whether it is the standard
/// library. Calls a SCIP index settled (`resolved_by`) are not counted: the
/// index types nearly every call of the files it covers, and a method name
/// that one package's typed calls dominate there (`db.exec()` of
/// better-sqlite3) would take every untyped call of that name elsewhere
/// (`child_process.exec()`).
type Observations = HashMap<(&'static str, String), HashMap<String, (usize, bool)>>;

/// How many typed calls a method name needs before its unknown-receiver
/// calls are taken for a package's.
const MIN_OBSERVATIONS: usize = 2;

/// The share of those calls one package needs: `kind` is `tree_sitter`'s
/// 252 times and `std`'s (`io::Error::kind`) 12 times.
const MIN_SHARE: f64 = 0.9;

fn observed_methods(tx: &Transaction<'_>) -> Result<Observations> {
    let mut stmt = tx.prepare(
        "SELECT file_path, json_extract(extra, '$.external_package'), \
                COALESCE(json_extract(extra, '$.stdlib'), 0), \
                json_extract(extra, '$.external_symbol') FROM edges \
         WHERE kind = 'CALLS' AND COALESCE(json_extract(extra, '$.external'), 0) = 1 \
           AND json_extract(extra, '$.external_symbol') IS NOT NULL \
           AND COALESCE(json_extract(extra, '$.inferred_from'), '') = '' \
           AND json_extract(extra, '$.resolved_by') IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut observed = Observations::new();
    for row in rows {
        let (file, package, stdlib, symbol) = row?;
        let Some(family) = language_family(&file) else {
            continue;
        };
        // A method written on a type or value (`Node::kind`, `Path.exists`),
        // not a function of the package itself (`fmt.Println`).
        let symbol = symbol.trim_end_matches('!');
        let Some((owner, method)) = symbol.rsplit_once("::").or_else(|| symbol.rsplit_once('.'))
        else {
            continue;
        };
        // Only a receiver whose type is written: a value a chain returned
        // (`json!(..).into_pyobject()`) may be another package's.
        let owner_leaf = owner.rsplit(['.', ':']).next().unwrap_or(owner);
        let typed =
            !owner.ends_with(')') && owner_leaf.starts_with(|c: char| c.is_ascii_uppercase());
        if !typed || method.is_empty() {
            continue;
        }
        let entry = observed
            .entry((family, method.to_string()))
            .or_default()
            .entry(package)
            .or_insert((0, stdlib));
        entry.0 += 1;
    }
    Ok(observed)
}

/// The package most typed calls of a method reach, when enough of them do.
fn dominant_package(packages: &HashMap<String, (usize, bool)>) -> Option<(&String, bool)> {
    let total = packages.values().map(|(count, _)| count).sum::<usize>();
    let (package, (count, stdlib)) = packages.iter().max_by_key(|(_, (count, _))| *count)?;
    (*count >= MIN_OBSERVATIONS && *count as f64 >= MIN_SHARE * total as f64)
        .then_some((package, *stdlib))
}

/// Points `receiver_unknown` calls at the package nearly every typed call
/// of the same method name reaches (`child.kind()` in `for child in
/// node.children(..)` is `tree_sitter`'s when `Node::kind` is nearly every
/// `kind` the graph has seen), `MEDIUM`, unless a function of that name is
/// visible to the calling file. Returns how many changed.
pub(crate) fn mark_observed_method_calls(
    tx: &Transaction<'_>,
    context: &ResolveContext,
) -> Result<i64> {
    let observed = observed_methods(tx)?;
    if observed.is_empty() {
        return Ok(0);
    }
    let defined = context.defined();
    let import_targets = context.import_targets();
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, target_qualified, file_path, extra FROM edges \
             WHERE kind = 'CALLS' AND target_qualified NOT LIKE '%::%' \
               AND COALESCE(json_extract(extra, '$.receiver_unknown'), 0) = 1 \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0",
        )?;
        let rows = stmt.query_map([], |row| <(i64, String, String, String)>::try_from(row))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut marked = 0_i64;
    for (edge_id, method, file_path, extra) in edges {
        let Some(family) = language_family(&file_path) else {
            continue;
        };
        let Some((package, stdlib)) = observed
            .get(&(family, method.clone()))
            .and_then(dominant_package)
        else {
            continue;
        };
        let visible = defined.get(&(family, method.clone())).is_some_and(|files| {
            files.iter().any(|declaring| {
                *declaring == file_path
                    || import_targets
                        .get(&file_path)
                        .is_some_and(|targets| targets.contains(declaring))
            })
        });
        if visible {
            continue;
        }
        let package = package.clone();
        let mut extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        if !extra.is_object() {
            extra = json!({});
        }
        extra["external"] = json!(true);
        extra["external_package"] = json!(package);
        extra["external_symbol"] = json!(method);
        extra["inferred_from"] = json!("observed_method");
        if stdlib {
            extra["stdlib"] = json!(true);
        }
        extra["confidence"] = json!(INFERRED_CONFIDENCE);
        extra["confidence_tier"] = json!("MEDIUM");
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                package,
                package,
                serde_json::to_string(&extra)?,
                INFERRED_CONFIDENCE,
                ConfidenceTier::Medium.as_str(),
                edge_id
            ],
        )?;
        marked += 1;
    }
    Ok(marked)
}

/// Points Rust calls of an enum variant (`JvmReceiver::Local(x)`, which
/// the extractor leaves as `Local` with `receiver_type: "JvmReceiver"`) at
/// the enum the variant constructs, as `Store::new(..)` would be its
/// constructor: the one enum of that name declaring the variant, preferring
/// the caller's own file. `enum_variant` keeps the variant. Returns how many
/// changed.
pub(crate) fn resolve_enum_variant_calls(tx: &Transaction<'_>) -> Result<i64> {
    // Enum name -> (QN, file, variants).
    let mut enums: HashMap<String, Vec<(String, String, HashSet<String>)>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, name, file_path, json_extract(extra, '$.variants') \
             FROM nodes WHERE kind = 'Class' AND file_path LIKE '%.rs' \
               AND json_extract(extra, '$.variants') IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| <(String, String, String, String)>::try_from(row))?;
        for row in rows {
            let (qualified, name, file, variants) = row?;
            let variants: HashSet<String> = serde_json::from_str(&variants).unwrap_or_default();
            enums
                .entry(name)
                .or_default()
                .push((qualified, file, variants));
        }
    }
    if enums.is_empty() {
        return Ok(0);
    }
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, target_qualified, file_path, json_extract(extra, '$.receiver_type'), extra \
             FROM edges \
             WHERE kind = 'CALLS' AND file_path LIKE '%.rs' \
               AND target_qualified NOT LIKE '%::%' \
               AND json_extract(extra, '$.receiver_type') IS NOT NULL \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0",
        )?;
        let rows = stmt.query_map([], |row| {
            <(i64, String, String, String, String)>::try_from(row)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut resolved = 0_i64;
    for (edge_id, variant, file, receiver_type, extra) in edges {
        let Some(candidates) = enums.get(&receiver_type) else {
            continue;
        };
        let declaring = candidates
            .iter()
            .filter(|(_, _, variants)| variants.contains(&variant))
            .collect::<Vec<_>>();
        let target = declaring
            .iter()
            .find(|(_, declared_in, _)| *declared_in == file)
            .or(match declaring.as_slice() {
                [only] => Some(only),
                _ => None,
            });
        let Some((target, _, _)) = target else {
            continue;
        };
        let mut extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        extra["enum_variant"] = json!(variant);
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                target,
                edge_target_name(target),
                serde_json::to_string(&extra)?,
                DIRECT_IMPORT_CONFIDENCE,
                ConfidenceTier::High.as_str(),
                edge_id
            ],
        )?;
        resolved += 1;
    }
    Ok(resolved)
}
