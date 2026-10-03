//! The standalone `dagayn` binary.
//!
//! Only `build` is implemented so far; every other command still lives in the
//! Python CLI. Flags the Python `build` accepts but this one does not port yet
//! fail loudly instead of being ignored.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use dagayn_build::{BuildOptions, GraphWriteLock, PostprocessLevel, db_path_for_build};
use dagayn_graph::GraphStore;
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
    Build(BuildArgs),
}

#[derive(Args)]
struct BuildArgs {
    /// Repository root (auto-detected).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Delete the existing graph database before rebuilding.
    #[arg(long = "force-full-build", visible_alias = "force")]
    force: bool,
    /// Skip flow/community detection (signatures + FTS only).
    #[arg(long)]
    skip_flows: bool,
    /// Skip all post-processing (raw parse only).
    #[arg(long)]
    skip_postprocess: bool,
    /// Settle call targets with SCIP indexers (not supported yet).
    #[arg(long)]
    scip: bool,
    /// Local embeddings (not supported yet).
    #[arg(long, num_args = 0..=1, default_missing_value = "bge-m3")]
    local_embedding: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Build(args) => run_build(&args),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("ERROR: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_build(args: &BuildArgs) -> Result<ExitCode, String> {
    if args.scip {
        return Err(unsupported("--scip"));
    }
    if args
        .local_embedding
        .as_deref()
        .is_some_and(|mode| mode != "none")
    {
        return Err(unsupported("--local-embedding"));
    }
    let postprocess = if args.skip_postprocess {
        PostprocessLevel::None
    } else if args.skip_flows {
        return Err(unsupported("--skip-flows"));
    } else {
        PostprocessLevel::Full
    };

    let repo_root = resolve_repo_root(args.repo.as_deref())?;
    let db_path = db_path_for_build(&repo_root).map_err(|err| err.to_string())?;
    let _lock =
        GraphWriteLock::acquire(&db_path, write_lock_timeout()).map_err(|err| err.to_string())?;
    if args.force {
        remove_database(&db_path)?;
    }
    let mut store = GraphStore::open(&db_path).map_err(|err| err.to_string())?;
    let options = BuildOptions {
        recurse_submodules: env_flag("CRG_RECURSE_SUBMODULES"),
        postprocess,
    };
    let report = dagayn_build::full_build(&repo_root, &mut store, &options)
        .map_err(|err| err.to_string())?;
    drop(store);

    println!(
        "Full build: {} files, {} nodes, {} edges (postprocess={})",
        report.files_parsed,
        report.total_nodes,
        report.total_edges,
        postprocess.as_str()
    );
    if !report.errors.is_empty() {
        println!("Errors: {}", report.errors.len());
    }
    if let Some(counters) = &report.postprocess {
        print_postprocess_summary(counters);
    }
    for warning in &report.warnings {
        eprintln!("WARNING: {warning}");
    }
    Ok(ExitCode::SUCCESS)
}

fn unsupported(flag: &str) -> String {
    format!("{flag} is not supported by this build of dagayn yet; use the Python CLI")
}

/// `_print_postprocess_summary` in `dagayn.cli.commands.build_handlers`.
fn print_postprocess_summary(counters: &Value) {
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
fn resolve_repo_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
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

fn remove_database(db_path: &Path) -> Result<(), String> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = db_path.as_os_str().to_os_string();
        name.push(suffix);
        match std::fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("cannot delete {}: {err}", db_path.display())),
        }
    }
    Ok(())
}

fn write_lock_timeout() -> Duration {
    let seconds = std::env::var("DAGAYN_WRITE_LOCK_TIMEOUT")
        .ok()
        .and_then(|raw| raw.parse::<f64>().ok())
        .unwrap_or(DEFAULT_WRITE_LOCK_TIMEOUT_SECS);
    Duration::from_secs_f64(seconds.max(0.0))
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}
