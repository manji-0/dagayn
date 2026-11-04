use std::path::PathBuf;
use std::process::Command;

struct Repo(PathBuf);

impl Repo {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-status-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create repo dir");
        std::fs::write(root.join("app.py"), "def main():\n    pass\n").expect("write");
        let repo = Self(root);
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "--no-gpg-sign", "-m", "init"]);
        repo
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

    fn dagayn(&self, command: &str) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_dagayn"))
            .args([command, "--repo", self.0.to_str().expect("utf-8")])
            .env_remove("CRG_DATA_DIR")
            .env_remove("CRG_REPO_ROOT")
            .env_remove("DAGAYN_HOOK_UPDATE")
            .output()
            .expect("run dagayn");
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn sql(&self, statement: &str) {
        let out = Command::new("sqlite3")
            .arg(self.0.join(".dagayn/graph.db"))
            .arg(statement)
            .output()
            .expect("run sqlite3");
        assert!(out.status.success(), "{out:?}");
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn state(status: &str) -> &str {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Graph state: "))
        .unwrap_or("")
}

#[test]
fn status_follows_the_graph_through_its_sync_states() {
    let repo = Repo::new("states");
    assert!(state(&repo.dagayn("status")).starts_with("unbuilt"));

    repo.dagayn("build");
    let built = repo.dagayn("status");
    assert!(built.starts_with("Nodes: 2\n"), "{built}");
    // The build creates the (empty) embeddings schema, as Python's does.
    assert!(
        built.contains("Embeddings: empty (0 vectors, 0 provider(s))"),
        "{built}"
    );
    assert!(built.contains("Built on branch: main"), "{built}");
    assert!(state(&built).starts_with("commit_synced"), "{built}");

    std::fs::write(repo.0.join("app.py"), "def main():\n    return 1\n").expect("edit");
    let behind = repo.dagayn("status");
    assert!(state(&behind).starts_with("worktree_behind"), "{behind}");
    assert!(behind.contains("  Needs re-indexing: app.py"), "{behind}");

    repo.dagayn("update");
    assert!(state(&repo.dagayn("status")).starts_with("worktree_ahead"));

    repo.git(&["commit", "-qam", "edit", "--no-gpg-sign"]);
    let drift = repo.dagayn("status");
    assert!(state(&drift).starts_with("commit_drift"), "{drift}");
    assert!(
        drift.contains("WARNING: Graph was built at commit"),
        "{drift}"
    );

    repo.git(&["checkout", "-q", "-b", "feature"]);
    assert!(
        repo.dagayn("status")
            .contains("but you are now on 'feature'")
    );
}

#[test]
fn status_reports_embedding_coverage_for_the_active_provider() {
    let repo = Repo::new("embeddings");
    repo.dagayn("build");
    repo.sql(
        // The build created the embeddings table.
        "INSERT INTO embeddings VALUES ('app.py::main', x'00', 'h', 'm#dim=4'), \
         ('gone.py::f', x'00', 'h', 'm#dim=4'), ('app.py::main', x'00', 'h', 'old'); \
         INSERT OR REPLACE INTO metadata (key, value) VALUES ('embedding_provider', 'M'), \
         ('extractor_versions', 'python=0');",
    );

    let status = repo.dagayn("status");

    assert!(
        status.contains("Embeddings: stale (3 vectors, 2 provider(s))"),
        "{status}"
    );
    assert!(
        status.contains("  Coverage: 1/1 embeddable nodes (0 missing)"),
        "{status}"
    );
    assert!(status.contains("  Orphans: 1"), "{status}");
    assert!(
        status.contains("  Provider: m#dim=4 (2)\n  Provider: old (1)"),
        "{status}"
    );
    assert!(
        status.contains("  Parsed by an older extractor: "),
        "{status}"
    );
}

/// A queue row as the Python worker leaves it, stamped with SQLite's clock.
fn plant_update_task(repo: &Repo, state: &str, last_error: &str) {
    let out = Command::new("sqlite3")
        .arg(repo.0.join(".dagayn/task_queue.db"))
        .arg(format!(
            "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, \
             priority INTEGER NOT NULL DEFAULT 0, payload TEXT NOT NULL DEFAULT '{{}}', \
             state TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0, \
             created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_error TEXT, \
             not_before REAL NOT NULL DEFAULT 0); \
             INSERT INTO tasks (kind, state, attempts, created_at, updated_at, last_error) \
             VALUES ('update', '{state}', 3, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'), \
             strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'), '{last_error}');"
        ))
        .output()
        .expect("run sqlite3");
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn status_names_a_dead_hook_update_until_the_graph_moves_on() {
    // dagayn: tests crates/dagayn-build/src/status.rs::status_lines
    let repo = Repo::new("hook-queue");
    repo.dagayn("build");
    assert!(!repo.dagayn("status").contains("Hook updates:"));

    std::thread::sleep(std::time::Duration::from_millis(1100));
    plant_update_task(&repo, "dead", "budget exceeded: queue update ran past 120s");
    let stopped = repo.dagayn("status");
    let line = stopped
        .lines()
        .find(|line| line.starts_with("Hook updates: stopped"))
        .unwrap_or_else(|| panic!("{stopped}"));
    assert!(
        line.contains("update task #1 dead after 3 attempt(s)"),
        "{line}"
    );
    assert!(line.contains("budget exceeded"), "{line}");
    assert!(line.contains("'dagayn update'"), "{line}");
    assert!(line.contains(".dagayn/hook-skip"), "{line}");

    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(repo.0.join("app.py"), "def main():\n    return 2\n").expect("edit");
    repo.dagayn("update");
    let resumed = repo.dagayn("status");
    assert!(!resumed.contains("Hook updates:"), "{resumed}");
}
