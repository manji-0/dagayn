//! Base-side symbols: helpers, and scenarios on real git repositories whose
//! graphs are built with the full build and the incremental update path.

use std::path::PathBuf;

use dagayn_build::{
    BuildOptions, PostprocessLevel, UpdateOptions, change_file_sources, db_path_for_build,
    full_build, incremental_update,
};

use super::*;

#[test]
fn module_forms_cover_dotted_and_rust_paths() {
    let forms = module_forms("pkg/gone.py");
    assert!(forms.contains(&"pkg.gone".to_string()), "{forms:?}");
    assert!(!forms.contains(&"gone".to_string()), "{forms:?}");
    let forms = module_forms("pkg/sub/__init__.py");
    assert!(forms.contains(&"pkg.sub".to_string()), "{forms:?}");
    let forms = module_forms("src/net/util.rs");
    assert!(forms.contains(&"crate::net::util".to_string()), "{forms:?}");
    assert_eq!(module_forms("util.py"), vec!["crate::util", "util"]);
}

#[test]
fn svn_base_revision_takes_the_range_start() {
    assert_eq!(svn_base_revision("r5:HEAD"), "5");
    assert_eq!(svn_base_revision("12"), "12");
    assert_eq!(svn_base_revision("HEAD~1"), "BASE");
}

#[test]
fn unsafe_refs_and_paths_are_refused() {
    assert!(is_safe_base("HEAD~1"));
    assert!(!is_safe_base("-p"));
    assert!(!is_safe_base("HEAD:x"));
    assert!(is_safe_repo_path("src/a.rs"));
    assert!(!is_safe_repo_path("../a.rs"));
    assert!(!is_safe_repo_path("/etc/passwd"));
    assert!(!is_safe_repo_path("-a"));
}

#[test]
fn qualified_names_rebase_only_their_file_prefix() {
    assert_eq!(
        rebase_qualified("new.py::f", "new.py", "old.py"),
        "old.py::f"
    );
    assert_eq!(rebase_qualified("new.py", "new.py", "old.py"), "old.py");
    assert_eq!(
        rebase_qualified("new.pyx::f", "new.py", "old.py"),
        "new.pyx::f"
    );
}

#[test]
fn renamed_paths_pair_once() {
    let renames = HashMap::from([("new.py".to_string(), "old.py".to_string())]);
    let changed = ["old.py", "new.py", "a.py"].map(str::to_string);
    assert_eq!(
        file_pairs(&changed, &renames),
        vec![
            ("old.py".to_string(), "new.py".to_string()),
            ("a.py".to_string(), "a.py".to_string()),
        ]
    );
}

/// A throwaway git repository with a graph.
struct Repo(PathBuf);

impl Repo {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-base-symbols-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create repo");
        let repo = Self(root.canonicalize().expect("canonical repo"));
        repo.git(&["init", "-q", "-b", "main"]);
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
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    fn remove(&self, rel: &str) {
        std::fs::remove_file(self.0.join(rel)).expect("remove");
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--no-gpg-sign", "-m", message]);
    }

    fn store(&self) -> GraphStore {
        GraphStore::open(db_path_for_build(&self.0).expect("db path")).expect("store")
    }

    fn full_build(&self) {
        full_build(
            &self.0,
            &mut self.store(),
            &BuildOptions {
                recurse_submodules: false,
                postprocess: PostprocessLevel::Full,
            },
        )
        .expect("build");
    }

    fn update(&self, base: &str) {
        incremental_update(
            &self.0,
            &mut self.store(),
            &UpdateOptions {
                base: base.to_string(),
                postprocess: PostprocessLevel::Full,
                recurse_submodules: false,
            },
        )
        .expect("update");
    }

    /// What `review_tool` would ask: the change set against `base`, its
    /// delta, and the references into it.
    fn analyze(&self, base: &str) -> (Vec<String>, SymbolDelta, Vec<Reference>) {
        let changed: Vec<String> = change_file_sources(&self.0, base)
            .expect("changes")
            .files
            .into_iter()
            .filter(|path| !path.starts_with(".dagayn"))
            .collect();
        let delta = symbol_delta(&self.0, base, &changed);
        let references = references_to(&self.store(), &delta.reference_targets(), &changed);
        (changed, delta, references)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug)]
