//! The standalone `dagayn` binary.
//!
//! Only `build`, `update`, and `status` are implemented so far; every other command
//! still lives in the Python CLI. Flags the Python commands accept but these
//! do not port yet fail loudly instead of being ignored.

mod build_cmd;
mod hook;
mod status_cmd;
mod update_cmd;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use dagayn_build::PostprocessLevel;
use serde_json::Value;

const DEFAULT_WRITE_LOCK_TIMEOUT_SECS: f64 = 120.0;

#[derive(Parser)]
#[command(
    name = "dagayn",
    version,
    about = "Code knowledge graph for AI coding tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Full rebuild of the graph, then post-processing.
    Build(build_cmd::BuildArgs),
    /// Incremental update (only changed files).
    Update(update_cmd::UpdateArgs),
    /// Show graph statistics.
    Status(status_cmd::StatusArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Build(args) => build_cmd::run(&args),
        Command::Update(args) => update_cmd::run(&args),
        Command::Status(args) => status_cmd::run(&args),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("ERROR: {message}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn unsupported(flag: &str) -> String {
    format!("{flag} is not supported by this build of dagayn yet; use the Python CLI")
}

/// `_print_postprocess_summary` in `dagayn.cli.commands.build_handlers`.
pub(crate) fn print_postprocess_summary(counters: &Value) {
    let count = |key: &str| counters.get(key).and_then(Value::as_i64);
    if let Some(n) = count("signatures_computed").filter(|n| *n != 0) {
        println!("Signatures: {n} nodes");
    }
    if let Some(n) = count("fts_indexed").filter(|n| *n != 0) {
        println!("FTS indexed: {n} nodes");
    }
    if let Some(n) = count("flows_detected") {
        println!("Flows: {n}");
    }
    if let Some(n) = count("communities_detected") {
        println!("Communities: {n}");
    }
}

/// `--repo`, else `CRG_REPO_ROOT`, else the nearest ancestor checkout of the
/// working directory, else the working directory. Editor workspace hints
/// (`CLAUDE_PROJECT_DIR`, ...) that the Python CLI also consults are not ported.
pub(crate) fn resolve_repo_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let candidate = match explicit {
        Some(path) => path.to_path_buf(),
        None => match std::env::var("CRG_REPO_ROOT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty() && Path::new(value).exists())
        {
            Some(value) => PathBuf::from(value),
            None => {
                let cwd = std::env::current_dir().map_err(|err| err.to_string())?;
                find_checkout_root(&cwd).unwrap_or(cwd)
            }
        },
    };
    let resolved = candidate.canonicalize().map_err(|_| {
        format!(
            "repo_root is not an existing directory: {}",
            candidate.display()
        )
    })?;
    if !resolved.is_dir() {
        return Err(format!(
            "repo_root is not an existing directory: {}",
            resolved.display()
        ));
    }
    let is_project_root = resolved.join(".git").exists()
        || resolved.join(".svn").exists()
        || resolved.join(".dagayn").join("graph.db").is_file();
    if !is_project_root {
        return Err(format!(
            "repo_root does not look like a project root (no .git or .dagayn/graph.db found): {}",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn find_checkout_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

pub(crate) fn write_lock_timeout() -> Duration {
    let seconds = std::env::var("DAGAYN_WRITE_LOCK_TIMEOUT")
        .ok()
        .and_then(|raw| raw.parse::<f64>().ok())
        .unwrap_or(DEFAULT_WRITE_LOCK_TIMEOUT_SECS);
    Duration::from_secs_f64(seconds.max(0.0))
}

pub(crate) fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// `--skip-postprocess` wins over `--skip-flows`, as in Python.
pub(crate) fn postprocess_level(skip_flows: bool, skip_postprocess: bool) -> PostprocessLevel {
    if skip_postprocess {
        PostprocessLevel::None
    } else if skip_flows {
        PostprocessLevel::Minimal
    } else {
        PostprocessLevel::Full
    }
}

pub(crate) fn print_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("WARNING: {warning}");
    }
}
