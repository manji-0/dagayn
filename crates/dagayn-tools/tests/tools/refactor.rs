use serde_json::{Value, json};

use crate::common::{Repo, answer, declines};

#[test]
fn refactor_suggest_does_not_let_one_file_fill_every_unused_slot() {
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    // A survey or generated file with many unused symbols sorts first; the
    // two other unused symbols must still be shown.
    let repo = Repo::new("refactor-spread", true);
    let many: String = (0..12)
        .map(|i| format!("def survey_{i:02}():\n    return {i}\n\n\n"))
        .collect();
    repo.write("aaa_corpus.py", &many);
    repo.write("zz_one.py", "def stale_one():\n    return 1\n");
    repo.write("zz_two.py", "def stale_two():\n    return 2\n");
    repo.build();
    let suggest = answer(&repo.context(), "refactor_tool", json!({}));
    let unused: Vec<&Value> = suggest["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter(|f| f["kind"] == "unused_symbol")
        .collect();
    let names: Vec<&str> = unused
        .iter()
        .filter_map(|f| f["qualified_name"].as_str())
        .collect();
    assert_eq!(unused.len(), 10, "{names:?}");
    assert!(names.contains(&"zz_one.py::stale_one"), "{names:?}");
    assert!(names.contains(&"zz_two.py::stale_two"), "{names:?}");
    assert_eq!(suggest["findings_omitted"]["unused_symbol"], 4);
    // The busy file keeps its own order.
    let corpus: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| n.starts_with("aaa_corpus.py::"))
        .collect();
    let mut sorted = corpus.clone();
    sorted.sort_unstable();
    assert_eq!(corpus, sorted);
}

#[test]
fn refactor_finds_dead_code_and_suggests() {
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    let repo = Repo::new("refactor", true);
    repo.write("unused.py", "def orphan():\n    return 1\n");
    repo.build();
    let context = repo.context();
    let dead = answer(&context, "refactor_tool", json!({"mode": "dead_code"}));
    let names: Vec<&str> = dead["dead_code"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["name"].as_str())
        .collect();
    assert!(names.contains(&"orphan"), "{names:?}");
    assert!(dead["next"].is_array());
    assert!(dead.get("_hints").is_none());
    let suggest = answer(&context, "refactor_tool", json!({}));
    assert_eq!(suggest["findings"][0]["kind"], "unused_symbol");
    assert_eq!(
        suggest["findings"][0]["qualified_name"],
        "unused.py::orphan"
    );
    assert!(suggest["findings"][0]["evidence"].is_object());
    assert_eq!(suggest["findings_omitted"], json!({}));
    assert_eq!(suggest["summary"], "Findings: 1 unused_symbol.");
    assert!(suggest.get("suggestions").is_none());
    let minimal = answer(
        &context,
        "refactor_tool",
        json!({"detail_level": "minimal"}),
    );
    assert!(minimal["findings"][0].get("evidence").is_none());
    let verbose = answer(
        &context,
        "refactor_tool",
        json!({"detail_level": "verbose"}),
    );
    // The size-based suggestions are gone at every level.
    assert_eq!(verbose["findings"], suggest["findings"]);
    for gone in ["suggestions", "work_packs", "counts_by_type"] {
        assert!(verbose.get(gone).is_none(), "{gone}");
    }
    let preview = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "kept"}),
    );
    let id = preview["refactor_id"].as_str().expect("id").to_string();
    assert_eq!(preview["edits"][0]["source"], "definition");
    assert_eq!(preview["edits_omitted"], 0);
    assert_eq!(
        preview["files"],
        json!([{"file": "unused.py", "edit_count": 1}])
    );
    let stored: Value =
        serde_json::from_str(&dagayn_tools::pending::get(&id).expect("pending")).expect("json");
    assert_eq!(stored["new_name"], "kept");
    let bad = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "1x"}),
    );
    assert_eq!(bad["status"], "error");
    let missing = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "zz_none_qq", "new_name": "x"}),
    );
    assert_eq!(missing["status"], "not_found");
    // Python's `\w`: a letter is one, a combining mark is not; `repr` escapes
    // what is not printable.
    let accented = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "\u{e9}t\u{e9}"}),
    );
    assert_eq!(accented["status"], "ok", "{accented}");
    let marked = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "e\u{301}\u{200b}"}),
    );
    assert_eq!(
        marked["error"],
        "new_name is not a valid identifier: 'e\u{301}\\u200b'"
    );
    assert!(declines(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "", "new_name": "b"})
    ));
}

