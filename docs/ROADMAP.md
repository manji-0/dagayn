# Roadmap

<!-- derived-from ./FEATURES.md -->
<!-- derived-from ./RECIPES.md -->

`dagayn` is being shaped around real-world repository analysis rather than benchmark-only feature growth.

Current direction:

- better infrastructure-aware review for Terraform-heavy repositories
- stronger mixed-language and monorepo support
- continued refinement of repo-root-relative graph semantics
- better MCP ergonomics for terminal and editor agents
- tighter graph-backed refactor workflows
- continued documentation cleanup so the fork stays self-describing

Areas that still need work:

- deeper cross-language resolution in some ecosystems
- measuring the SCIP overlay (`build --scip`) for Go, Java/Kotlin, C/C++, C#, Ruby, Dart, PHP, and R on representative repositories, and making each authoritative where it beats dagayn's own resolution ([plans/SCIP-CALL-RESOLUTION.md](./plans/SCIP-CALL-RESOLUTION.md))
- Python support for the SCIP overlay beyond filling, and freezing the inference passes it makes redundant
- broader flow coverage outside the strongest language integrations

Shipped docs recipes for single-repo watch, multi-repo registry/daemon, and
optional embedding providers live in [RECIPES.md](./RECIPES.md).