enum Index {
    /// Graph built at base, then `dagayn update` over the change.
    Incremental,
    /// Graph rebuilt from scratch after the change.
    Full,
}

const BOTH: [Index; 2] = [Index::Incremental, Index::Full];

/// Builds the graph at the current state, applies `change` (committed when
/// `commit` is set), and reindexes the way `index` says. Returns the base the
/// review compares against.
fn change_and_index(repo: &Repo, index: Index, commit: bool, change: impl FnOnce(&Repo)) -> String {
    repo.full_build();
    change(repo);
    let base = if commit {
        repo.commit("change");
        "HEAD~1"
    } else {
        "HEAD"
    };
    match index {
        Index::Incremental => repo.update(base),
        Index::Full => repo.full_build(),
    }
    base.to_string()
}

fn removed_names(delta: &SymbolDelta) -> Vec<&str> {
    delta
        .removed
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

fn refs_to<'a>(references: &'a [Reference], target: &str) -> Vec<&'a Reference> {
    references
        .iter()
        .filter(|reference| reference.target == target)
        .collect()
}

fn python_repo(label: &str) -> Repo {
    let repo = Repo::new(label);
    repo.write(
        "pkg/lib.py",
        "def helper(x):\n    return x + 1\n\n\ndef other():\n    return 1\n",
    );
    repo.write(
        "app.py",
        "from pkg.lib import helper\n\n\ndef main():\n    return helper(1)\n",
    );
    repo.write(
        "test_lib.py",
        "from pkg.lib import other\n\n\ndef test_other():\n    assert other() == 1\n",
    );
    repo.commit("init");
    repo
}

#[test]
fn python_function_deleted_with_a_caller_elsewhere() {
    for commit in [true, false] {
        for index in BOTH {
            let repo = python_repo("py-delete");
            let base = change_and_index(&repo, index, commit, |repo| {
                repo.write("pkg/lib.py", "def other():\n    return 1\n");
            });
            let (changed, delta, references) = repo.analyze(&base);
            assert_eq!(changed, vec!["pkg/lib.py"], "{index:?} commit={commit}");
            assert_eq!(removed_names(&delta), vec!["pkg/lib.py::helper"]);
            assert!(delta.signature_changed.is_empty());
            let refs = refs_to(&references, "pkg/lib.py::helper");
            let call = refs
                .iter()
                .find(|r| r.edge_kind == "CALLS")
                .unwrap_or_else(|| panic!("{index:?} commit={commit}: {references:?}"));
            assert_eq!(call.source_qualified, "app.py::main");
            assert_eq!(call.file_path, "app.py");
            assert_eq!(call.line, 5);
            assert_eq!(call.matched_by, MatchKind::ExactTarget);
            assert!(
                refs.iter()
                    .any(|r| r.edge_kind == "IMPORTS_FROM"
                        && r.matched_by == MatchKind::ImportedName),
                "{references:?}"
            );
            // `other` survived, so the test importing it is no reference.
            assert!(
                references.iter().all(|r| r.file_path == "app.py"),
                "{references:?}"
            );
        }
    }
}

fn rust_repo(label: &str) -> Repo {
    let repo = Repo::new(label);
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    );
    repo.write(
        "src/util.rs",
        "pub fn gone() -> u32 {\n    1\n}\n\npub fn kept() -> u32 {\n    2\n}\n",
    );
    repo.write(
        "src/main.rs",
        "mod util;\nuse crate::util::gone;\n\nfn main() {\n    let _ = gone() + util::kept();\n}\n",
    );
    repo.commit("init");
    repo
}

