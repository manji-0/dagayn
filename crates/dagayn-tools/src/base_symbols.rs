//! Base-side symbols: what a change removed or reshaped, and what outside
//! the change still points at it.
//!
//! The graph reflects the working tree, so a symbol the change deleted has no
//! node left to ask about. This module re-parses each changed file as it was
//! at `base` with the same in-process parser the build uses, compares it to
//! the file as it is now, and then asks the graph which nodes outside the
//! change still reference the symbols that went away or changed shape. That
//! is the evidence behind the `dangling_reference` and `unchanged_caller`
//! findings of `review_tool(mode="changes")`.
//!
//! # API (crate-internal)
//!
//! - [`base_source`]`(root, base, path) -> Option<Vec<u8>>`: the bytes of
//!   `path` at `base` (git `cat-file blob`, the backing git repository of a
//!   jj workspace, or `svn cat`); `None` when the path did not exist there or
//!   the ref is unsafe.
//! - [`symbol_delta`]`(root, base, changed_files) -> SymbolDelta`: per changed
//!   file, base parse versus current parse. [`SymbolDelta`] holds `removed`
//!   (production symbols), `removed_tests`, `removed_files`, `moved_files`
//!   (the old side of renames), `renamed_candidates`, and `signature_changed`.
//! - [`references_to`]`(store, targets, changed_files) -> Vec<Reference>`:
//!   graph edges from files outside `changed_files` that still point at the
//!   targets. [`SymbolDelta::reference_targets`] gives the usual targets.
//!   For a removed symbol a reference is `dangling_reference` evidence; for a
//!   signature change it is `unchanged_caller` evidence.
//!
//! # How references are found
//!
//! What the graph keeps for an edge into a deleted symbol depends on how the
//! caller's file was last indexed (measured on this repository's build and
//! update paths):
//!
//! - **Incremental update** (`dagayn update`, the hook): the caller's file is
//!   not re-parsed, so its `CALLS` edge keeps the old qualified target
//!   (`pkg/lib.py::helper`); post-processing only lowers its tier to `LOW`.
//! - **Full rebuild**: a Python or TypeScript caller still resolves the call
//!   through its import (`pkg/lib.py::helper`, `LOW`); a Rust caller's call
//!   becomes the bare name (`gone`), and an import of a deleted Python module
//!   becomes the unresolved module (`pkg.gone`).
//!
//! So each reference carries a [`MatchKind`]: an exact `target_qualified`
//! match (the stored tier is reported as is; `LOW` there means the graph
//! noticed the target vanished, not that the edge is weak), an
//! `IMPORTS_FROM` that names the symbol, an import of a deleted module by its
//! module name, or a bare-name edge accepted only when the same file imports
//! the changed file and defines no symbol of that name itself.
//!
//! # Limits
//!
//! - Symbols are compared by qualified name (`path::Parent.name`), so a
//!   symbol moved to another file reads as removed (and its new home as
//!   added elsewhere); a move inside a file under a new parent likewise.
//! - A code symbol the parser no longer finds while its name is still an
//!   identifier in the file goes to `unconfirmed_removed`, not `removed`: on
//!   this repository's history that is a function turned macro-generated.
//!   A deletion that leaves a use or comment naming it in the same file is
//!   held back the same way.
//! - Languages built without their grammar feature parse to a File node only
//!   and yield nothing.
//! - The base parse resolves modules against the current tree.
//! - Renames come from git's rename detection over tracked paths; SVN
//!   reports none, and a plain `mv` not yet added is a delete plus an add.
//! - Bare-name and module-name matching knows path-shaped module names
//!   (`pkg/gone.py` as `pkg.gone`, `crate::a::b`); relative TypeScript
//!   specifiers that no longer resolve are not matched.

// Nothing outside the tests calls this yet; review.rs wires it in.
#![allow(dead_code)]

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use dagayn_build::{Vcs, detect_vcs, is_safe_git_ref, jj, renames_since, svn};
use dagayn_graph::{ConfidenceTier, GraphEdge, GraphStore};
use dagayn_parser::{NodeKind, ParsedNode, RustOwnedParser};
use serde_json::{Value, json};

/// Edge kinds that make a node depend on another. `CONTAINS` is structure,
/// and a `TESTED_BY` edge points at the test (its reference is the test's
/// `CALLS` edge, which is listed).
const REFERENCE_KINDS: [&str; 7] = [
    "CALLS",
    "IMPORTS_FROM",
    "INHERITS",
    "IMPLEMENTS",
    "REFERENCES",
    "DEPENDS_ON",
    "CROSS_ARTIFACT",
];

