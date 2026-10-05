//! `dagayn status`: graph statistics, embedding coverage, and freshness.
//!
//! Port of `handle_status_command` and the assessment it prints,
//! `dagayn.tools.sync_status.assess_graph_sync`. The printed lines match the
//! Python CLI's.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use dagayn_graph::{GraphError, GraphStore};

use crate::parse_batch::{file_mtime_ns, sha256_hex};
use crate::update::outdated_extractors;
use crate::vcs::{
    Vcs, detect_vcs, dirty_files, git_branch_info, is_linked_worktree, main_checkout,
};

/// `GIT_BACKED_VCS`: revisions are git commits, so `git_head_sha` applies.
fn is_git_backed(vcs: Vcs) -> bool {
    matches!(vcs, Vcs::Git | Vcs::Jj)
}

/// Hash at most this many files to verify the diff tier; above it the
/// assessment falls back to git's answer (a just-seeded worktree whose stored
/// mtimes all came from the main checkout lands here).
const MAX_HASH_CANDIDATES: usize = 200;
const SEEDED_NEEDS_VERIFY_KEY: &str = "seeded_needs_content_verify";
const ACTIVE_EMBEDDING_PROVIDER_KEY: &str = "embedding_provider";

/// The graph's freshness relative to the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAssessment {
    /// `unbuilt`, `commit_drift`, `commit_synced`, `worktree_behind`, or
    /// `worktree_ahead`.
    pub state: &'static str,
    pub extractor_drift: Vec<String>,
    /// Files the graph is behind on, for `worktree_behind`.
    pub pending_files: Vec<String>,
    /// Git reports uncommitted changes (`worktree_dirty`).
    pub worktree_dirty: bool,
    /// HEAD of the working copy, when git is the VCS and it has one.
    pub current_head_sha: Option<String>,
    /// The commit the graph records (`git_head_sha`).
    pub git_head_sha: Option<String>,
    /// The checked-out branch, when git is the VCS and HEAD is on one.
    pub current_branch: Option<String>,
    pub last_updated: Option<String>,
    /// Dirty files already in the graph byte for byte, for `worktree_ahead`.
    pub indexed_files: Vec<String>,
    /// False when the diff tier gave up on hashing (too many candidates) or
    /// failed; the state is then git's dirty-only answer.
    pub content_verified: bool,
    /// Files left unhashed when the cap bit.
    pub unverified_file_count: usize,
}

/// The lines `dagayn status` prints.
pub fn status_lines(repo_root: &Path, store: &GraphStore) -> Result<Vec<String>, GraphError> {
    let stats = store.get_stats()?;
    let mut lines = vec![
        format!("Nodes: {}", stats.total_nodes),
        format!("Edges: {}", stats.total_edges),
        format!("Files: {}", stats.files_count),
        format!("Languages: {}", stats.languages.join(", ")),
        format!(
            "Last updated: {}",
            stats.last_updated.as_deref().unwrap_or("never")
        ),
    ];
    embedding_lines(store, &mut lines);
    vcs_lines(repo_root, store, &mut lines)?;
    // Status must never fail on the assessment.
    if let Ok(sync) = assess_graph_sync(store, repo_root) {
        let hint = match sync.state {
            "unbuilt" => Some("no graph yet — run 'dagayn build'"),
            "commit_drift" => Some("graph describes another commit — run 'dagayn update'"),
            "commit_synced" => Some("graph matches HEAD"),
            "worktree_behind" => {
                Some("uncommitted or reverted edits are not in the graph — run 'dagayn update'")
            }
            "worktree_ahead" => Some("graph already includes the uncommitted edits"),
            _ => None,
        };
        lines.push(match hint {
            Some(hint) => format!("Graph state: {} — {hint}", sync.state),
            None => format!("Graph state: {}", sync.state),
        });
        if !sync.extractor_drift.is_empty() {
            lines.push(format!(
                "  Parsed by an older extractor: {} — 'dagayn update' re-parses those files",
                sync.extractor_drift.join(", ")
            ));
        }
        if !sync.pending_files.is_empty() {
            let shown = sync.pending_files[..sync.pending_files.len().min(5)].join(", ");
            let more = sync.pending_files.len().saturating_sub(5);
            lines.push(if more > 0 {
                format!("  Needs re-indexing: {shown} (+{more} more)")
            } else {
                format!("  Needs re-indexing: {shown}")
            });
        }
    }
    Ok(lines)
}