#[test]
fn rust_fn_deleted_with_a_use_and_call_in_another_module() {
    for index in BOTH {
        let repo = rust_repo("rs-delete");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.write("src/util.rs", "pub fn kept() -> u32 {\n    2\n}\n");
        });
        let (_, delta, references) = repo.analyze(&base);
        assert_eq!(removed_names(&delta), vec!["src/util.rs::gone"]);
        let refs = refs_to(&references, "src/util.rs::gone");
        let call = refs
            .iter()
            .find(|r| r.edge_kind == "CALLS")
            .unwrap_or_else(|| panic!("{index:?}: {references:?}"));
        assert_eq!(call.source_qualified, "src/main.rs::main");
        // The update keeps the qualified target; a rebuild leaves the bare
        // name, tied back through the `use`.
        let expected = match index {
            Index::Incremental => MatchKind::ExactTarget,
            Index::Full => MatchKind::BareNameViaImport,
        };
        assert_eq!(call.matched_by, expected, "{references:?}");
        assert!(
            refs.iter()
                .any(|r| r.edge_kind == "IMPORTS_FROM" && r.matched_by == MatchKind::ImportedName),
            "{references:?}"
        );
    }
}

#[test]
fn renamed_function_with_a_stale_caller() {
    for index in BOTH {
        let repo = python_repo("py-rename");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.write(
                "pkg/lib.py",
                "def helper_v2(x):\n    return x + 1\n\n\ndef other():\n    return 1\n",
            );
        });
        let (_, delta, references) = repo.analyze(&base);
        assert_eq!(removed_names(&delta), vec!["pkg/lib.py::helper"]);
        assert_eq!(delta.renamed_candidates.len(), 1, "{delta:?}");
        let rename = &delta.renamed_candidates[0];
        assert_eq!(rename.from.qualified_name, "pkg/lib.py::helper");
        assert_eq!(rename.to_qualified_name, "pkg/lib.py::helper_v2");
        assert!(
            refs_to(&references, "pkg/lib.py::helper")
                .iter()
                .any(|r| r.edge_kind == "CALLS" && r.file_path == "app.py"),
            "{index:?}: {references:?}"
        );
    }
}

#[test]
fn different_bodies_are_no_rename_candidate() {
    let repo = python_repo("py-no-rename");
    let base = change_and_index(&repo, Index::Full, false, |repo| {
        repo.write(
            "pkg/lib.py",
            "def fresh(y):\n    return y * 3\n\n\ndef other():\n    return 1\n",
        );
    });
    let (_, delta, _) = repo.analyze(&base);
    assert_eq!(removed_names(&delta), vec!["pkg/lib.py::helper"]);
    assert!(delta.renamed_candidates.is_empty(), "{delta:?}");
}

#[test]
fn required_parameter_added_with_an_unchanged_caller() {
    for index in BOTH {
        let repo = python_repo("py-sig");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.write(
                "pkg/lib.py",
                "def helper(x, y):\n    return x + y\n\n\ndef other():\n    return 1\n",
            );
        });
        let (_, delta, references) = repo.analyze(&base);
        assert!(delta.removed.is_empty(), "{delta:?}");
        assert_eq!(delta.signature_changed.len(), 1, "{delta:?}");
        let change = &delta.signature_changed[0];
        assert_eq!(change.symbol.qualified_name, "pkg/lib.py::helper");
        assert_eq!(change.before.params.as_deref(), Some("(x)"));
        assert_eq!(change.after.params.as_deref(), Some("(x, y)"));
        assert!(change.params_changed);
        let calls: Vec<_> = refs_to(&references, "pkg/lib.py::helper")
            .into_iter()
            .filter(|r| r.edge_kind == "CALLS")
            .collect();
        assert_eq!(calls.len(), 1, "{index:?}: {references:?}");
        assert_eq!(calls[0].source_qualified, "app.py::main");
        assert_eq!(calls[0].matched_by, MatchKind::ExactTarget);
    }
}

#[test]
fn signature_change_with_the_caller_updated_in_the_same_diff() {
    for index in BOTH {
        let repo = python_repo("py-sig-updated");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.write(
                "pkg/lib.py",
                "def helper(x, y):\n    return x + y\n\n\ndef other():\n    return 1\n",
            );
            repo.write(
                "app.py",
                "from pkg.lib import helper\n\n\ndef main():\n    return helper(1, 2)\n",
            );
        });
        let (_, delta, references) = repo.analyze(&base);
        assert_eq!(delta.signature_changed.len(), 1);
        assert!(references.is_empty(), "{index:?}: {references:?}");
    }
}

