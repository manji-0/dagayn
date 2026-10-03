//! Command lines the Rust CLI hands back to Python: [`dagayn_cli::run`]
//! answers `Fallback` without touching the repository, and the standalone
//! binary reports why.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use dagayn_cli::{Fallback, Outcome};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-cli-fallback-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".git")).expect("create temp project");
        std::fs::write(root.join("app.py"), "def main():\n    pass\n").expect("write source");
        Self(root)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 path")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dagayn(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dagayn"));
    command
        .args(args)
        .env_remove("CRG_DATA_DIR")
        .env_remove("CRG_REPO_ROOT")
        .env_remove("CURSOR_PROJECT_DIR")
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("WORKSPACE_FOLDER_PATHS");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run dagayn")
}

fn assert_fell_back(out: &Output, reason: &str) {
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "nothing is printed before falling back"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(reason), "{stderr}");
    assert!(stderr.contains("use the Python CLI"), "{stderr}");
}

fn falls_back_on_parse(argv: &[&str]) -> bool {
    matches!(
        dagayn_cli::run(argv.iter().copied()),
        Outcome::Fallback(Fallback::Parse(_))
    )
}

#[test]
fn other_commands_help_and_version_go_to_python() {
    for argv in [
        &["dagayn"][..],
        &["dagayn", "--version"],
        &["dagayn", "-v"],
        &["dagayn", "serve"],
        &["dagayn", "build", "--help"],
        &["dagayn", "status", "-h"],
        &["dagayn", "update", "--local-embedding-port", "8080"],
        // argparse accepts unambiguous prefixes; clap does not.
        &["dagayn", "update", "--skip-f"],
    ] {
        assert!(falls_back_on_parse(argv), "{argv:?}");
    }
    let argv: Vec<std::ffi::OsString> = ["dagayn", "status"].map(Into::into).into();
    assert!(dagayn_cli::handles_command(&argv));
    let argv: Vec<std::ffi::OsString> = ["dagayn", "serve"].map(Into::into).into();
    assert!(!dagayn_cli::handles_command(&argv));
}

#[test]
fn a_workspace_hint_without_a_repo_goes_to_python() {
    let project = TempDir::new("hint");
    let out = Command::new(env!("CARGO_BIN_EXE_dagayn"))
        .arg("status")
        .current_dir(&project.0)
        .env_remove("CRG_REPO_ROOT")
        .env("CLAUDE_PROJECT_DIR", project.path())
        .output()
        .expect("run dagayn");
    assert_fell_back(&out, "CLAUDE_PROJECT_DIR");
    assert!(!project.0.join(".dagayn").exists());

    // `--repo` settles the root, so the hint no longer matters.
    let out = dagayn(
        &["status", "--repo", project.path()],
        &[("CLAUDE_PROJECT_DIR", project.path())],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `dagayn <command>` with no `--repo`, run from `cwd`.
fn implicit(command: &str, cwd: &Path, envs: &[(&str, &str)]) -> Output {
    let mut run = Command::new(env!("CARGO_BIN_EXE_dagayn"));
    run.arg(command).current_dir(cwd);
    for var in [
        "CRG_DATA_DIR",
        "CRG_REPO_ROOT",
        "CURSOR_PROJECT_DIR",
        "CLAUDE_PROJECT_DIR",
        "WORKSPACE_FOLDER_PATHS",
        "DAGAYN_ALLOW_WIDE_ROOT",
    ] {
        run.env_remove(var);
    }
    for (key, value) in envs {
        run.env(key, value);
    }
    run.output().expect("run dagayn")
}

#[test]
fn without_repo_only_a_plain_git_checkout_is_handled() {
    let project = TempDir::new("implicit");
    let nested = project.0.join("src").join("pkg");
    std::fs::create_dir_all(&nested).expect("create subdirectory");
    let out = implicit("status", &nested, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // `update` ignores CRG_REPO_ROOT in Python while `build` honours it.
    let out = implicit("update", &nested, &[("CRG_REPO_ROOT", project.path())]);
    assert_fell_back(&out, "CRG_REPO_ROOT");

    // Python stops at a nested jj workspace instead of the outer checkout.
    let workspace = project.0.join("ws");
    std::fs::create_dir_all(workspace.join(".jj")).expect("create jj workspace");
    let out = implicit("update", &workspace, &[]);
    assert_fell_back(&out, "outside a git checkout");

    // The home directory is refused unless DAGAYN_ALLOW_WIDE_ROOT is set.
    let out = implicit("status", &project.0, &[("HOME", project.path())]);
    assert_fell_back(&out, "as the repository root");
    let out = implicit(
        "status",
        &project.0,
        &[("HOME", project.path()), ("DAGAYN_ALLOW_WIDE_ROOT", "1")],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_legacy_database_goes_to_python_to_migrate() {
    let project = TempDir::new("legacy");
    std::fs::write(project.0.join(".dagayn.db"), b"").expect("write legacy db");
    for command in ["build", "update", "status"] {
        let out = dagayn(&[command, "--repo", project.path()], &[]);
        assert_fell_back(&out, "legacy .dagayn.db");
    }
    assert!(!project.0.join(".dagayn").exists());
}

#[test]
fn a_corrupt_database_goes_to_python_to_quarantine() {
    let project = TempDir::new("corrupt");
    let db = project.0.join(".dagayn").join("graph.db");
    std::fs::create_dir_all(db.parent().expect("data dir")).expect("create data dir");
    let garbage = vec![0x5a_u8; 8192];
    std::fs::write(&db, &garbage).expect("write garbage");
    for command in ["build", "update", "status"] {
        let out = dagayn(&[command, "--repo", project.path()], &[]);
        assert_fell_back(&out, "corrupt");
        assert_eq!(read(&db), garbage, "{command} leaves the file for Python");
    }
}

#[test]
fn a_shared_data_dir_goes_to_python() {
    let project = TempDir::new("datadir");
    let out = dagayn(
        &["update", "--repo", project.path()],
        &[("CRG_DATA_DIR", project.path())],
    );
    assert_fell_back(&out, "CRG_DATA_DIR");
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read file")
}
