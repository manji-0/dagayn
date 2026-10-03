use std::path::PathBuf;
use std::process::{Command, Output};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root =
            std::env::temp_dir().join(format!("dagayn-cli-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create temp dir");
        Self(root)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dagayn(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dagayn"))
        .args(args)
        .env_remove("CRG_DATA_DIR")
        .env_remove("CRG_REPO_ROOT")
        .output()
        .expect("run dagayn")
}

fn project(label: &str) -> TempDir {
    let dir = TempDir::new(label);
    std::fs::create_dir(dir.0.join(".git")).expect("mark project root");
    std::fs::write(dir.0.join("app.py"), "def main():\n    pass\n").expect("write source");
    dir
}

#[test]
fn build_reports_counts_and_postprocess_summary() {
    let dir = project("ok");
    let out = dagayn(&["build", "--repo", dir.0.to_str().expect("utf-8 path")]);

    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("Full build: 1 files, "),
        "unexpected output: {stdout}"
    );
    assert!(stdout.contains("(postprocess=full)"), "{stdout}");
    assert!(stdout.contains("Communities: "), "{stdout}");
    assert!(dir.0.join(".dagayn").join("graph.db").is_file());
}

#[test]
fn force_rebuild_replaces_the_database() {
    let dir = project("force");
    let repo = dir.0.to_str().expect("utf-8 path");
    assert!(dagayn(&["build", "--repo", repo]).status.success());
    std::fs::remove_file(dir.0.join("app.py")).expect("remove source");
    std::fs::write(dir.0.join("lib.py"), "def lib():\n    pass\n").expect("write source");

    let out = dagayn(&["build", "--repo", repo, "--force", "--skip-postprocess"]);

    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Full build: 1 files"), "{stdout}");
    assert!(stdout.contains("(postprocess=none)"), "{stdout}");
}

#[test]
fn unported_flags_fail_instead_of_being_ignored() {
    let dir = project("flags");
    let repo = dir.0.to_str().expect("utf-8 path");
    for flag in ["--scip", "--local-embedding"] {
        let out = dagayn(&["build", "--repo", repo, flag]);
        assert!(!out.status.success(), "{flag} should fail");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("not supported"), "{flag}: {stderr}");
    }
}

#[test]
fn a_directory_that_is_not_a_project_is_refused() {
    let dir = TempDir::new("noproject");
    let out = dagayn(&["build", "--repo", dir.0.to_str().expect("utf-8 path")]);

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not look like a project root"),
        "{stderr}"
    );
    assert!(
        !dir.0.join(".dagayn").exists(),
        "nothing is written on refusal"
    );
}

#[test]
fn a_graph_that_describes_another_repository_is_refused() {
    let first = project("first");
    let second = project("second");
    assert!(
        dagayn(&["build", "--repo", first.0.to_str().expect("utf-8")])
            .status
            .success()
    );
    std::fs::create_dir_all(second.0.join(".dagayn")).expect("data dir");
    std::fs::copy(
        first.0.join(".dagayn/graph.db"),
        second.0.join(".dagayn/graph.db"),
    )
    .expect("copy graph");

    let out = dagayn(&["build", "--repo", second.0.to_str().expect("utf-8")]);

    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("describes"),
        "{out:?}"
    );
}