/// The new file a whole-file unified diff describes: its context and added
/// lines, after the `---`/`+++`/`@@` headers.
fn new_side(diff: &str) -> String {
    let body = diff.splitn(4, '\n').nth(3).expect("diff headers");
    let mut out = String::new();
    let mut rest = body;
    while !rest.is_empty() {
        // Each diff line is a marker and a source line with its own end.
        let (marker, tail) = rest.split_at(1);
        let end = tail
            .char_indices()
            .find_map(|(i, c)| match c {
                '\n' => Some(i + 1),
                '\r' if !tail[i + 1..].starts_with('\n') => Some(i + 1),
                _ => None,
            })
            .unwrap_or(tail.len());
        if marker != "-" {
            out.push_str(&tail[..end]);
        }
        rest = &tail[end..];
    }
    out
}

#[test]
fn a_rename_preview_is_applied_or_shown_as_a_diff() {
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    // dagayn: tests crates/dagayn-tools/src/apply.rs::apply_refactor
    let repo = Repo::new("apply", true);
    repo.build();
    let context = repo.context();
    let rename = |old: &str, new: &str| {
        answer(
            &context,
            "refactor_tool",
            json!({"mode": "rename", "old_name": old, "new_name": new}),
        )["refactor_id"]
            .as_str()
            .expect("refactor_id")
            .to_string()
    };
    let id = rename("helper", "assist");
    let dry = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": id, "dry_run": true}),
    );
    assert_eq!(dry["status"], "ok");
    assert_eq!(dry["would_modify"], json!(["app.py"]));
    let diff = dry["diffs"]["app.py"].as_str().expect("diff");
    assert!(
        diff.starts_with("--- a/app.py\n+++ b/app.py\n@@ -1,6 +1,6 @@\n"),
        "{diff}"
    );
    assert!(diff.contains("-def helper():\n+def assist():\n"), "{diff}");

    let applied = answer(&context, "apply_refactor_tool", json!({"refactor_id": id}));
    assert_eq!(applied["status"], "ok");
    assert_eq!(applied["edits_applied"], 2);
    let source = std::fs::read_to_string(repo.0.join("app.py")).expect("app.py");
    assert!(source.contains("return assist()") && source.contains("def assist():"));
    let gone = answer(&context, "apply_refactor_tool", json!({"refactor_id": id}));
    assert_eq!(
        gone["error"],
        format!("Refactor '{id}' not found or expired.")
    );

    // CRLF line ends are kept (#180), and bytes that are not UTF-8 are
    // replaced and written back that way. The diff shows what is written.
    let id = rename("main", "start");
    std::fs::write(
        repo.0.join("app.py"),
        b"def main():\r\n    return '\xff'\r\n",
    )
    .expect("write");
    let dry = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": id, "dry_run": true}),
    );
    let diff = dry["diffs"]["app.py"].as_str().expect("diff");
    assert!(
        diff.contains("-def main():\r\n+def start():\r\n     return '\u{fffd}'\r\n"),
        "{dry}"
    );
    let applied = answer(&context, "apply_refactor_tool", json!({"refactor_id": id}));
    assert_eq!(applied["status"], "ok", "{applied}");
    let written = std::fs::read_to_string(repo.0.join("app.py")).expect("app.py");
    assert_eq!(written, "def start():\r\n    return '\u{fffd}'\r\n");
    assert_eq!(new_side(diff), written);

    // A preview Python would have stored: a path that does not exist yet
    // is skipped, an empty name matches nothing, and a float line counts.
    let stored = |id: &str, edits: Value| {
        dagayn_tools::pending::set(
            id,
            json!({"created_at": dagayn_tools::pending::now(), "edits": edits}).to_string(),
        );
    };
    // A lone `\r` and a `\n` beside `\r\n` stay as they are; the line an
    // edit names counts each of them as a line end.
    std::fs::write(
        repo.0.join("ends.py"),
        b"x = 1\r\ny = old_n\rz = old_n\n\r\nw = old_n\r",
    )
    .expect("write");
    stored(
        "ends",
        json!([
            {"file": "ends.py", "line": 2, "old": "old_n", "new": "new_n"},
            {"file": "ends.py", "line": 5, "old": "old_n", "new": "new_n"},
        ]),
    );
    let dry = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "ends", "dry_run": true}),
    );
    assert_eq!(dry["edits_applied"], 2, "{dry}");
    let diff = dry["diffs"]["ends.py"].as_str().expect("diff").to_string();
    let applied = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "ends"}),
    );
    assert_eq!(applied["status"], "ok", "{applied}");
    let written = std::fs::read(repo.0.join("ends.py")).expect("ends.py");
    assert_eq!(written, b"x = 1\r\ny = new_n\rz = old_n\n\r\nw = new_n\r");
    assert_eq!(new_side(&diff).as_bytes(), written);

    stored(
        "odd",
        json!([
            {"file": "gone/new.py", "line": 1, "old": "a", "new": "b"},
            {"file": "app.py", "line": 1, "old": "", "new": "b"},
            {"file": "app.py", "line": 1.0, "old": "start", "new": "begin"},
            {"file": "app.py", "line": null, "old": "start", "new": "begin"},
            {"file": "app.py", "line": 9, "old": "start", "new": "begin"},
        ]),
    );
    let odd = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "odd", "dry_run": true}),
    );
    assert_eq!(odd["status"], "partial");
    assert_eq!(odd["edits_applied"], 1);
    let reasons: Vec<&str> = odd["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .filter_map(|skip| skip["reason"].as_str())
        .collect();
    assert_eq!(
        reasons,
        [
            "file_not_found",
            "line_no_longer_matches",
            "no_line_recorded",
            "line_out_of_range"
        ]
    );
    stored(
        "escape",
        json!([{"file": "../outside.py", "line": 1, "old": "a", "new": "b"}]),
    );
    let escape = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "escape"}),
    );
    assert_eq!(
        escape["error"],
        "Edit path '../outside.py' is outside repo root."
    );

    // A named root is checked as `_validate_repo_root` does.
    let missing = repo.0.join("no-such-dir");
    let named = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "odd", "repo_root": missing}),
    );
    assert_eq!(
        named["error"],
        format!(
            "repo_root is not an existing directory: {}",
            missing.display()
        )
    );
    let plain = repo.0.join("docs");
    let named = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "odd", "repo_root": plain}),
    );
    assert_eq!(
        named["error"],
        format!(
            "repo_root does not look like a project root (no .git or .dagayn/graph.db found): {}",
            plain.display()
        )
    );
}