/// Kinds whose bare-name targets are looked up when no qualified edge exists.
const BARE_NAME_KINDS: [&str; 4] = ["CALLS", "INHERITS", "IMPLEMENTS", "REFERENCES"];

/// The bytes of `path` at `base`, or `None` when it did not exist there, the
/// VCS cannot say, or `base` or `path` is unsafe to pass on.
///
/// Git reads the blob from the object store, so `base="HEAD"` on a dirty
/// tree is HEAD's content, not the edited file. In a jj workspace a
/// `HEAD`-relative ref is rebased onto `@-` and read from the backing git
/// repository. In SVN, `base` is a revision or a range whose start is used;
/// anything else reads the pristine `BASE` copy.
pub(crate) fn base_source(root: &Path, base: &str, path: &str) -> Option<Vec<u8>> {
    if !is_safe_repo_path(path) {
        return None;
    }
    match detect_vcs(root) {
        Vcs::Svn => {
            let rev = svn_base_revision(base);
            // A trailing `@` keeps an `@` in the path from reading as a peg.
            let target = if path.contains('@') {
                format!("{path}@")
            } else {
                path.to_string()
            };
            let mut command = Command::new("svn");
            command
                .args(["cat", "--non-interactive", "-r", &rev, "--", &target])
                .current_dir(root);
            stdout_bytes(command)
        }
        Vcs::Jj => {
            if !is_safe_base(base) {
                return None;
            }
            let commit = jj::resolve_commit(root, base, None)?;
            let mut command = jj::git_command(root)?;
            command.args(["cat-file", "blob", &format!("{commit}:{path}")]);
            stdout_bytes(command)
        }
        Vcs::Git | Vcs::None => {
            if !is_safe_base(base) {
                return None;
            }
            let mut command = Command::new("git");
            command
                .args(["cat-file", "blob", &format!("{base}:{path}")])
                .current_dir(root);
            stdout_bytes(command)
        }
    }
}

fn is_safe_base(base: &str) -> bool {
    is_safe_git_ref(base) && !base.starts_with('-') && !base.contains(':')
}

/// A repo-relative path that stays inside the repository.
fn is_safe_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '-'])
        && !path.contains('\0')
        && !path.split('/').any(|part| part == "..")
}

/// The revision `svn cat -r` reads for `base`: the start of a safe range
/// without its `r`, else the pristine `BASE`.
fn svn_base_revision(base: &str) -> String {
    let base = base.strip_suffix('\n').unwrap_or(base);
    if !svn::is_safe_svn_rev(base) {
        return "BASE".to_string();
    }
    let start = base.split(':').next().unwrap_or(base);
    start.trim_start_matches(['r', 'R']).to_string()
}

fn stdout_bytes(mut command: Command) -> Option<Vec<u8>> {
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// A symbol as it was at `base`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BaseSymbol {
    /// `Function`, `Class`, `Type`, `Test`, `DocSection`, or `File`.
    pub kind: String,
    pub name: String,
    /// The qualified name at `base`, under the base path: what an edge
    /// indexed before the change points at.
    pub qualified_name: String,
    /// The qualified name now, when the symbol still exists (a signature
    /// change, or a file that moved).
    pub current_qualified_name: Option<String>,
    /// The path at `base`.
    pub file_path: String,
    /// The path now; `None` for a deleted file.
    pub current_file: Option<String>,
    pub parent_name: Option<String>,
    /// Lines at `base`.
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub is_test: bool,
}

impl BaseSymbol {
    fn from_parsed(node: &ParsedNode, base_path: &str, current: Option<&str>) -> Self {
        let parsed_path = node.file_path.as_str();
        let qualified = qualified_name_of(node);
        let qualified_name = rebase_qualified(&qualified, parsed_path, base_path);
        Self {
            kind: node.kind.as_str().to_string(),
            name: node.name.clone(),
            qualified_name,
            current_qualified_name: None,
            file_path: base_path.to_string(),
            current_file: current.map(str::to_string),
            parent_name: node.parent_name.clone(),
            line_start: node.line_start,
            line_end: node.line_end,
            language: node.language.clone(),
            is_test: node.is_test || node.kind == NodeKind::Test,
        }
    }