/// `_print_embedding_status`.
fn embedding_lines(store: &GraphStore, lines: &mut Vec<String>) {
    let counts = match store.embedding_provider_counts() {
        Ok(Some(counts)) => counts,
        Ok(None) => {
            lines.push("Embeddings: not indexed".to_string());
            return;
        }
        Err(err) => {
            lines.push(format!("Embeddings: unavailable ({err})"));
            return;
        }
    };
    let total: i64 = counts.values().sum();
    let preferred = store
        .get_metadata(ACTIVE_EMBEDDING_PROVIDER_KEY)
        .ok()
        .flatten();
    let provider = resolve_active_provider(&counts, preferred.as_deref());
    let coverage = match store.embedding_coverage(provider.as_deref()) {
        Ok(coverage) => coverage,
        Err(err) => {
            lines.push(format!("Embeddings: unavailable ({err})"));
            return;
        }
    };
    let state = if total == 0 {
        "empty"
    } else if coverage.orphan_embeddings > 0 {
        "stale"
    } else if coverage.missing_embeddings > 0 {
        "partial"
    } else {
        "complete"
    };
    lines.push(format!(
        "Embeddings: {state} ({total} vectors, {} provider(s))",
        counts.len()
    ));
    lines.push(format!(
        "  Coverage: {}/{} embeddable nodes ({} missing)",
        coverage.indexed_embeddings, coverage.embeddable_nodes, coverage.missing_embeddings
    ));
    if coverage.orphan_embeddings > 0 {
        lines.push(format!("  Orphans: {}", coverage.orphan_embeddings));
    }
    let mut providers: Vec<(&String, &i64)> = counts.iter().collect();
    providers.sort();
    for (name, count) in providers {
        lines.push(format!("  Provider: {name} ({count})"));
    }
}

/// `commit_tier_freshness`: the cheap half of the assessment that read tools
/// attach to every answer, for a git working copy with a HEAD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFreshness {
    /// `commit_synced` or `commit_drift` (also for an extractor drift).
    pub state: &'static str,
    pub git_head_sha: Option<String>,
    pub current_head_sha: String,
    pub worktree_dirty: bool,
    pub extractor_drift: Vec<String>,
}

/// `None` where Python's state is `None`: not a git checkout or jj
/// workspace (`GIT_BACKED_VCS`), or no HEAD (in jj, a working copy jj cannot
/// read).
pub fn commit_tier_freshness(
    store: &GraphStore,
    repo_root: &Path,
) -> Result<Option<CommitFreshness>, GraphError> {
    if !is_git_backed(detect_vcs(repo_root)) {
        return Ok(None);
    }
    let stored = store
        .get_metadata("git_head_sha")?
        .filter(|sha| !sha.is_empty());
    let (_, current) = git_branch_info(repo_root);
    if current.is_empty() {
        return Ok(None);
    }
    let dirty = !dirty_files(repo_root).is_empty();
    let languages = store.get_stats()?.languages;
    let drift: Vec<String> = outdated_extractors(store, &languages)?
        .iter()
        .map(|entry| entry.extractor.to_string())
        .collect();
    let state = if !drift.is_empty() || stored.as_deref() != Some(current.as_str()) {
        "commit_drift"
    } else {
        "commit_synced"
    };
    Ok(Some(CommitFreshness {
        state,
        git_head_sha: stored,
        current_head_sha: current,
        worktree_dirty: dirty,
        extractor_drift: drift,
    }))
}

/// Whether `embedding_refresh_action` would answer `skip` for a requested
/// local embedding mode: vectors exist and none of the active provider's are
/// missing. `not_indexed` and `empty` refresh inline, missing vectors refresh
/// inline or in the background; both are the Python server's to start.
pub fn embedding_refresh_skips(store: &GraphStore) -> Result<bool, GraphError> {
    let Some(counts) = store.embedding_provider_counts()? else {
        return Ok(false);
    };
    if counts.values().sum::<i64>() == 0 {
        return Ok(false);
    }
    let preferred = store.get_metadata(ACTIVE_EMBEDDING_PROVIDER_KEY)?;
    let provider = resolve_active_provider(&counts, preferred.as_deref());
    let coverage = store.embedding_coverage(provider.as_deref())?;
    Ok(coverage.missing_embeddings <= 0 || coverage.embeddable_nodes <= 0)
}

