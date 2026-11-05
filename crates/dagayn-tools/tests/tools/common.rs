//! What the tool tests share: a scratch repository and calls by tool name.

use std::path::PathBuf;
use std::process::Command;

use dagayn_build::{
    BuildOptions, PostprocessLevel, UpdateOptions, db_path_for_build, full_build,
    incremental_update,
};
use dagayn_graph::GraphStore;
use dagayn_tools::{Context, call};
use serde_json::{Value, json};

pub(crate) struct Repo(pub(crate) PathBuf);

impl Repo {
    pub(crate) fn new(label: &str, git: bool) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-tools-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create repo");
        let root = root.canonicalize().expect("canonical repo");
        let repo = Self(root);
        repo.write(
            "app.py",
            "def main():\n    return helper()\n\n\ndef helper():\n    pass\n",
        );
        repo.write(
            "test_app.py",
            "from app import main\n\n\ndef test_main():\n    main()\n",
        );
        repo.write(
            "docs/LLM-OPTIMIZED-REFERENCE.md",
            "<section name=\"trust\">\n  Graph reach is not correctness.\n</section>\n",
        );
        if git {
            for args in [
                &["init", "-q", "-b", "main"][..],
                &["add", "-A"],
                &["commit", "-q", "--no-gpg-sign", "-m", "init"],
            ] {
                let out = Command::new("git")
                    .args(args)
                    .current_dir(&repo.0)
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .output()
                    .expect("git");
                assert!(out.status.success(), "git {args:?}: {out:?}");
            }
        } else {
            std::fs::create_dir_all(repo.0.join(".git")).expect("mark project root");
        }
        repo
    }

    pub(crate) fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    pub(crate) fn build(&self) {
        let db = db_path_for_build(&self.0).expect("db path");
        let mut store = GraphStore::open(&db).expect("store");
        full_build(
            &self.0,
            &mut store,
            &BuildOptions {
                recurse_submodules: false,
                postprocess: PostprocessLevel::Full,
            },
        )
        .expect("build");
    }

    pub(crate) fn update(&self, base: &str) {
        let db = db_path_for_build(&self.0).expect("db path");
        let mut store = GraphStore::open(&db).expect("store");
        incremental_update(
            &self.0,
            &mut store,
            &UpdateOptions {
                base: base.to_string(),
                postprocess: PostprocessLevel::Full,
                recurse_submodules: false,
            },
        )
        .expect("update");
    }

    pub(crate) fn context(&self) -> Context {
        Context {
            pinned_repo: Some(self.0.clone()),
            ..Context::default()
        }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Each test that calls a tool by name declares the function that tool
// dispatches to (`dagayn: tests`): `dagayn_tools::call` picks it by the
// name, which the call graph cannot follow (docs/plans/TEST-REACH-TARGET.md).
pub(crate) fn answer(context: &Context, name: &str, arguments: Value) -> Value {
    let payload = call(context, name, &arguments)
        .unwrap_or_else(|| panic!("{name} {arguments} was not answered"));
    let parsed: Value = serde_json::from_str(&payload.text).expect("text is JSON");
    assert_eq!(parsed, payload.value, "text and value agree");
    payload.value
}

pub(crate) fn declines(context: &Context, name: &str, arguments: Value) -> bool {
    call(context, name, &arguments).is_none()
}

/// A context that queues repairs with an interpreter that cannot start, so
/// no worker ever runs.
pub(crate) fn auto_preparing(repo: &Repo) -> Context {
    Context {
        package_root: Some(repo.0.clone()),
        auto_prepare: Some(dagayn_tools::AutoPrepare {
            python_executable: Some(PathBuf::from("/nonexistent/python")),
            budget_seconds: Some(300),
        }),
        ..repo.context()
    }
}

/// `git add -A && git commit` in `repo`.
pub(crate) fn commit_all(repo: &Repo, message: &str) {
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "--no-gpg-sign", "-m", message],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(&repo.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
}

pub(crate) fn counts_as_tested(repo: &Repo, target: &str) -> bool {
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": target}),
    );
    reply["test_reach"]["counts_as_tested"].as_bool() == Some(true)
}
