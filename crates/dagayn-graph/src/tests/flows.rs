#[test]
fn flow_test_file_pattern_covers_javascript_variants() {
    for path in [
        "src/App.test.tsx",
        "src/app.spec.mts",
        "src/app.test.cjs",
        "cypress/e2e/login.cy.ts",
        "src/__tests__/util.ts",
    ] {
        assert!(crate::flow_trace::is_test_file(path), "{path}");
    }
    for path in ["src/latest.ts", "src/contest.tsx"] {
        assert!(!crate::flow_trace::is_test_file(path), "{path}");
    }
}
