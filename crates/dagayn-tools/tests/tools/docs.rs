use serde_json::json;

use crate::common::{Repo, answer};

#[test]
fn docs_sections_come_from_the_repository_and_are_truncated_by_characters() {
    // dagayn: tests crates/dagayn-tools/src/docs.rs::get_docs_section
    let repo = Repo::new("docs", false);
    repo.build();
    let context = repo.context();
    let section = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "TRUST"}),
    );
    assert_eq!(section["content"], "Graph reach is not correctness.");
    assert_eq!(section["truncated"], false);
    let cut = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "trust", "max_chars": 5}),
    );
    assert_eq!(cut["content"], "Graph\n... (truncated)");
    assert_eq!(cut["truncated"], true);
    // An unknown section lists the sections the reference file holds.
    let missing = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "nope"}),
    );
    assert_eq!(missing["status"], "not_found");
    assert!(
        missing["error"]
            .as_str()
            .unwrap()
            .starts_with("Section 'nope' not found. Available: ")
    );
    let negative = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "trust", "max_chars": -100000}),
    );
    assert_eq!(negative["content"], "\n... (truncated)");
}