    fn file(base_path: &str, current: Option<&str>, language: &str) -> Self {
        Self {
            kind: "File".to_string(),
            name: base_path.to_string(),
            qualified_name: base_path.to_string(),
            current_qualified_name: current.map(str::to_string),
            file_path: base_path.to_string(),
            current_file: current.map(str::to_string),
            parent_name: None,
            line_start: 1,
            line_end: 1,
            language: language.to_string(),
            is_test: false,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "name": self.name,
            "qualified_name": self.qualified_name,
            "current_qualified_name": self.current_qualified_name,
            "file_path": self.file_path,
            "current_file": self.current_file,
            "line_start": self.line_start,
            "line_end": self.line_end,
            "language": self.language,
            "is_test": self.is_test,
        })
    }
}

/// What a function declares, as the parser extracts it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Signature {
    pub params: Option<String>,
    pub return_type: Option<String>,
    pub modifiers: Option<String>,
}

impl Signature {
    fn of(node: &ParsedNode) -> Self {
        Self {
            params: node.params.clone(),
            return_type: node.return_type.clone(),
            modifiers: node.modifiers.clone(),
        }
    }

    /// Same declaration up to whitespace.
    fn same_shape(&self, other: &Self) -> bool {
        squash(&self.params) == squash(&other.params)
            && squash(&self.return_type) == squash(&other.return_type)
            && squash(&self.modifiers) == squash(&other.modifiers)
    }

    fn to_json(&self) -> Value {
        json!({
            "params": self.params,
            "return_type": self.return_type,
            "modifiers": self.modifiers,
        })
    }
}

/// `text` without whitespace, so reformatting is no change.
fn squash(text: &Option<String>) -> String {
    text.as_deref()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// A function whose qualified name survived but whose declaration changed.
#[derive(Clone, Debug)]
pub(crate) struct SignatureChange {
    /// The base side; `current_qualified_name` is set.
    pub symbol: BaseSymbol,
    pub before: Signature,
    pub after: Signature,
    /// Lines now.
    pub line_start: i64,
    pub line_end: i64,
    /// The params differ (not only return type or modifiers).
    pub params_changed: bool,
}

impl SignatureChange {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "symbol": self.symbol.to_json(),
            "before": self.before.to_json(),
            "after": self.after.to_json(),
            "line_start": self.line_start,
            "line_end": self.line_end,
            "params_changed": self.params_changed,
        })
    }
}

/// A removed symbol and an added one in the same file, with the same kind
/// and parent, whose bodies are identical once each one's own name is
/// blanked out; paired only when the match is unique both ways.
#[derive(Clone, Debug)]
pub(crate) struct RenameCandidate {
    pub from: BaseSymbol,
    pub to_name: String,
    pub to_qualified_name: String,
    pub to_line_start: i64,
}

impl RenameCandidate {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "from": self.from.to_json(),
            "to_name": self.to_name,
            "to_qualified_name": self.to_qualified_name,
            "to_line_start": self.to_line_start,
        })
    }
}

/// Base versus now for a set of changed files.
#[derive(Clone, Debug, Default)]
pub(crate) struct SymbolDelta {
    /// Production symbols present at `base` and gone now. A renamed symbol
    /// stays here and is also listed in `renamed_candidates`.
    pub removed: Vec<BaseSymbol>,
    /// Test functions and test classes that went away.
    pub removed_tests: Vec<BaseSymbol>,
    /// Deleted files (kind `File`).
    pub removed_files: Vec<BaseSymbol>,
    /// Renamed files: the old path, with `current_file` the new one. Their
    /// symbols are compared under the new path, so a pure move adds nothing
    /// else; an import still naming the old path is a reference to it.
    pub moved_files: Vec<BaseSymbol>,
    /// Code symbols the parser no longer finds although their name is still
    /// an identifier in the file (generated by a macro, behind a `cfg`, or
    /// deleted with a use left behind). Kept out of `removed` and of
    /// [`SymbolDelta::reference_targets`] for precision.
    pub unconfirmed_removed: Vec<BaseSymbol>,
    pub renamed_candidates: Vec<RenameCandidate>,
    pub signature_changed: Vec<SignatureChange>,
    /// `(base path, current path)` of each file compared.
    pub files_compared: Vec<(String, String)>,
}

impl SymbolDelta {
    /// What [`references_to`] should be asked about: removed production
    /// symbols, removed or moved files, and changed signatures.
    pub(crate) fn reference_targets(&self) -> Vec<&BaseSymbol> {
        self.removed
            .iter()
            .chain(&self.removed_files)
            .chain(&self.moved_files)
            .chain(self.signature_changed.iter().map(|change| &change.symbol))
            .collect()
    }

