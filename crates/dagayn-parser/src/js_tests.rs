//! JavaScript / TypeScript test files and test-runner calls (Jest, Vitest,
//! Mocha, Jasmine, Playwright, Cypress).
//!
//! A test-runner call (`it("x", fn)`, `test.each(table)("x", fn)`,
//! `test.describe("g", fn)`) becomes a synthetic `Test` node. The other
//! runner APIs (hooks such as `beforeEach`, `test.step`, `test.use`, the
//! inner `test.each(table)` factory) are neither tests nor production code,
//! so they get no node and no edge; the calls inside their callbacks belong
//! to the enclosing node. Assertion and mock APIs (`expect(...)`, `vi.fn()`)
//! keep their `CALLS` edge, marked `test_api: true`, so that `TESTED_BY`
//! is never derived from them.

use std::collections::HashSet;

use super::js_modules::decode_javascript_string_literal;
use super::util::{
    contains_ascii_ignore_case, is_test_file, node_text, starts_with_ascii_ignore_case,
};

/// What a call in a test file is, as far as the test runner is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JavaScriptTestCall {
    /// A test or suite: `runner` is the written runner name (`it`,
    /// `describe`), `modifiers` the chained modifiers in source order
    /// (`only`, `skip`, `each`, ...).
    Test {
        runner: String,
        modifiers: Vec<String>,
    },
    /// A runner API that is not itself a test: hooks, `test.step`,
    /// `test.use`, `describe.configure`, or the `test.each(table)` factory.
    RunnerApi,
}

/// Runner names that declare a test or a suite.
pub(super) fn is_test_runner_name(name: &str) -> bool {
    matches!(
        name,
        "describe"
            | "it"
            | "test"
            | "suite"
            | "specify"
            | "context"
            | "fdescribe"
            | "xdescribe"
            | "fit"
            | "xit"
    )
}

/// Hooks: setup / teardown callbacks that are not tests.
fn is_test_hook_name(name: &str) -> bool {
    matches!(
        name,
        "beforeEach" | "afterEach" | "beforeAll" | "afterAll" | "before" | "after"
    )
}

/// Chained modifiers that keep a runner call a test.
fn is_test_modifier(name: &str) -> bool {
    matches!(
        name,
        "only"
            | "skip"
            | "todo"
            | "concurrent"
            | "sequential"
            | "shuffle"
            | "fails"
            | "failing"
            | "fixme"
            | "slow"
            | "serial"
            | "parallel"
            | "each"
            | "for"
            | "skipIf"
            | "runIf"
    )
}

/// Modifiers that return a test function instead of declaring a test:
/// `test.each(table)` and `test.skipIf(cond)` are factories whose result is
/// called with the name and body.
fn is_test_factory_modifier(name: &str) -> bool {
    matches!(name, "each" | "for" | "skipIf" | "runIf")
}

/// Roots of assertion and mocking APIs (`expect(x).toBe(y)`, `vi.fn()`,
/// `cy.get()`).
fn is_test_api_root(name: &str) -> bool {
    matches!(
        name,
        "expect" | "assert" | "vi" | "vitest" | "jest" | "sinon" | "chai" | "cy" | "Cypress"
    )
}

/// JavaScript / TypeScript test files: the shared directory rules
/// (`test/`, `tests/`), `*.test.*` / `*.spec.*` / Cypress `*.cy.*` with any
/// JavaScript or TypeScript extension, `__tests__/`, and end-to-end
/// directories (`e2e/`, `e2e-tests/`, `cypress/`).
pub(super) fn is_javascript_test_file(file_path: &str) -> bool {
    if is_test_file(file_path) {
        return true;
    }
    let normalized = file_path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    if javascript_test_suffix(name) {
        return true;
    }
    normalized.split('/').rev().skip(1).any(|dir| {
        dir.eq_ignore_ascii_case("__tests__")
            || dir.eq_ignore_ascii_case("cypress")
            || dir.eq_ignore_ascii_case("e2e")
            || (starts_with_ascii_ignore_case(dir, "e2e")
                && contains_ascii_ignore_case(dir, "test"))
    })
}

/// `name.test.ts`, `name.spec.mjs`, `name.cy.jsx`, ...
fn javascript_test_suffix(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some((stem, extension)) = lower.rsplit_once('.') else {
        return false;
    };
    if !matches!(
        extension,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts"
    ) {
        return false;
    }
    let Some((base, marker)) = stem.rsplit_once('.') else {
        return false;
    };
    !base.is_empty() && matches!(marker, "test" | "spec" | "cy")
}

/// One segment of a callee chain (`test`, `each`, ...), and whether the
/// segment was itself called (`each` in `test.each(table)(...)`).
struct CalleeSegment {
    name: String,
    called: bool,
}

