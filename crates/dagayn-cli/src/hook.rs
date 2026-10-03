//! Guards that keep a hook-triggered `dagayn update` bounded.
//!
//! Port of `dagayn.hook_guard`: editors do not kill the process a hook
//! started, so an update has to stop itself, and a repository can opt out of
//! edit-triggered updates entirely.

use std::path::Path;
use std::time::Duration;

/// Set by generated hooks so an unattended run can be told from a manual one.
const HOOK_UPDATE_ENV: &str = "DAGAYN_HOOK_UPDATE";
/// Opt-out marker under `.dagayn/`, also honoured in the main checkout.
pub(crate) const HOOK_SKIP_MARKER: &str = "hook-skip";
/// Budget for hook-triggered updates when none is given.
const DEFAULT_HOOK_BUDGET_SECONDS: f64 = 120.0;
/// Exit status when the budget runs out; generated hooks append `|| true`.
const BUDGET_EXCEEDED_EXIT_CODE: i32 = 75;

pub(crate) fn running_from_hook() -> bool {
    std::env::var(HOOK_UPDATE_ENV).is_ok_and(|value| !value.is_empty() && value != "0")
}

/// True when the repository, or the main checkout of a linked worktree, holds
/// `.dagayn/hook-skip`.
pub(crate) fn hook_updates_disabled(repo_root: &Path) -> bool {
    let marked = |root: &Path| root.join(".dagayn").join(HOOK_SKIP_MARKER).exists();
    marked(repo_root) || dagayn_build::main_checkout(repo_root).is_some_and(|main| marked(&main))
}

/// `--budget-seconds` (0 or less disables it), else the hook default for hook
/// runs; manual runs are unbounded.
pub(crate) fn update_budget(explicit: Option<f64>) -> Option<Duration> {
    let seconds = match explicit {
        Some(seconds) => seconds,
        None if running_from_hook() => DEFAULT_HOOK_BUDGET_SECONDS,
        None => return None,
    };
    (seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// Exit the process once `budget` is spent. SQLite rolls the interrupted
/// write transaction back, so the graph stays as it was.
pub(crate) fn start_budget_watchdog(budget: Option<Duration>, label: &'static str) {
    let Some(budget) = budget else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(budget);
        eprintln!(
            "dagayn: {label} exceeded its {}s budget and was stopped; the graph is unchanged",
            budget.as_secs_f64()
        );
        std::process::exit(BUDGET_EXCEEDED_EXIT_CODE);
    });
}
