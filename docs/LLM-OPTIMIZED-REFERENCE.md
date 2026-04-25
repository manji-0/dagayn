# dagayn LLM reference

<section name="usage">
Install with `pip install git+https://github.com/manji-0/dagayn.git`, run `dagayn install`, then `dagayn build`.

Use `dagayn update` for change-driven refreshes and `dagayn watch` for live development.

Use `dagayn` in all user-facing guidance.
</section>

<section name="review-delta">
Recommended sequence for reviewing a delta:

1. ensure the graph is up to date
2. call `detect_changes` or `get_review_context`
3. inspect affected nodes, flows, and tests
4. read only the files that remain ambiguous after graph queries

The fork is designed to work well when docs, app code, and Terraform all change together.
</section>

<section name="review-pr">
For larger reviews, start with `get_minimal_context`, then use `detect_changes`, `get_impact_radius`, `list_flows`, and `list_communities` as needed.

If the PR touches infrastructure, assume Terraform nodes and references are part of the review surface.
</section>

<section name="commands">
Important CLI commands:

- `dagayn install`
- `dagayn build`
- `dagayn update`
- `dagayn watch`
- `dagayn status`
- `dagayn detect-changes`
- `dagayn visualize`
- `dagayn serve`
- `dagayn register` / `dagayn repos` / `dagayn daemon`
</section>

<section name="legal">
`dagayn` is an MIT-licensed fork of `code-review-graph`.

The graph database is local by default. Optional embedding providers may call remote services only when explicitly configured.
</section>

<section name="watch">
Use `dagayn watch` when you want continuous graph refresh during active development.

Use `dagayn update` when you want a one-shot incremental refresh tied to a change set.
</section>

<section name="embeddings">
Embeddings are optional.

Use them when semantic search quality matters more than minimal dependencies. If provider imports are unavailable, keyword-based graph search still works.
</section>

<section name="languages">
The fork supports mainstream app languages plus Markdown, notebooks, and Terraform.

Terraform and Markdown are notable differentiators for this fork's review workflows.
</section>

<section name="troubleshooting">
If results look stale, rebuild or update the graph.

If integrations are missing, re-run `dagayn install --dry-run` first.

If local type checks disagree with CI, use the repository's `ty` command line with the documented excludes.
</section>