#[test]
fn whitespace_only_signature_edits_are_no_change() {
    let repo = python_repo("py-sig-format");
    let base = change_and_index(&repo, Index::Full, false, |repo| {
        repo.write(
            "pkg/lib.py",
            "def helper( x ):\n    return x + 1\n\n\ndef other():\n    return 1\n",
        );
    });
    let (_, delta, _) = repo.analyze(&base);
    assert!(delta.is_empty(), "{delta:?}");
}

#[test]
fn deleted_file_reports_its_symbols_and_importers() {
    for index in BOTH {
        let repo = Repo::new("py-file-delete");
        repo.write("pkg/gone.py", "def dead():\n    return 0\n");
        repo.write(
            "user.py",
            "from pkg.gone import dead\n\n\ndef user():\n    return dead()\n",
        );
        repo.commit("init");
        let base = change_and_index(&repo, index, true, |repo| repo.remove("pkg/gone.py"));
        let (_, delta, references) = repo.analyze(&base);
        assert_eq!(removed_names(&delta), vec!["pkg/gone.py::dead"]);
        assert_eq!(delta.removed_files.len(), 1);
        assert_eq!(delta.removed_files[0].file_path, "pkg/gone.py");
        assert!(delta.removed_files[0].current_file.is_none());
        let call = refs_to(&references, "pkg/gone.py::dead")
            .into_iter()
            .find(|r| r.edge_kind == "CALLS")
            .unwrap_or_else(|| panic!("{index:?}: {references:?}"));
        assert_eq!(call.source_qualified, "user.py::user");
        assert!(
            refs_to(&references, "pkg/gone.py")
                .iter()
                .any(|r| r.edge_kind == "IMPORTS_FROM"),
            "{index:?}: {references:?}"
        );
    }
}

#[test]
fn renamed_file_with_unchanged_content_reports_nothing() {
    for index in BOTH {
        let repo = python_repo("py-file-move");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.git(&["mv", "pkg/lib.py", "pkg/core.py"]);
            repo.write(
                "app.py",
                "from pkg.core import helper\n\n\ndef main():\n    return helper(1)\n",
            );
            repo.write(
                "test_lib.py",
                "from pkg.core import other\n\n\ndef test_other():\n    assert other() == 1\n",
            );
        });
        let (changed, delta, references) = repo.analyze(&base);
        assert!(changed.contains(&"pkg/core.py".to_string()), "{changed:?}");
        assert!(delta.is_empty(), "{index:?}: {delta:?}");
        assert!(delta.removed_files.is_empty());
        assert_eq!(delta.moved_files.len(), 1);
        assert_eq!(delta.moved_files[0].file_path, "pkg/lib.py");
        assert_eq!(
            delta.moved_files[0].current_file.as_deref(),
            Some("pkg/core.py")
        );
        assert!(references.is_empty(), "{index:?}: {references:?}");
    }
}

#[test]
fn renamed_file_with_a_stale_importer_is_referenced() {
    let repo = python_repo("py-file-move-stale");
    let base = change_and_index(&repo, Index::Incremental, true, |repo| {
        repo.git(&["mv", "pkg/lib.py", "pkg/core.py"]);
    });
    let (_, delta, references) = repo.analyze(&base);
    assert!(delta.is_empty(), "{delta:?}");
    assert!(
        refs_to(&references, "pkg/lib.py")
            .iter()
            .any(|r| r.edge_kind == "IMPORTS_FROM" && r.file_path == "app.py"),
        "{references:?}"
    );
}

