use super::*;

#[test]
fn parses_markdown_sections_and_edges() {
    let source = b"# API Reference

<!-- derived-from ./guide.md#Installation -->

See [Getting Started](./guide.md#Getting-Started).

[InstallRef]: ./guide.md#Installation

## Endpoints

Call `build_graph`.
";
    let (nodes, edges) = parse_markdown("api.md", source);
    assert_eq!(nodes.len(), 5);
    assert!(nodes.iter().any(|node| node.name == "api-reference"));
    assert!(nodes.iter().any(|node| node.name == "endpoints"));
    assert!(nodes.iter().any(|node| {
        node.kind == "DocBody"
            && node.name == "api-reference--body-1"
            && node.line_start == 5
            && node.line_end == 5
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "DocBody"
            && node.name == "endpoints--body-1"
            && node.line_start == 11
            && node.line_end == 11
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "DEPENDS_ON"
            && edge.source == "api.md::api-reference"
            && edge.target == "guide.md::installation"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "api.md::api-reference"
            && edge.target == "api.md::api-reference--body-1"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "api.md::api-reference"
            && edge.target == "guide.md::getting-started"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "api.md::api-reference"
            && edge.target == "guide.md::installation"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT" && edge.target == "<unresolved:build_graph>"
    }));
}

#[test]
fn dagayn_directives_shown_as_code_create_no_edges() {
    // The second and third directives are examples a guide shows, not claims.
    let source = b"# Contract

<!-- dagayn: implemented-by src/auth.py::refresh -->

Write `<!-- dagayn: implemented-by src/example.py::shown -->` to link code.

```markdown
<!-- dagayn: implemented-by src/example.py::fenced -->
```
";
    let (_, edges) = parse_markdown("spec.md", source);
    let targets = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    assert_eq!(targets, vec!["src/auth.py::refresh"]);
}