    /// No symbol removed or reshaped (a pure move counts as empty).
    pub(crate) fn is_empty(&self) -> bool {
        self.removed.is_empty()
            && self.removed_tests.is_empty()
            && self.removed_files.is_empty()
            && self.renamed_candidates.is_empty()
            && self.signature_changed.is_empty()
    }

    pub(crate) fn to_json(&self) -> Value {
        let list =
            |symbols: &[BaseSymbol]| -> Value { symbols.iter().map(BaseSymbol::to_json).collect() };
        json!({
            "removed": list(&self.removed),
            "removed_tests": list(&self.removed_tests),
            "removed_files": list(&self.removed_files),
            "moved_files": list(&self.moved_files),
            "unconfirmed_removed": list(&self.unconfirmed_removed),
            "renamed_candidates": self
                .renamed_candidates
                .iter()
                .map(RenameCandidate::to_json)
                .collect::<Vec<_>>(),
            "signature_changed": self
                .signature_changed
                .iter()
                .map(SignatureChange::to_json)
                .collect::<Vec<_>>(),
        })
    }
}

/// `qualified_name_of` in dagayn-parser (and `make_qualified_parts` in
/// dagayn-graph): the name the graph stores for a node.
fn qualified_name_of(node: &ParsedNode) -> String {
    match (node.kind, &node.parent_name) {
        (NodeKind::File, _) => node.file_path.to_string(),
        (_, Some(parent)) => format!("{}::{parent}.{}", node.file_path, node.name),
        (_, None) => format!("{}::{}", node.file_path, node.name),
    }
}

/// `qualified` with its file prefix `from` replaced by `to`.
fn rebase_qualified(qualified: &str, from: &str, to: &str) -> String {
    if from == to {
        return qualified.to_string();
    }
    match qualified.strip_prefix(from) {
        Some(rest) if rest.is_empty() || rest.starts_with("::") => format!("{to}{rest}"),
        _ => qualified.to_string(),
    }
}

/// Nodes a reviewer could reference: not the File node, not a section body.
fn is_symbol(node: &ParsedNode) -> bool {
    !matches!(node.kind, NodeKind::File | NodeKind::DocBody)
}

/// `(base path, current path)` for each changed file: the new path of a
/// rename is paired with its old path, and the old path is not compared on
/// its own.
fn file_pairs(
    changed_files: &[String],
    renames: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let old_to_new: HashMap<&str, &str> = renames
        .iter()
        .map(|(new, old)| (old.as_str(), new.as_str()))
        .collect();
    let mut seen = HashSet::new();
    let mut pairs = Vec::new();
    for path in changed_files {
        let pair = if let Some(old) = renames.get(path) {
            (old.clone(), path.clone())
        } else if let Some(new) = old_to_new.get(path.as_str()) {
            (path.clone(), (*new).to_string())
        } else {
            (path.clone(), path.clone())
        };
        if seen.insert(pair.clone()) {
            pairs.push(pair);
        }
    }
    pairs
}

/// Each changed file parsed at `base` and now, compared by qualified name.
///
/// A file absent at `base` (added) contributes nothing; a file absent now
/// (deleted) contributes every symbol as removed. A renamed file is parsed
/// under its new path on both sides, so a pure move is no change.
pub(crate) fn symbol_delta(root: &Path, base: &str, changed_files: &[String]) -> SymbolDelta {
    let renames = renames_since(root, base);
    let mut parser = RustOwnedParser::new();
    let mut delta = SymbolDelta::default();
    for (base_path, current_path) in file_pairs(changed_files, &renames) {
        let Some(base_bytes) = base_source(root, base, &base_path) else {
            continue;
        };
        let current_bytes = std::fs::metadata(root.join(&current_path))
            .ok()
            .filter(std::fs::Metadata::is_file)
            .and_then(|_| std::fs::read(root.join(&current_path)).ok());
        delta
            .files_compared
            .push((base_path.clone(), current_path.clone()));
        match current_bytes {
            None => {
                let (nodes, _) = parser.parse_file_in_repo(Some(root), &base_path, &base_bytes);
                let language = nodes
                    .iter()
                    .find(|node| node.kind == NodeKind::File)
                    .map(|node| node.language.clone())
                    .unwrap_or_default();
                for node in nodes.iter().filter(|node| is_symbol(node)) {
                    push_removed(&mut delta, BaseSymbol::from_parsed(node, &base_path, None));
                }
                delta
                    .removed_files
                    .push(BaseSymbol::file(&base_path, None, &language));
            }
            Some(current_bytes) => {
                let (base_nodes, _) =
                    parser.parse_file_in_repo(Some(root), &current_path, &base_bytes);
                let (now_nodes, _) =
                    parser.parse_file_in_repo(Some(root), &current_path, &current_bytes);
                if base_path != current_path {
                    let language = now_nodes
                        .iter()
                        .find(|node| node.kind == NodeKind::File)
                        .map(|node| node.language.clone())
                        .unwrap_or_default();
                    delta.moved_files.push(BaseSymbol::file(
                        &base_path,
                        Some(&current_path),
                        &language,
                    ));
                }
                compare_file(
                    &mut delta,
                    FileSides {
                        base_path: &base_path,
                        current_path: &current_path,
                        base_source: &base_bytes,
                        current_source: &current_bytes,
                    },
                    &base_nodes,
                    &now_nodes,
                );
            }
        }
    }
    delta
}