#[test]
fn typescript_export_removed_with_an_importer() {
    for index in BOTH {
        let repo = Repo::new("ts-export");
        repo.write(
            "lib.ts",
            "export function foo(a: number): number {\n  return a;\n}\n\nexport function bar(): number {\n  return 2;\n}\n",
        );
        repo.write(
            "main.ts",
            "import { foo, bar } from './lib';\n\nexport function use1(): number {\n  return foo(1) + bar();\n}\n",
        );
        repo.commit("init");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.write(
                "lib.ts",
                "export function bar(): number {\n  return 2;\n}\n",
            );
        });
        let (_, delta, references) = repo.analyze(&base);
        assert_eq!(removed_names(&delta), vec!["lib.ts::foo"]);
        let call = refs_to(&references, "lib.ts::foo")
            .into_iter()
            .find(|r| r.edge_kind == "CALLS")
            .unwrap_or_else(|| panic!("{index:?}: {references:?}"));
        assert_eq!(call.source_qualified, "main.ts::use1");
        assert_eq!(call.matched_by, MatchKind::ExactTarget);
        assert!(refs_to(&references, "lib.ts::bar").is_empty());
    }
}

#[test]
fn dirty_tree_against_head_reads_heads_blob() {
    let repo = python_repo("py-dirty");
    // Committed: `other` goes. Uncommitted: `helper` goes too.
    repo.write("pkg/lib.py", "def helper(x):\n    return x + 1\n");
    repo.write("test_lib.py", "def test_nothing():\n    pass\n");
    repo.commit("drop other");
    let base = change_and_index(&repo, Index::Incremental, false, |repo| {
        repo.write("pkg/lib.py", "VALUE = 1\n");
    });
    let source = base_source(&repo.0, "HEAD", "pkg/lib.py").expect("blob");
    assert_eq!(source, b"def helper(x):\n    return x + 1\n");
    let (changed, delta, references) = repo.analyze(&base);
    assert_eq!(changed, vec!["pkg/lib.py"]);
    assert_eq!(removed_names(&delta), vec!["pkg/lib.py::helper"]);
    assert_eq!(references.len(), 2, "{references:?}");
    // Against HEAD~1 the committed removal shows as well.
    let (_, delta, _) = repo.analyze("HEAD~1");
    let mut names = removed_names(&delta);
    names.sort_unstable();
    assert_eq!(names, vec!["pkg/lib.py::helper", "pkg/lib.py::other"]);
}

#[test]
fn importing_the_class_is_no_reference_to_a_removed_method() {
    for index in BOTH {
        let repo = Repo::new("py-method");
        repo.write(
            "pkg/shapes.py",
            "class Box:\n    def area(self):\n        return 1\n\n    def gone(self):\n        return 2\n",
        );
        repo.write(
            "use_box.py",
            "from pkg.shapes import Box\n\n\ndef size():\n    return Box().area()\n",
        );
        repo.commit("init");
        let base = change_and_index(&repo, index, true, |repo| {
            repo.write(
                "pkg/shapes.py",
                "class Box:\n    def area(self):\n        return 1\n",
            );
        });
        let (_, delta, references) = repo.analyze(&base);
        assert_eq!(removed_names(&delta), vec!["pkg/shapes.py::Box.gone"]);
        assert!(
            refs_to(&references, "pkg/shapes.py::Box.gone")
                .iter()
                .all(|r| r.edge_kind != "IMPORTS_FROM"),
            "{index:?}: {references:?}"
        );
    }
}

#[test]
fn function_turned_macro_generated_is_unconfirmed() {
    let repo = rust_repo("rs-macro");
    let base = change_and_index(&repo, Index::Incremental, false, |repo| {
        repo.write(
            "src/util.rs",
            "macro_rules! constant {\n    ($name:ident, $value:expr) => {\n        pub fn $name() -> u32 {\n            $value\n        }\n    };\n}\n\nconstant!(gone, 1);\n\npub fn kept() -> u32 {\n    2\n}\n",
        );
    });
    let (_, delta, references) = repo.analyze(&base);
    assert!(delta.removed.is_empty(), "{delta:?}");
    let unconfirmed: Vec<&str> = delta
        .unconfirmed_removed
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(unconfirmed, vec!["src/util.rs::gone"]);
    assert!(references.is_empty(), "{references:?}");
}