/// `resolve_active_embedding_provider` without a text mode.
fn resolve_active_provider(
    counts: &HashMap<String, i64>,
    preferred: Option<&str>,
) -> Option<String> {
    resolve_active_embedding_provider(counts, None, preferred)
}

/// `resolve_active_embedding_provider`: the stored provider wins, matched to a
/// partition by identity; otherwise the largest partition among those of
/// `text_mode` (`_provider_candidates`: that mode's, else the legacy unmoded
/// ones, else all).
pub fn resolve_active_embedding_provider(
    counts: &HashMap<String, i64>,
    text_mode: Option<&str>,
    preferred: Option<&str>,
) -> Option<String> {
    if let Some(preferred) = preferred.filter(|name| !name.is_empty()) {
        return Some(
            match_preferred_provider(preferred, counts).unwrap_or_else(|| preferred.to_string()),
        );
    }
    let candidates: Vec<(&String, &i64)> = match text_mode.filter(|mode| !mode.is_empty()) {
        None => counts.iter().collect(),
        Some(mode) => {
            let suffix = format!("#text={mode}");
            let matches: Vec<_> = counts
                .iter()
                .filter(|(name, _)| name.ends_with(&suffix))
                .collect();
            let legacy: Vec<_> = counts
                .iter()
                .filter(|(name, _)| !name.contains("#text="))
                .collect();
            if !matches.is_empty() {
                matches
            } else if !legacy.is_empty() {
                legacy
            } else {
                counts.iter().collect()
            }
        }
    };
    candidates
        .into_iter()
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| left.0.cmp(right.0)))
        .map(|(name, _)| name.clone())
}

fn match_preferred_provider(preferred: &str, counts: &HashMap<String, i64>) -> Option<String> {
    if counts.contains_key(preferred) {
        return Some(preferred.to_string());
    }
    let wanted = provider_identity(preferred);
    let mode = text_mode(preferred);
    let matches: Vec<&String> = counts
        .keys()
        .filter(|name| {
            provider_identity(name) == wanted
                && (mode.is_none() || text_mode(name).is_none() || text_mode(name) == mode)
        })
        .collect();
    if let Some(best) = matches.iter().max_by(|left, right| {
        counts[**left]
            .cmp(&counts[**right])
            .then_with(|| left.cmp(right))
    }) {
        return Some((*best).clone());
    }
    let mut names: Vec<&String> = counts.keys().collect();
    names.sort();
    names
        .into_iter()
        .find(|name| openai_names_match(name, preferred) || openai_names_match(preferred, name))
        .cloned()
}