fn push_removed(delta: &mut SymbolDelta, symbol: BaseSymbol) {
    if symbol.is_test {
        delta.removed_tests.push(symbol);
    } else {
        delta.removed.push(symbol);
    }
}

struct FileSides<'a> {
    base_path: &'a str,
    current_path: &'a str,
    base_source: &'a [u8],
    current_source: &'a [u8],
}

fn compare_file(
    delta: &mut SymbolDelta,
    sides: FileSides<'_>,
    base_nodes: &[ParsedNode],
    now_nodes: &[ParsedNode],
) {
    let now_by_name: HashMap<String, &ParsedNode> = now_nodes
        .iter()
        .filter(|node| is_symbol(node))
        .map(|node| (qualified_name_of(node), node))
        .collect();
    let base_names: HashSet<String> = base_nodes
        .iter()
        .filter(|node| is_symbol(node))
        .map(qualified_name_of)
        .collect();
    let current_text = String::from_utf8_lossy(sides.current_source);
    let mut removed_here: Vec<(&ParsedNode, BaseSymbol)> = Vec::new();
    for node in base_nodes.iter().filter(|node| is_symbol(node)) {
        let qualified = qualified_name_of(node);
        let mut symbol = BaseSymbol::from_parsed(node, sides.base_path, Some(sides.current_path));
        match now_by_name.get(&qualified) {
            None => {
                if !symbol.is_test && still_named(node, &current_text) {
                    delta.unconfirmed_removed.push(symbol);
                    continue;
                }
                if !symbol.is_test {
                    removed_here.push((node, symbol.clone()));
                }
                push_removed(delta, symbol);
            }
            Some(now) => {
                if node.kind != NodeKind::Function || symbol.is_test || now.is_test {
                    continue;
                }
                let (before, after) = (Signature::of(node), Signature::of(now));
                if before.same_shape(&after) {
                    continue;
                }
                let params_changed = squash(&before.params) != squash(&after.params);
                symbol.current_qualified_name = Some(qualified);
                delta.signature_changed.push(SignatureChange {
                    symbol,
                    before,
                    after,
                    line_start: now.line_start,
                    line_end: now.line_end,
                    params_changed,
                });
            }
        }
    }
    let added: Vec<&ParsedNode> = now_nodes
        .iter()
        .filter(|node| is_symbol(node) && !node.is_test)
        .filter(|node| !base_names.contains(&qualified_name_of(node)))
        .collect();
    delta.renamed_candidates.extend(rename_candidates(
        &removed_here,
        &added,
        sides.base_source,
        sides.current_source,
    ));
}