#[test]
fn dead_code_reports_unused_methods_of_pydantic_models() {
    // dagayn: tests crates/dagayn-tools/src/dead_code.rs::graph_candidates
    // dagayn: tests crates/dagayn-tools/src/dead_code_verify.rs::verify
    let repo = Repo::new("pydantic-dead", true);
    repo.write(
        "models.py",
        "from typing import Any\n\nfrom pydantic import BaseModel, field_validator\nfrom vendor import Plugin\n\n\nclass Parent(BaseModel):\n    code: str\n\n    def unused_helper(self) -> str:\n        return self.code\n\n    def shared_name(self) -> str:\n        return self.code\n\n    def model_post_init(self, context: Any) -> None:\n        self.code = self.code.strip()\n\n    def model_dump(self, **kwargs: Any) -> dict:\n        return super().model_dump(**kwargs)\n\n    @field_validator(\"code\")\n    @classmethod\n    def check_code(cls, v: str) -> str:\n        return v\n\n\nclass Child(Parent):\n    def child_unused(self) -> str:\n        return self.code\n\n    def shared_name(self) -> str:\n        return self.code + \"!\"\n\n\nclass Hook(Plugin):\n    def on_load(self) -> None:\n        return None\n\n\ndef make() -> tuple:\n    return Child(code=\"a\"), Hook()\n",
    );
    repo.write("main.py", "from models import make\n\nmake()\n");
    repo.build();
    let context = repo.context();
    let dead = answer(&context, "refactor_tool", json!({"mode": "dead_code"}));
    let mut names: Vec<&str> = dead["dead_code"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["qualified_name"].as_str())
        .collect();
    names.sort_unstable();
    // pydantic calls `model_post_init`, `model_dump` (an override of
    // BaseModel's), and the validator; `Child.shared_name` overrides
    // `Parent.shared_name` (whose name the source scan then finds);
    // `Hook.on_load` may override `Plugin`'s, which is not pydantic.
    assert_eq!(
        names,
        [
            "models.py::Child.child_unused",
            "models.py::Parent.unused_helper",
        ],
        "{dead}"
    );
}