/// Case-folded model identity without `#text=` and `#dim=` suffixes.
fn provider_identity(name: &str) -> String {
    let base = name.split("#text=").next().unwrap_or(name);
    let mut out = String::new();
    let mut rest = base;
    while let Some(index) = rest.find("#dim=") {
        out.push_str(&rest[..index]);
        let digits = rest[index + 5..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(rest.len(), |end| index + 5 + end);
        if digits == index + 5 {
            // `#dim=` without digits is not a dimension suffix.
            out.push_str("#dim=");
            rest = &rest[index + 5..];
        } else {
            rest = &rest[digits..];
        }
    }
    out.push_str(rest);
    out.to_lowercase()
}

fn text_mode(name: &str) -> Option<&str> {
    name.rsplit_once("#text=")
        .map(|(_, mode)| mode)
        .filter(|mode| !mode.is_empty())
}

/// `_openai_provider_names_match`: same name ignoring case, or `computed` is
/// `persisted` plus a `#dim=N` suffix.
pub fn openai_names_match(persisted: &str, computed: &str) -> bool {
    if persisted.to_lowercase() == computed.to_lowercase() {
        return true;
    }
    if persisted.contains("#dim=") {
        return false;
    }
    computed
        .strip_prefix(persisted)
        .and_then(|suffix| suffix.strip_prefix("#dim="))
        .is_some_and(|digits| digits.parse::<i64>().is_ok())
}

/// `_print_vcs_status` for git; the SVN branch of it is not ported.
fn vcs_lines(
    repo_root: &Path,
    store: &GraphStore,
    lines: &mut Vec<String>,
) -> Result<(), GraphError> {
    let stored_branch = store.get_metadata("git_branch")?.filter(|v| !v.is_empty());
    let stored_sha = store
        .get_metadata("git_head_sha")?
        .filter(|v| !v.is_empty());
    let git = detect_vcs(repo_root) == Vcs::Git;
    let label = (git && is_linked_worktree(repo_root))
        .then(|| {
            repo_root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .flatten();
    if let Some(label) = &label {
        let main = main_checkout(repo_root)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "None".to_string());
        lines.push(format!("Linked worktree: {label} (main checkout: {main})"));
    }
    if let Some(branch) = &stored_branch {
        lines.push(format!("Built on branch: {branch}"));
    }
    if let Some(sha) = &stored_sha {
        lines.push(format!("Built at commit: {}", short(sha)));
    }
    if !git {
        return Ok(());
    }
    let (current_branch, current_sha) = git_branch_info(repo_root);
    let refresh_hint = if label.is_some() {
        "Run 'dagayn worktree sync' to catch up."
    } else {
        "Run 'dagayn build' to rebuild."
    };
    let same_commit = stored_sha
        .as_deref()
        .is_some_and(|sha| !current_sha.is_empty() && sha == current_sha);
    if same_commit {
        return Ok(());
    }
    match (&stored_branch, &stored_sha) {
        (Some(branch), _) if !current_branch.is_empty() && *branch != current_branch => {
            lines.push(format!(
                "WARNING: Graph was built on '{branch}' but you are now on '{current_branch}'. \
                 {refresh_hint}"
            ));
        }
        (_, Some(sha)) if !current_sha.is_empty() => {
            lines.push(format!(
                "WARNING: Graph was built at commit '{}' but HEAD is now '{}'. \
                 Run 'dagayn update' or 'dagayn build' to refresh.",
                short(sha),
                short(&current_sha)
            ));
        }
        _ => {}
    }
    Ok(())
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// The commit, extractor, and diff tiers of the freshness assessment.
pub fn assess_graph_sync(
    store: &GraphStore,
    repo_root: &Path,
) -> Result<SyncAssessment, GraphError> {
    let stats = store.get_stats()?;
    let stored_sha = store
        .get_metadata("git_head_sha")?
        .filter(|v| !v.is_empty());
    let last_updated = stats
        .last_updated
        .clone()
        .or(store.get_metadata("last_updated")?)
        .filter(|v| !v.is_empty());
    let vcs = detect_vcs(repo_root);
    let git = is_git_backed(vcs);
    let (current_branch, current_sha, dirty_files) = if git {
        let (branch, sha) = git_branch_info(repo_root);
        (branch, sha, dirty_files(repo_root))
    } else {
        (String::new(), String::new(), Vec::new())
    };
    let current_branch = (!current_branch.is_empty()).then_some(current_branch);
    let head = (!current_sha.is_empty()).then(|| current_sha.clone());
    let graph_empty = stats.total_nodes == 0 || stats.files_count == 0;
    // A jj working copy jj cannot read (a stale workspace) is no evidence
    // the graph matches it.
    let commit_drift =
        (git && !current_sha.is_empty() && stored_sha.as_deref() != Some(&*current_sha))
            || (vcs == Vcs::Jj && current_sha.is_empty());
    let undated = last_updated.is_none() && !graph_empty;
    let extractor_drift: Vec<String> = if graph_empty {
        Vec::new()
    } else {
        outdated_extractors(store, &stats.languages)?
            .iter()
            .map(|entry| entry.extractor.to_string())
            .collect()
    };

    let assessment = |state, worktree_dirty| SyncAssessment {
        state,
        extractor_drift: extractor_drift.clone(),
        pending_files: Vec::new(),
        worktree_dirty,
        current_head_sha: head.clone(),
        git_head_sha: stored_sha.clone(),
        current_branch: current_branch.clone(),
        last_updated: last_updated.clone(),
        indexed_files: Vec::new(),
        content_verified: true,
        unverified_file_count: 0,
    };
    if graph_empty {
        // Python clears the dirty files of an unbuilt graph.
        return Ok(assessment("unbuilt", false));
    }
    if commit_drift || undated || !extractor_drift.is_empty() {
        return Ok(assessment("commit_drift", !dirty_files.is_empty()));
    }
    let seeded = store
        .get_metadata(SEEDED_NEEDS_VERIFY_KEY)?
        .is_some_and(|value| value == "1");
    let cap = (!seeded).then_some(MAX_HASH_CANDIDATES);
    let tier = classify_diff_tier(store, repo_root, &dirty_files, cap).unwrap_or(DiffTier {
        state: "worktree_behind",
        files: Vec::new(),
        unverified: Some(0),
    });
    if tier.unverified.is_none() {
        // Verified once; the stored mtimes now describe this worktree. Best
        // effort, as in Python: a stale flag only costs a re-verify.
        let _ = store
            .set_metadata(SEEDED_NEEDS_VERIFY_KEY, "0")
            .and_then(|()| store.commit());
    }
    let mut sync = assessment(tier.state, !dirty_files.is_empty());
    match tier.state {
        "worktree_behind" => sync.pending_files = tier.files,
        "worktree_ahead" => sync.indexed_files = tier.files,
        _ => {}
    }
    if let Some(count) = tier.unverified {
        sync.content_verified = false;
        sync.unverified_file_count = count;
    }
    Ok(sync)
}

/// What `_classify_diff_tier` decided.
struct DiffTier {
    state: &'static str,
    /// The files behind for `worktree_behind`, the dirty candidates otherwise.
    files: Vec<String>,
    /// `Some(files left unhashed)` when the content was not verified.
    unverified: Option<usize>,
}

/// `_classify_diff_tier`.
///
/// Every indexed file is stat'ed, not just the files git calls dirty (an edit
/// indexed by a hook and then discarded leaves git clean); bytes are hashed
/// only where the mtime moved, plus dirty files the graph never indexed.
fn classify_diff_tier(
    store: &GraphStore,
    root: &Path,
    dirty_files: &[String],
    max_hash_candidates: Option<usize>,
) -> Result<DiffTier, GraphError> {
    let dirty_state = if dirty_files.is_empty() {
        "commit_synced"
    } else {
        "worktree_ahead"
    };
    let indexed = store.get_file_meta_map()?;
    let mut stale = BTreeSet::new();
    let mut to_hash = BTreeSet::new();
    for (path, (_, stored_mtime)) in &indexed {
        match file_mtime_ns(&root.join(path)) {
            Err(_) => {
                stale.insert(path.clone());
            }
            Ok(mtime) if mtime != *stored_mtime => {
                to_hash.insert(path.clone());
            }
            Ok(_) => {}
        }
    }
    let dirty: Vec<String> = dirty_files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let (candidates, removed) = dagayn_parser::filter_incremental_candidates(
        root,
        &dirty,
        &dagayn_parser::load_ignore_patterns(root),
    );
    stale.extend(
        removed
            .into_iter()
            .filter(|path| indexed.contains_key(path)),
    );
    to_hash.extend(
        candidates
            .iter()
            .filter(|path| !indexed.contains_key(*path))
            .cloned(),
    );
    let mut candidates = candidates;
    candidates.sort();

    if max_hash_candidates.is_some_and(|cap| to_hash.len() > cap) {
        return Ok(DiffTier {
            state: dirty_state,
            files: candidates,
            unverified: Some(to_hash.len()),
        });
    }
    let mut pending = stale;
    for path in to_hash {
        let unchanged = std::fs::read(root.join(&path)).is_ok_and(|source| {
            indexed
                .get(&path)
                .is_some_and(|(hash, _)| *hash == sha256_hex(&source))
        });
        if !unchanged {
            pending.insert(path);
        }
    }
    Ok(if pending.is_empty() {
        DiffTier {
            state: dirty_state,
            files: candidates,
            unverified: None,
        }
    } else {
        DiffTier {
            state: "worktree_behind",
            files: pending.into_iter().collect(),
            unverified: None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs
            .iter()
            .map(|(name, n)| (name.to_string(), *n))
            .collect()
    }

    #[test]
    fn stored_provider_wins_over_the_largest_partition() {
        let counts = counts(&[("old-model", 900), ("new-model#dim=1024", 10)]);
        assert_eq!(
            resolve_active_provider(&counts, Some("NEW-model")).as_deref(),
            Some("new-model#dim=1024")
        );
        assert_eq!(
            resolve_active_provider(&counts, None).as_deref(),
            Some("old-model")
        );
        assert_eq!(
            resolve_active_provider(&counts, Some("absent")).as_deref(),
            Some("absent"),
            "an unmatched stored provider still scopes coverage"
        );
    }

    #[test]
    fn identity_drops_text_and_dimension_suffixes() {
        assert_eq!(provider_identity("Model#dim=768#text=material"), "model");
        assert_eq!(text_mode("m#text=material"), Some("material"));
        assert!(openai_names_match("m", "m#dim=512"));
        assert!(!openai_names_match("m#dim=1", "m#dim=2"));
    }
}
