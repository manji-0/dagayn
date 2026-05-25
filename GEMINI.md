# dagayn integration notes for Gemini-family tooling

`dagayn` is the documented product name for this fork. Upstream `code-review-graph` compatibility remains in code, but docs and examples in this repo should speak in terms of `dagayn`.

Recommended workflow:

1. run `dagayn install` in the repository you want to wire up
2. build the graph with `dagayn build`
3. use MCP tools such as `get_minimal_context_tool`, `review_tool`,
   `query_graph_tool`, and `flow_tool`
4. refresh with `dagayn update` or `dagayn watch`

The fork is especially useful in repositories that mix application code, docs, and Terraform.

MCP responses are evidence-ranked leads, not verdicts. Prefer `guidance`,
`answerability`, `missingness`, `zero_result_reason`, and `next_action` before
falling back to legacy raw fields. Documentation bridge results distinguish
`authored`, `extracted`, and `heuristic_reachable` evidence.
