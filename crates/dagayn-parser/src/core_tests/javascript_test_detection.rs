use super::*;

#[test]
fn does_not_mark_test_prefixed_components_as_tests() {
    let source = br#"export function TestimonialCard() {
  return <div />;
}
export const TestBadge = () => <span />;
function test_helper() {}
class Tester {
  TestMode = () => test_helper();
}
"#;
    let (nodes, _edges) = parse_javascript_like("src/components/Testimonial.tsx", source, "tsx");
    for name in ["TestimonialCard", "TestBadge", "test_helper", "TestMode"] {
        assert!(
            nodes
                .iter()
                .any(|node| { node.kind == "Function" && node.name == name && !node.is_test }),
            "{name} should be a plain Function: {nodes:?}"
        );
    }
    assert!(!nodes.iter().any(|node| node.kind == "Test"));

    let test_source = br#"function TestHelper() {}
function test_setup() {}
function helper() {}
"#;
    let (nodes, _edges) = parse_javascript_like("src/card.test.ts", test_source, "typescript");
    for name in ["TestHelper", "test_setup"] {
        assert!(
            nodes
                .iter()
                .any(|node| node.kind == "Test" && node.name == name && node.is_test),
            "{name} should stay a Test in a test file: {nodes:?}"
        );
    }
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "helper")
    );
}

#[test]
fn detects_javascript_test_files_by_suffix_and_directory() {
    let source = b"it('works', () => {});\n";
    for path in [
        "src/App.test.tsx",
        "src/app.spec.jsx",
        "src/app.test.mjs",
        "src/app.test.cjs",
        "src/app.spec.mts",
        "src/app.test.cts",
        "src/__tests__/util.ts",
        "__tests__/util.js",
        "cypress/e2e/login.cy.ts",
        "src/components/Button.cy.jsx",
        "e2e/login.ts",
        "apps/web/e2e/checkout.ts",
        "tests/unit/app.ts",
    ] {
        let language = if path.ends_with("ts") || path.ends_with("tsx") {
            "typescript"
        } else {
            "javascript"
        };
        let (nodes, _edges) = parse_javascript_like(path, source, language);
        assert!(nodes[0].is_test, "{path} should be a test file");
        assert!(
            nodes
                .iter()
                .any(|node| node.kind == "Test" && node.name == "it:works@L1"),
            "{path} should have a Test node: {nodes:?}"
        );
    }
    for path in [
        "src/latest.ts",
        "src/contest.tsx",
        "src/spec.ts",
        "src/test.ts",
        "src/e2e-config.ts",
        "src/protests/index.js",
    ] {
        let (nodes, _edges) = parse_javascript_like(path, source, "typescript");
        assert!(!nodes[0].is_test, "{path} should not be a test file");
        assert!(!nodes.iter().any(|node| node.kind == "Test"), "{path}");
    }
}

#[test]
fn parses_javascript_test_runner_variants() {
    let source = br#"import { decl, arrow } from "../functions";

describe("functions", () => {
  beforeEach(() => { arrow(0); });
  it("decl works", () => {
    expect(decl(1)).toBe(2);
  });
  test.each([1, 2])("arrow %i", (n) => {
    arrow(n);
  });
  it.each`
    a    | b
    ${1} | ${2}
  `("adds $a", ({ a }) => {
    decl(a);
  });
  it.only("focused", () => { decl(3); });
  describe.skip("nested", () => {
    test.todo("later");
    test.concurrent("parallel", async () => { decl(4); });
  });
  suite("suite block", () => {
    specify("spec", () => { decl(5); });
  });
});
test.describe("group", () => {
  test.beforeEach(async () => { arrow(6); });
  test("inner", () => { vi.fn(); });
});
"#;
    let path = "src/__tests__/calls.test.ts";
    let (nodes, edges) = parse_javascript_like(path, source, "typescript");
    let tests = nodes
        .iter()
        .filter(|node| node.kind == "Test")
        .map(|node| (node.name.as_str(), node.line_start, node.line_end))
        .collect::<Vec<_>>();
    for expected in [
        ("describe:functions@L3", 3, 25),
        ("it:decl works@L5", 5, 7),
        ("test:arrow %i@L8", 8, 10),
        ("it:adds $a@L11", 11, 16),
        ("it:focused@L17", 17, 17),
        ("describe:nested@L18", 18, 21),
        ("test:later@L19", 19, 19),
        ("test:parallel@L20", 20, 20),
        ("suite:suite block@L22", 22, 24),
        ("specify:spec@L23", 23, 23),
        ("describe:group@L26", 26, 29),
        ("test:inner@L28", 28, 28),
    ] {
        assert!(tests.contains(&expected), "{expected:?} missing: {tests:?}");
    }
    assert_eq!(tests.len(), 12, "unexpected Test nodes: {tests:?}");
    let modifiers = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| node.extra.get("test_modifiers").cloned())
    };
    assert_eq!(modifiers("test:arrow %i@L8"), Some(json!(["each"])));
    assert_eq!(modifiers("describe:nested@L18"), Some(json!(["skip"])));
    assert_eq!(modifiers("test:later@L19"), Some(json!(["todo"])));
    assert_eq!(modifiers("it:decl works@L5"), None);

    let qn = |name: &str| format!("{path}::{name}");
    let has_edge = |kind: &str, source: &str, target_suffix: &str, line: i64| {
        edges.iter().any(|edge| {
            edge.kind.as_str() == kind
                && edge.source == source
                && edge.target.ends_with(target_suffix)
                && edge.line == line
        })
    };
    // `.each` bodies belong to the synthetic test, hooks to the describe.
    assert!(has_edge("CALLS", &qn("test:arrow %i@L8"), "arrow", 9));
    assert!(has_edge("CALLS", &qn("it:adds $a@L11"), "decl", 15));
    assert!(has_edge("CALLS", &qn("describe:functions@L3"), "arrow", 4));
    assert!(has_edge("CALLS", &qn("describe:group@L26"), "arrow", 27));
    assert!(has_edge(
        "CONTAINS",
        &qn("describe:nested@L18"),
        "test:later@L19",
        19
    ));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source.ends_with("decl")
            && edge.target == qn("it:decl works@L5")
    }));
    // Runner APIs get no edges; assertion / mock APIs keep CALLS but never
    // become TESTED_BY sources.
    for api in ["beforeEach", "each", "describe", "it", "test", "todo"] {
        assert!(
            !edges
                .iter()
                .any(|edge| matches!(edge.kind.as_str(), "CALLS" | "TESTED_BY")
                    && (edge.target == api || edge.source == api)),
            "{api} should not get CALLS / TESTED_BY: {edges:?}"
        );
    }
    for api in ["expect", "toBe", "fn"] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CALLS" && edge.target == api && edge.extra["test_api"] == true
            }),
            "{api} CALLS should be marked test_api: {edges:?}"
        );
        assert!(
            !edges
                .iter()
                .any(|edge| edge.kind == "TESTED_BY" && edge.source == api),
            "{api} should not be a TESTED_BY source"
        );
    }
}