/// Flattens `test.describe.only`, `test.each(table)`, and
/// `expect(x).not.toBe` into their segments; `false` for any other shape.
fn javascript_callee_chain(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<CalleeSegment>,
) -> bool {
    match node.kind() {
        "identifier" => {
            out.push(CalleeSegment {
                name: node_text(node, source),
                called: false,
            });
            true
        }
        "member_expression" => {
            let (Some(object), Some(property)) = (
                node.child_by_field_name("object"),
                node.child_by_field_name("property"),
            ) else {
                return false;
            };
            if property.kind() != "property_identifier"
                || !javascript_callee_chain(object, source, out)
            {
                return false;
            }
            out.push(CalleeSegment {
                name: node_text(property, source),
                called: false,
            });
            true
        }
        "call_expression" => {
            let Some(function) = node.child_by_field_name("function") else {
                return false;
            };
            if !javascript_callee_chain(function, source, out) {
                return false;
            }
            if let Some(last) = out.last_mut() {
                last.called = true;
            }
            true
        }
        _ => false,
    }
}

fn javascript_call_chain(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<CalleeSegment>> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    let mut chain = Vec::new();
    javascript_callee_chain(function, source, &mut chain).then_some(chain)
}

/// Classifies a call in a test file; `None` for ordinary calls.
pub(super) fn javascript_test_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    defined_names: &HashSet<String>,
) -> Option<JavaScriptTestCall> {
    let chain = javascript_call_chain(node, source)?;
    let root = chain.first()?;
    if is_test_hook_name(&root.name) {
        let hook = chain.len() == 1 && !defined_names.contains(&root.name);
        return hook.then_some(JavaScriptTestCall::RunnerApi);
    }
    if !is_test_runner_name(&root.name) {
        return None;
    }
    let name = javascript_test_name_argument(node, source);
    if root.name == "context"
        && (defined_names.contains("context") || !matches!(name, Some((_, true))))
    {
        // Mocha's `context` alias only with a literal title.
        return None;
    }
    let mut runner = root.name.clone();
    let mut modifiers = Vec::new();
    for (index, segment) in chain.iter().enumerate() {
        if index > 0 {
            if is_test_runner_name(&segment.name) {
                // Playwright `test.describe`.
                runner = segment.name.clone();
            } else if is_test_modifier(&segment.name) {
                modifiers.push(segment.name.clone());
            } else {
                // `test.step`, `test.use`, `test.beforeEach`, `describe.configure`.
                return Some(JavaScriptTestCall::RunnerApi);
            }
        }
        // Only the last segment may be an invoked factory
        // (`test.each(table)("x", fn)`); the factory call itself
        // (`test.each(table)`) and any other called segment are not tests.
        let last = index + 1 == chain.len();
        let factory = is_test_factory_modifier(&segment.name);
        if (segment.called && !(factory && last)) || (factory && last && !segment.called) {
            return Some(JavaScriptTestCall::RunnerApi);
        }
    }
    // Playwright `test.skip()` / `test.skip(cond, reason)` inside a test
    // skips it; only a title (or a named body) declares a test.
    let declares = match name {
        Some((_, literal)) => literal || javascript_has_function_argument(node),
        None => javascript_has_function_argument(node),
    };
    if !declares {
        return Some(JavaScriptTestCall::RunnerApi);
    }
    Some(JavaScriptTestCall::Test { runner, modifiers })
}

/// Whether a call belongs to an assertion or mocking API
/// (`expect(x).toBe(y)`, `vi.fn()`, `jest.mock("./m")`, `cy.get("a")`).
pub(super) fn javascript_is_test_api_call(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    javascript_call_chain(node, source)
        .and_then(|chain| chain.into_iter().next())
        .is_some_and(|root| is_test_api_root(&root.name))
}

/// The title of a test declaration: a string or template literal (`true`),
/// or an identifier / member expression such as `describe(UserService, fn)`
/// (`false`).
fn javascript_test_name_argument(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, bool)> {
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let first = arguments.named_children(&mut cursor).next()?;
    match first.kind() {
        "string" | "template_string" => {
            Some((decode_javascript_string_literal(first, source), true))
        }
        "identifier" | "member_expression" => Some((node_text(first, source), false)),
        _ => None,
    }
}

/// The title used in a synthetic test name, when the call has one.
pub(super) fn javascript_test_title(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    javascript_test_name_argument(node, source)
        .map(|(title, _)| title)
        .filter(|title| !title.is_empty())
}

fn javascript_has_function_argument(node: tree_sitter::Node<'_>) -> bool {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = arguments.walk();
    arguments.named_children(&mut cursor).any(|argument| {
        matches!(
            argument.kind(),
            "arrow_function" | "function_expression" | "function" | "generator_function"
        )
    })
}
