use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Repo(PathBuf);

impl Repo {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-update-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create repo dir");
        let repo = Self(root);
        repo.write(
            "app.py",
            "def main():\n    helper()\n\n\ndef helper():\n    return 1\n",
        );
        repo.write(
            "lib.py",
            "from app import helper\n\n\ndef use():\n    helper()\n",
        );
        repo.git(&["init", "-q", "-b", "main"]);
        repo.commit("init");
        repo
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 path")
    }

    fn write(&self, rel: &str, body: &str) {
        std::fs::write(self.0.join(rel), body).expect("write file");
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--no-gpg-sign", "-m", message]);
    }

    fn dagayn(&self, args: &[&str], hook: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dagayn"));
        cmd.args(args)
            .env_remove("CRG_DATA_DIR")
            .env_remove("CRG_REPO_ROOT");
        if hook {
            cmd.env("DAGAYN_HOOK_UPDATE", "1");
        } else {
            cmd.env_remove("DAGAYN_HOOK_UPDATE");
        }
        cmd.output().expect("run dagayn")
    }

    fn update(&self, extra: &[&str]) -> String {
        let mut args = vec!["update", "--repo", self.path()];
        args.extend_from_slice(extra);
        let out = self.dagayn(&args, false);
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn query(&self, sql: &str) -> String {
        let out = Command::new("sqlite3")
            .arg(self.0.join(".dagayn/graph.db"))
            .arg(sql)
            .output()
            .expect("run sqlite3");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn built(label: &str) -> Repo {
    let repo = Repo::new(label);
    let out = repo.dagayn(&["build", "--repo", repo.path()], false);
    assert!(out.status.success(), "{out:?}");
    repo
}

#[test]
fn update_with_nothing_changed_is_a_no_op() {
    let repo = built("noop");
    let stdout = repo.update(&["--skip-flows"]);
    assert_eq!(
        stdout.trim(),
        "Incremental: 0 files updated, 0 nodes, 0 edges (postprocess=minimal)"
    );
}

#[test]
fn edited_file_and_its_dependents_are_reparsed() {
    let repo = built("edit");
    repo.write(
        "app.py",
        "def main():\n    helper()\n\n\ndef helper():\n    return 2\n\n\ndef added():\n    pass\n",
    );

    let stdout = repo.update(&[]);

    assert!(
        stdout.starts_with("Incremental: 2 files updated"),
        "{stdout}"
    );
    assert!(stdout.contains("(postprocess=full)"), "{stdout}");
    assert_eq!(
        repo.query("SELECT count(*) FROM nodes WHERE qualified_name = 'app.py::added'"),
        "1"
    );
    assert_eq!(
        repo.query("SELECT value FROM metadata WHERE key = 'last_build_type'"),
        "incremental"
    );
    // Unchanged content after the first update: nothing to do.
    assert!(repo.update(&[]).starts_with("Incremental: 0 files updated"));
}

#[test]
fn committed_deletion_removes_nodes_and_moves_the_recorded_head() {
    let repo = built("delete");
    let before = repo.query("SELECT value FROM metadata WHERE key = 'git_head_sha'");
    std::fs::remove_file(repo.0.join("lib.py")).expect("delete");
    repo.commit("drop lib");

    repo.update(&["--skip-postprocess"]);

    assert_eq!(
        repo.query("SELECT count(*) FROM nodes WHERE file_path = 'lib.py'"),
        "0"
    );
    let after = repo.query("SELECT value FROM metadata WHERE key = 'git_head_sha'");
    assert_ne!(before, after, "the graph now describes the new HEAD");
}

#[test]
fn hook_run_respects_the_skip_marker() {
    let repo = built("marker");
    std::fs::write(repo.0.join(".dagayn/hook-skip"), "").expect("marker");
    let out = repo.dagayn(&["update", "--repo", repo.path()], true);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Skipped: .dagayn/hook-skip"));
}

#[test]
fn hook_run_skips_instead_of_waiting_for_a_writer() {
    let repo = built("busy");
    let db = Path::new(repo.path()).join(".dagayn/graph.db");
    let _held = dagayn_build_lock(&db);
    let out = repo.dagayn(&["update", "--repo", repo.path()], true);
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("Skipped: another process is writing"),
        "{out:?}"
    );
}

/// Hold the graph's write lock the way Python does: `flock` on the lock file.
fn dagayn_build_lock(db: &Path) -> std::fs::File {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(db.with_file_name("graph.db.write.lock"))
        .expect("open lock");
    // SAFETY: flock on a descriptor this test owns.
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    file
}
