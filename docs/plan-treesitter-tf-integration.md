# Terraform grammar design note

This file records the fork's Terraform parser direction as implemented, not an upstream planning artifact.

## Why Terraform is bundled

Terraform support is a defining part of `dagayn`, so the fork vendors a grammar instead of relying only on whatever happens to ship in a generic language bundle.

## What the parser extracts

The Terraform layer captures:

- block kinds such as `resource`, `data`, `module`, `variable`, `output`, `local`, `provider`, and `terraform`
- references between blocks and attributes
- built-in function calls used inside expressions
- file participation in blast-radius analysis and review tooling

## Why this matters

In infra-heavy repositories, code review often spans app code, generated docs, and Terraform changes in one PR. The fork treats that as a first-class use case rather than an extension point.
