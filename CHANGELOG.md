# Changelog

This changelog tracks `dagayn` as a forked project with its own release notes.

## Unreleased

- rewrote repository documentation to describe `dagayn` rather than upstream terminology
- switched CI type checking from `mypy` to `ty`
- normalized docs, lint, and format workflows around `ruff`
- stabilized notebook `cell_index` tagging after formatter-driven fixture changes

## 2.3.2 fork line

- kept compatibility aliases such as `code-review-graph` while documenting `dagayn` as the preferred command name
- added first-class Terraform support via a commit-pinned grammar fetch flow
- added Markdown parsing support, including heading sections, references, and directive-based dependencies
- added multi-file Markdown and Terraform graph tests
- added mixed monorepo coverage for Markdown, Python, and Terraform
- moved graph-registered file paths toward repo-root-relative storage for dagayn workflows

## Notes on versioning

`dagayn` may share some version numbers with the upstream project when the fork line is rebased, but the changelog in this repository only describes fork behavior and fork releases.
