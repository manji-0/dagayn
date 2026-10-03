//! The Rust `dagayn` CLI.
//!
//! Only `build`, `update`, and `status` are implemented so far; every other
//! command still lives in the Python CLI. [`run`] answers
//! [`Outcome::Fallback`] for anything it does not handle the way Python
//! would, decided before it takes the graph lock, touches the database, or
//! prints anything. The installed `dagayn` entry point (through `_core`) then
//! runs the Python CLI with the same arguments; the standalone binary reports
//! the reason and fails.

mod build_cmd;
mod hook;
mod status_cmd;
mod update_cmd;

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, Subcommand};
use dagayn_build::{BuildError, DataDirError, PostprocessLevel};
use dagayn_graph::GraphError;
use serde_json::Value;

const DEFAULT_WRITE_LOCK_TIMEOUT_SECS: f64 = 120.0;

/// Subcommands this crate implements; everything else goes to Python.
const COMMANDS: [&str; 3] = ["build", "update", "status"];

/// Editor workspace hints `dagayn.incremental_files.find_project_root` weighs
/// against the working directory. Not ported, so a run that would consult
/// them falls back.
const WORKSPACE_HINT_ENVS: [&str; 3] = [
    "CURSOR_PROJECT_DIR",
    "CLAUDE_PROJECT_DIR",
    "WORKSPACE_FOLDER_PATHS",
];

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

/// What [`run`] did with a command line.
#[derive(Debug)]
pub enum Outcome {
    /// Handled; the process should exit with this status.
    Exit(u8),
    /// Not handled, and nothing was changed or printed.
    Fallback(Fallback),
}

#[derive(Debug)]
pub enum Fallback {
    /// Not a command line this crate parses: another subcommand, `--help`,
    /// `--version`, or a flag only Python accepts.
    Parse(clap::Error),
    /// Parsed, but needs behaviour that is not ported yet.
    Unsupported(String),
}

/// Run `argv` (program name first) if this crate implements it.
pub fn run<I, T>(argv: I) -> Outcome
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) if !asks_for_help(&argv) => cli,
        Ok(_) => return Outcome::Fallback(Fallback::Unsupported("--help".to_string())),
        Err(err) => return Outcome::Fallback(Fallback::Parse(err)),
    };
    let result = match cli.command {
        Command::Build(args) => build_cmd::run(&args),
        Command::Update(args) => update_cmd::run(&args),
        Command::Status(args) => status_cmd::run(&args),
    };
    let outcome = match result {
        Ok(code) => Outcome::Exit(code),
        Err(Failure::Fallback(reason)) => Outcome::Fallback(Fallback::Unsupported(reason)),
        Err(Failure::Error(message)) => {
            eprintln!("ERROR: {message}");
            Outcome::Exit(1)
        }
    };
    // The embedding interpreter exits without running Rust's stdout cleanup.
    let _ = std::io::stdout().flush();
    outcome
}

/// `-h`/`--help` anywhere clap would still accept, e.g. after `--repo X`.
/// Python's help text lists flags this crate does not have.
fn asks_for_help(argv: &[OsString]) -> bool {
    argv.iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "-h" || arg == "--help")
}

/// Whether `argv` names a subcommand this crate implements, for callers that
/// want to skip loading it otherwise.
pub fn handles_command(argv: &[OsString]) -> bool {
    argv.get(1)
        .and_then(|arg| arg.to_str())
        .is_some_and(|arg| COMMANDS.contains(&arg))
}

/// Why a command did not complete.
pub(crate) enum Failure {
    /// Hand the command line to Python; nothing was changed or printed.
    Fallback(String),
    /// Report the error and exit 1.
    Error(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Error(message)
    }
}

impl From<GraphError> for Failure {
    /// A corrupt database falls back so Python's CLI quarantines it, as an
    /// unattended hook needs to recover instead of failing on every edit.
    fn from(err: GraphError) -> Self {
        if err.is_corrupt() {
            Failure::Fallback(format!("the graph database is corrupt ({err})"))
        } else {
            Failure::Error(err.to_string())
        }
    }
}

impl From<BuildError> for Failure {
    fn from(err: BuildError) -> Self {
        match err {
            BuildError::Graph(err) => err.into(),
            err @ BuildError::UnsupportedVcs(_) => Failure::Fallback(err.to_string()),
        }
    }
}

impl From<DataDirError> for Failure {
    fn from(err: DataDirError) -> Self {
        match err {
            err @ DataDirError::SharedDataDirUnsupported => Failure::Fallback(err.to_string()),
            err => Failure::Error(err.to_string()),
        }
    }
}

pub(crate) fn unsupported(what: &str) -> Failure {
    Failure::Fallback(format!("{what} is not supported by the Rust CLI yet"))
}

/// Refuse what is not ported before anything touches the graph: jj and SVN
/// working copies, and the legacy `.dagayn.db` files `dagayn.paths.get_db_path`
/// migrates or deletes.
pub(crate) fn require_ported_layout(repo_root: &Path) -> Result<(), Failure> {
    match dagayn_build::detect_vcs(repo_root) {
        dagayn_build::Vcs::Jj => return Err(unsupported("a jj workspace")),
        dagayn_build::Vcs::Svn => return Err(unsupported("an SVN working copy")),
        dagayn_build::Vcs::Git | dagayn_build::Vcs::None => {}
    }
    let legacy = ["", "-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| repo_root.join(format!(".dagayn.db{suffix}")).exists());
    if legacy {
        return Err(unsupported("migrating a legacy .dagayn.db"));
    }
    Ok(())
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
/// working directory, else the working directory. A run without either
/// override while an editor workspace hint is set falls back, since Python
/// weighs those hints against the working directory. A root Python would
/// reject falls back too, so the user gets Python's message.
pub(crate) fn resolve_repo_root(explicit: Option<&Path>) -> Result<PathBuf, Failure> {
    let override_root = std::env::var("CRG_REPO_ROOT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && Path::new(value).exists());
    let candidate = match (explicit, override_root) {
        (Some(path), _) => path.to_path_buf(),
        (None, Some(value)) => PathBuf::from(value),
        (None, None) => {
            if let Some(var) = WORKSPACE_HINT_ENVS
                .iter()
                .find(|var| std::env::var_os(var).is_some_and(|value| !value.is_empty()))
            {
                return Err(unsupported(&format!("resolving the repository from {var}")));
            }
            let cwd = std::env::current_dir().map_err(|err| err.to_string())?;
            find_checkout_root(&cwd).unwrap_or(cwd)
        }
    };
    let resolved = candidate
        .canonicalize()
        .ok()
        .filter(|path| path.is_dir())
        .ok_or_else(|| {
            Failure::Fallback(format!(
                "repo_root is not an existing directory: {}",
                candidate.display()
            ))
        })?;
    let is_project_root = resolved.join(".git").exists()
        || resolved.join(".svn").exists()
        || resolved.join(".dagayn").join("graph.db").is_file();
    if !is_project_root {
        return Err(Failure::Fallback(format!(
            "repo_root does not look like a project root (no .git or .dagayn/graph.db found): {}",
            resolved.display()
        )));
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