/// Whether a code symbol's name is still an identifier in the file: the
/// parser no longer sees a declaration, but a macro may generate it (Rust
/// `grammar_parser!(new_rust_parser, ..)`), a `cfg` or conditional may hide
/// it, or a use may be left behind. Section titles are not checked.
fn still_named(node: &ParsedNode, text: &str) -> bool {
    if !matches!(
        node.kind,
        NodeKind::Function | NodeKind::Class | NodeKind::Type
    ) {
        return false;
    }
    let name = node.name.as_str();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    if name.is_empty() || !name.chars().all(is_ident) {
        return false;
    }
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Pairs each removed symbol with the one added symbol of the same kind and
/// parent whose body has the same shape, when neither side has another
/// match.
fn rename_candidates(
    removed: &[(&ParsedNode, BaseSymbol)],
    added: &[&ParsedNode],
    base_source: &[u8],
    current_source: &[u8],
) -> Vec<RenameCandidate> {
    let base_text = String::from_utf8_lossy(base_source);
    let current_text = String::from_utf8_lossy(current_source);
    let base_lines: Vec<&str> = base_text.lines().collect();
    let current_lines: Vec<&str> = current_text.lines().collect();
    let removed_shapes: Vec<Option<String>> = removed
        .iter()
        .map(|(node, _)| body_shape(&base_lines, node))
        .collect();
    let added_shapes: Vec<Option<String>> = added
        .iter()
        .map(|node| body_shape(&current_lines, node))
        .collect();
    let matches = |r: usize, a: usize| -> bool {
        let (old, new) = (removed[r].0, added[a]);
        old.kind == new.kind
            && old.parent_name == new.parent_name
            && removed_shapes[r].is_some()
            && removed_shapes[r] == added_shapes[a]
    };
    let mut out = Vec::new();
    for r in 0..removed.len() {
        let hits: Vec<usize> = (0..added.len()).filter(|&a| matches(r, a)).collect();
        let [a] = hits[..] else {
            continue;
        };
        if (0..removed.len())
            .filter(|&other| matches(other, a))
            .count()
            != 1
        {
            continue;
        }
        let new = added[a];
        out.push(RenameCandidate {
            from: removed[r].1.clone(),
            to_name: new.name.clone(),
            to_qualified_name: qualified_name_of(new),
            to_line_start: new.line_start,
        });
    }
    out
}

/// A symbol's lines with its own name blanked and each line trimmed; `None`
/// when the span is empty or out of range.
fn body_shape(lines: &[&str], node: &ParsedNode) -> Option<String> {
    let start = usize::try_from(node.line_start).ok()?.checked_sub(1)?;
    let end = usize::try_from(node.line_end).ok()?.min(lines.len());
    if node.name.is_empty() || start >= end {
        return None;
    }
    let shape = lines[start..end]
        .iter()
        .map(|line| line.replace(node.name.as_str(), "\u{0}").trim().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    (!shape.trim().is_empty()).then_some(shape)
}

/// How a reference was tied to its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MatchKind {
    /// The edge's `target_qualified` is the symbol's qualified name.
    ExactTarget,
    /// An `IMPORTS_FROM` into the symbol's file that lists its name.
    ImportedName,
    /// An `IMPORTS_FROM` whose unresolved module names the deleted file.
    ImportedModule,
    /// A bare-name edge in a file that imports the changed file and defines
    /// no symbol of that name.
    BareNameViaImport,
}

impl MatchKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExactTarget => "exact_target",
            Self::ImportedName => "imported_name",
            Self::ImportedModule => "imported_module",
            Self::BareNameViaImport => "bare_name_via_import",
        }
    }

    /// `high`, `medium`, or `low`: how sure the match is that the edge means
    /// this symbol, independent of the edge's stored tier.
    pub(crate) fn confidence(self) -> &'static str {
        match self {
            Self::ExactTarget | Self::ImportedName => "high",
            Self::ImportedModule => "medium",
            Self::BareNameViaImport => "low",
        }
    }
}

/// A node outside the change that still points at a target.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reference {
    /// The target's base qualified name ([`BaseSymbol::qualified_name`]).
    pub target: String,
    pub edge_kind: String,
    pub source_qualified: String,
    /// The edge's target as stored.
    pub edge_target: String,
    pub file_path: String,
    pub line: i64,
    /// The tier stored on the edge.
    pub edge_tier: ConfidenceTier,
    pub matched_by: MatchKind,
}

impl Reference {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "edge_kind": self.edge_kind,
            "source_qualified": self.source_qualified,
            "edge_target": self.edge_target,
            "file_path": self.file_path,
            "line": self.line,
            "edge_tier": self.edge_tier.as_str(),
            "matched_by": self.matched_by.as_str(),
            "confidence": self.matched_by.confidence(),
        })
    }
}

