# Terraform grammar integration plan

## Goal

Document the fork-specific plan for Terraform support through a dedicated grammar path rather than relying only on generic language packs.

## Why the fork needs this

The fork treats Terraform as a first-class language for repository review. That requires stable parsing of:

- resources
- data sources
- modules
- variables
- locals
- outputs
- provider references
- inheritance-like dependency structures through references and composition

## Plan shape

1. keep Terraform grammar loading explicit in the parser
2. provision grammar sources from the fork at pinned commits
3. build parser bindings automatically when needed
4. preserve graph behavior across local runs, CI, and package builds

## Expected outcomes

- Terraform files participate in the same graph as app code and docs
- mixed-language monorepos can connect Markdown, Python, and Terraform nodes
- CI and local builds no longer depend on tracked vendor source trees in git

## Notes

This plan is now partially implemented through pinned grammar provisioning. The remaining long-term concern is keeping language semantics and documentation aligned as the fork evolves.
