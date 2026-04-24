# dagayn integration notes for Gemini-family tooling

`dagayn` is the documented product name for this fork. Upstream `code-review-graph` compatibility remains in code, but docs and examples in this repo should speak in terms of `dagayn`.

Recommended workflow:

1. run `dagayn install` in the repository you want to wire up
2. build the graph with `dagayn build`
3. use MCP tools such as `get_minimal_context`, `query_graph`, and `detect_changes`
4. refresh with `dagayn update` or `dagayn watch`

The fork is especially useful in repositories that mix application code, docs, and Terraform.