/// Edges from files outside `changed_files` that still point at `targets`,
/// strongest match first per (target, source, kind, line).
pub(crate) fn references_to(
    store: &GraphStore,
    targets: &[&BaseSymbol],
    changed_files: &[String],
) -> Vec<Reference> {
    let changed: HashSet<&str> = changed_files.iter().map(String::as_str).collect();
    let outside = |edge: &GraphEdge| -> bool {
        !changed.contains(edge.file_path.as_str())
            && !changed.contains(source_file(&edge.source_qualified))
    };
    let mut found: HashMap<(String, String, String, i64), Reference> = HashMap::new();
    let mut add = |target: &BaseSymbol, edge: &GraphEdge, matched_by: MatchKind| {
        let key = (
            target.qualified_name.clone(),
            edge.source_qualified.clone(),
            edge.kind.clone(),
            edge.line,
        );
        let candidate = Reference {
            target: target.qualified_name.clone(),
            edge_kind: edge.kind.clone(),
            source_qualified: edge.source_qualified.clone(),
            edge_target: edge.target_qualified.clone(),
            file_path: edge.file_path.clone(),
            line: edge.line,
            edge_tier: edge.confidence_tier,
            matched_by,
        };
        match found.get(&key) {
            Some(existing) if existing.matched_by <= matched_by => {}
            _ => {
                found.insert(key, candidate);
            }
        }
    };

    // Exact qualified targets, under the base name and the current one.
    let kinds: Vec<String> = REFERENCE_KINDS.iter().map(|k| k.to_string()).collect();
    let mut keys: Vec<String> = Vec::new();
    for target in targets {
        keys.push(target.qualified_name.clone());
        if let Some(now) = &target.current_qualified_name {
            keys.push(now.clone());
        }
    }
    let exact = store
        .get_edges_by_targets(&keys, &kinds)
        .unwrap_or_default();
    for target in targets {
        let names = std::iter::once(&target.qualified_name)
            .chain(target.current_qualified_name.as_ref())
            .collect::<HashSet<_>>();
        for name in names {
            for edge in exact.get(name).into_iter().flatten() {
                // A file that only moved is still imported at its new path.
                let moved_and_current = target.kind == "File"
                    && target.current_qualified_name.as_deref()
                        == Some(edge.target_qualified.as_str());
                if outside(edge) && !moved_and_current {
                    add(target, edge, MatchKind::ExactTarget);
                }
            }
        }
    }

    // Imports into each target's file, by path or by module name.
    let mut imports_by_file: HashMap<String, Vec<(GraphEdge, bool)>> = HashMap::new();
    for target in targets {
        if imports_by_file.contains_key(&target.file_path) {
            continue;
        }
        imports_by_file.insert(
            target.file_path.clone(),
            imports_of(store, &target.file_path, target.current_file.as_deref()),
        );
    }
    let mut importers: HashMap<&str, HashSet<String>> = HashMap::new();
    for target in targets {
        let imports = &imports_by_file[&target.file_path];
        let wanted = import_name(target);
        for (edge, by_module) in imports {
            if !outside(edge) {
                continue;
            }
            let listed = imported_names(edge);
            let names_it = listed.iter().any(|(name, _)| name == wanted);
            if target.kind == "File" {
                if *by_module && target.current_file.is_none() {
                    add(target, edge, MatchKind::ImportedModule);
                }
                continue;
            }
            if names_it {
                let kind = if *by_module {
                    MatchKind::ImportedModule
                } else {
                    MatchKind::ImportedName
                };
                add(target, edge, kind);
            }
            if names_it || listed.is_empty() {
                importers
                    .entry(target.qualified_name.as_str())
                    .or_default()
                    .insert(edge.file_path.clone());
            }
        }
    }

    // Bare names in files that import the target's file.
    let forms_of = |target: &BaseSymbol| -> Vec<String> {
        bare_forms(target, &imports_by_file[&target.file_path])
    };
    let mut bare_names: Vec<String> = targets
        .iter()
        .filter(|target| target.kind != "File")
        .flat_map(|target| forms_of(target))
        .collect();
    bare_names.sort();
    bare_names.dedup();
    for kind in BARE_NAME_KINDS {
        let by_name = store
            .get_edges_by_target_names(&bare_names, kind, false)
            .unwrap_or_default();
        for target in targets.iter().filter(|target| target.kind != "File") {
            let Some(files) = importers.get(target.qualified_name.as_str()) else {
                continue;
            };
            for form in forms_of(target) {
                for edge in by_name.get(&form).into_iter().flatten() {
                    let qualified = edge.target_qualified.contains("::");
                    if qualified || !outside(edge) || !files.contains(&edge.file_path) {
                        continue;
                    }
                    // A same-named symbol of the caller's own file shadows it.
                    let local = format!("{}::{form}", edge.file_path);
                    if store.get_node(&local).ok().flatten().is_some() {
                        continue;
                    }
                    add(target, edge, MatchKind::BareNameViaImport);
                }
            }
        }
    }

    let mut references: Vec<Reference> = found.into_values().collect();
    references.sort_by(|a, b| {
        (&a.target, a.matched_by, &a.file_path, a.line, &a.edge_kind).cmp(&(
            &b.target,
            b.matched_by,
            &b.file_path,
            b.line,
            &b.edge_kind,
        ))
    });
    references
}