#[test]
fn removed_test_functions_are_listed_apart() {
    let repo = python_repo("py-test-removed");
    let base = change_and_index(&repo, Index::Full, false, |repo| {
        repo.write("test_lib.py", "from pkg.lib import other\n");
    });
    let (_, delta, _) = repo.analyze(&base);
    assert!(delta.removed.is_empty(), "{delta:?}");
    assert_eq!(delta.removed_tests.len(), 1);
    assert_eq!(
        delta.removed_tests[0].qualified_name,
        "test_lib.py::test_other"
    );
}

#[test]
fn base_source_is_none_for_a_path_absent_at_base() {
    let repo = python_repo("py-absent");
    assert!(base_source(&repo.0, "HEAD", "nope.py").is_none());
    assert!(base_source(&repo.0, "--output=x", "app.py").is_none());
    assert!(base_source(&repo.0, "HEAD", "app.py").is_some());
}

/// Times `symbol_delta` and `references_to` on an existing checkout:
/// `DAGAYN_BASE_SYMBOLS_REPO=<root> DAGAYN_BASE_SYMBOLS_BASE=<ref>
/// cargo test -p dagayn-tools time_on_a_real_repository -- --ignored --nocapture`.
/// `DAGAYN_BASE_SYMBOLS_HEAD=<ref>` restricts the changed files to
/// `base..head` (committed only) instead of base versus the working tree.
#[test]
#[ignore]
fn time_on_a_real_repository() {
    let root = PathBuf::from(std::env::var("DAGAYN_BASE_SYMBOLS_REPO").expect("repo"));
    let base = std::env::var("DAGAYN_BASE_SYMBOLS_BASE").unwrap_or_else(|_| "HEAD~3".into());
    let changed: Vec<String> = match std::env::var("DAGAYN_BASE_SYMBOLS_HEAD") {
        Ok(head) => {
            let out = Command::new("git")
                .args(["diff", "--name-only", "-M", &base, &head, "--"])
                .current_dir(&root)
                .output()
                .expect("git diff");
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        }
        Err(_) => change_file_sources(&root, &base).expect("changes").files,
    };
    let db = std::env::var("DAGAYN_BASE_SYMBOLS_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| db_path_for_build(&root).expect("db"));
    let store = GraphStore::open_read_only(db).expect("store");
    let started = std::time::Instant::now();
    let delta = symbol_delta(&root, &base, &changed);
    let delta_time = started.elapsed();
    let targets = delta.reference_targets();
    let started = std::time::Instant::now();
    let references = references_to(&store, &targets, &changed);
    let reference_time = started.elapsed();
    println!(
        "files={} compared={} removed={} unconfirmed_removed={} removed_tests={} \
         removed_files={} moved_files={} renamed_candidates={} signature_changed={} \
         references={}",
        changed.len(),
        delta.files_compared.len(),
        delta.removed.len(),
        delta.unconfirmed_removed.len(),
        delta.removed_tests.len(),
        delta.removed_files.len(),
        delta.moved_files.len(),
        delta.renamed_candidates.len(),
        delta.signature_changed.len(),
        references.len(),
    );
    println!("symbol_delta={delta_time:?} references_to={reference_time:?}");
    let mut by_match: HashMap<&str, usize> = HashMap::new();
    for reference in &references {
        *by_match.entry(reference.matched_by.as_str()).or_default() += 1;
    }
    println!("references by match: {by_match:?}");
    let mut removed_by_file: std::collections::BTreeMap<&str, usize> = Default::default();
    for symbol in &delta.removed {
        *removed_by_file
            .entry(symbol.file_path.as_str())
            .or_default() += 1;
    }
    println!("removed by file: {removed_by_file:?}");
    for file in &delta.removed_files {
        println!("  removed file {}", file.file_path);
    }
    for change in &delta.signature_changed {
        println!(
            "  signature {} {:?} -> {:?}",
            change.symbol.qualified_name, change.before, change.after
        );
    }
    for reference in references.iter().take(40) {
        println!(
            "  {} <- {} {} {}:{} {} {}",
            reference.target,
            reference.edge_kind,
            reference.source_qualified,
            reference.file_path,
            reference.line,
            reference.edge_tier.as_str(),
            reference.matched_by.as_str()
        );
    }
}
