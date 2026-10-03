use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use dagayn_build::{BuildOptions, GraphWriteLock, PostprocessLevel, db_path_for_build, full_build};
use dagayn_graph::GraphStore;

struct TempRepo(PathBuf);

impl TempRepo {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-build-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp repo");
        Self(root.canonicalize().expect("canonical temp repo"))
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write file");
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    fn commit_all(&self) {
        self.git(&["init", "-q", "-b", "main"]);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--no-gpg-sign", "-m", "init"]);
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn build(repo: &Path, postprocess: PostprocessLevel) -> (GraphStore, dagayn_build::BuildReport) {
    let db_path = db_path_for_build(repo).expect("db path");
    let mut store = GraphStore::open(&db_path).expect("open store");
    let options = BuildOptions {
        recurse_submodules: false,
        postprocess,
    };
    let report = full_build(repo, &mut store, &options).expect("full build");
    (store, report)
}

fn node_names(store: &GraphStore) -> Vec<String> {
    let mut names: Vec<String> = store
        .get_all_nodes()
        .expect("nodes")
        .into_iter()
        .map(|node| node.qualified_name)
        .collect();
    names.sort();
    names
}

#[test]
fn full_build_stores_nodes_metadata_and_postprocess_counters() {
    let repo = TempRepo::new("full");
    repo.write(
        "app.py",
        "def main():\n    helper()\n\n\ndef helper():\n    return 1\n",
    );
    repo.write("docs/guide.md", "# Guide\n\nCall `main` to start.\n");
    repo.commit_all();

    let (store, report) = build(&repo.0, PostprocessLevel::Full);

    assert_eq!(report.files_parsed, 2);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(node_names(&store).contains(&"app.py::helper".to_string()));
    let meta = |key: &str| store.get_metadata(key).expect("metadata");
    assert_eq!(
        meta("repo_root").as_deref(),
        Some(&*repo.0.to_string_lossy())
    );
    assert_eq!(meta("last_build_type").as_deref(), Some("full"));
    assert_eq!(meta("git_branch").as_deref(), Some("main"));
    assert_eq!(meta("git_head_sha").map(|sha| sha.len()), Some(40));
    assert_eq!(meta("postprocess_level").as_deref(), Some("full"));
    assert!(meta("extractor_versions").is_some_and(|stamp| stamp.contains("python=")));
    let counters = report.postprocess.expect("postprocess counters");
    assert!(
        counters
            .get("fts_indexed")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            > 0
    );
    assert!(
        counters.get("warnings").is_none(),
        "warnings move to the report"
    );
    assert!(
        repo.0.join(".dagayn").join(".gitignore").is_file(),
        "the data dir keeps its graph out of commits"
    );
}

#[test]
fn rebuild_drops_files_that_left_the_repository() {
    let repo = TempRepo::new("stale");
    repo.write("a.py", "def a():\n    pass\n");
    repo.write("b.py", "def b():\n    pass\n");
    repo.commit_all();
    drop(build(&repo.0, PostprocessLevel::None));

    repo.git(&["rm", "-q", "b.py"]);
    let (store, report) = build(&repo.0, PostprocessLevel::None);

    assert_eq!(report.files_parsed, 1);
    assert!(report.postprocess.is_none());
    assert!(
        !node_names(&store)
            .iter()
            .any(|name| name.starts_with("b.py"))
    );
    assert_eq!(
        store.get_metadata("postprocess_level").expect("metadata"),
        None
    );
}

#[test]
fn build_outside_version_control_records_no_git_metadata() {
    let repo = TempRepo::new("novcs");
    repo.write("main.tf", "variable \"region\" {}\n");

    let (store, report) = build(&repo.0, PostprocessLevel::Full);

    assert_eq!(report.files_parsed, 1);
    assert_eq!(store.get_metadata("git_head_sha").expect("metadata"), None);
}

#[test]
fn write_lock_excludes_a_second_writer_until_dropped() {
    let repo = TempRepo::new("lock");
    let db_path = db_path_for_build(&repo.0).expect("db path");

    let held = GraphWriteLock::acquire(&db_path, Duration::from_secs(1)).expect("first lock");
    let contended = GraphWriteLock::acquire(&db_path, Duration::from_millis(150));
    assert!(contended.is_err(), "a second exclusive lock must wait");
    drop(held);
    GraphWriteLock::acquire(&db_path, Duration::from_secs(1)).expect("lock after release");
}