/// The file part of a qualified name.
fn source_file(qualified: &str) -> &str {
    qualified
        .split_once("::")
        .map_or(qualified, |(file, _)| file)
}

/// The name an import of the target's file would list: the symbol, or the
/// outermost parent of a member.
fn import_name(target: &BaseSymbol) -> &str {
    match &target.parent_name {
        Some(parent) => parent.split('.').next().unwrap_or(parent),
        None => &target.name,
    }
}

/// Bare target names an unresolved edge to the target would carry: its
/// name, `Parent.name` for a member, and any alias an import gave it.
fn bare_forms(target: &BaseSymbol, imports: &[(GraphEdge, bool)]) -> Vec<String> {
    let mut forms = vec![target.name.clone()];
    if let Some(parent) = &target.parent_name {
        forms.push(format!("{parent}.{}", target.name));
    } else {
        for (edge, _) in imports {
            for (name, alias) in imported_names(edge) {
                if name == target.name && !forms.contains(&alias) {
                    forms.push(alias);
                }
            }
        }
    }
    forms
}

/// `extra.names` of an `IMPORTS_FROM` edge as `(name, alias)` pairs.
fn imported_names(edge: &GraphEdge) -> Vec<(String, String)> {
    edge.extra
        .get("names")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    let name = pair.first()?.as_str()?.to_string();
                    let alias = pair
                        .get(1)
                        .and_then(Value::as_str)
                        .map_or_else(|| name.clone(), str::to_string);
                    Some((name, alias))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `IMPORTS_FROM` edges into `base_path` (or the file's current path), and
/// whether each matched by module name rather than path.
fn imports_of(
    store: &GraphStore,
    base_path: &str,
    current: Option<&str>,
) -> Vec<(GraphEdge, bool)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut paths = vec![base_path.to_string()];
    if let Some(current) = current.filter(|current| *current != base_path) {
        paths.push(current.to_string());
    }
    let modules = module_forms(base_path);
    let kinds = vec!["IMPORTS_FROM".to_string()];
    let mut keys = paths.clone();
    keys.extend(modules.iter().cloned());
    let edges = store
        .get_edges_by_targets(&keys, &kinds)
        .unwrap_or_default();
    for key in &keys {
        let by_module = !paths.contains(key);
        for edge in edges.get(key).into_iter().flatten() {
            if seen.insert(edge.id) {
                out.push((edge.clone(), by_module));
            }
        }
    }
    out
}

/// Module names an import of `path` could carry once the file is gone:
/// dotted (`pkg.gone`) and Rust (`crate::pkg::gone`) forms of the path
/// without its extension or package-index file, for each suffix of two or
/// more segments (the whole path when it has one).
fn module_forms(path: &str) -> Vec<String> {
    let stem = match path.rsplit_once('/') {
        Some((dir, file)) => match file.rsplit_once('.') {
            Some((name, _)) if !name.is_empty() => format!("{dir}/{name}"),
            _ => path.to_string(),
        },
        None => path
            .rsplit_once('.')
            .map_or(path, |(name, _)| name)
            .to_string(),
    };
    let stem = ["/__init__", "/index", "/mod"]
        .iter()
        .find_map(|suffix| stem.strip_suffix(suffix))
        .map_or(stem.clone(), str::to_string);
    let parts: Vec<&str> = stem.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let mut forms = Vec::new();
    let min = if parts.len() == 1 { 1 } else { 2 };
    for start in 0..=parts.len() - min {
        let suffix = &parts[start..];
        forms.push(suffix.join("."));
        forms.push(format!("crate::{}", suffix.join("::")));
    }
    // `src/a/b.rs` is `crate::a::b`.
    if let Some(rest) = parts.iter().position(|part| *part == "src") {
        let suffix = &parts[rest + 1..];
        if !suffix.is_empty() {
            forms.push(format!("crate::{}", suffix.join("::")));
        }
    }
    forms.sort();
    forms.dedup();
    forms
}
