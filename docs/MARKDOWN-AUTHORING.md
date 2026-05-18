<!-- constrained-by ./SCHEMA.md#edge-kinds -->
<!-- constrained-by ./CROSS-ARTIFACT-EDGES-WIP.md#documentation-bridges -->

# Markdown Authoring

Dagayn reads Markdown as graph input. Headings become `DocSection` nodes, prose
blocks become `DocBody` nodes under the nearest section, relative links become
document references, dependency comments become `DEPENDS_ON` edges, and
symbol-shaped backtick spans can become `CROSS_ARTIFACT` edges.

Use dependency comments only when one section or document has a real ongoing
constraint on another:

```markdown
<!-- constrained-by ./COMMANDS.md#mcp-tools -->
<!-- derived-from ./ARCHITECTURE.md#query-surfaces -->
```

Use `dagayn:` directives when the document intentionally creates a reviewable
obligation between prose and another artifact:

```markdown
# dagayn: implemented-by dagayn/tools/query.py::traverse_graph_func
```

Use ordinary backticks for display text, command names, file names, and short
examples that should not create a durable obligation. When a report mentions a
symbol only as evidence, prefer a sentence such as "Informed by
`dagayn/tools/query.py::traverse_graph_func`" instead of adding a dependency
comment or `dagayn:` directive.

For analysis reports, separate these cases deliberately:

- **Graph dependency:** the report section must change when the target changes.
- **Review obligation:** the target implements, validates, or contradicts the report.
- **Plain evidence:** the target is only an example or historical observation.

Only the first two should create graph edges. Plain evidence should remain prose.
