---
title: Generated workspace package inventory
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-01
supersedes: []
---

# Workspace packages

This table is generated from `cargo metadata`. Do not edit content between the
markers; run `python3 scripts/generate-workspace-inventory.py` after manifest
changes.

<!-- BEGIN GENERATED WORKSPACE PACKAGES -->

| Package | Version | Path | Default features | Direct internal dependencies |
| --- | --- | --- | --- | --- |
| `graphql-orm` | `0.18.0` | `crates/graphql-orm` | `sqlite` | `graphql-orm-macros`, `graphql-orm-router-protocol` (optional) |
| `graphql-orm-ai` | `0.61.0` | `crates/graphql-orm-ai` | `sqlite` | `graphql-orm`, `graphql-orm-backup` (optional), `graphql-orm-storage` |
| `graphql-orm-backup` | `0.7.0` | `crates/graphql-orm-backup` | `local` | `graphql-orm` (optional), `graphql-orm-storage` |
| `graphql-orm-macros` | `0.18.0` | `crates/graphql-orm-macros` | `sqlite` | none |
| `graphql-orm-router` | `0.1.0` | `crates/graphql-orm-router` | none | `graphql-orm-router-protocol` |
| `graphql-orm-router-protocol` | `0.1.0` | `crates/graphql-orm-router-protocol` | none | none |
| `graphql-orm-storage` | `0.6.0` | `crates/graphql-orm-storage` | `local` | none |

External exact-revision dependency:

- `agql-auth` requirement `^0.14.0`, source `git+https://github.com/Dastari/agql-auth.git?rev=413fda3435f060604cd653c11e2cc18a668aace1`, consumed by `graphql-orm`, `graphql-orm-ai`, `graphql-orm-router`.

<!-- END GENERATED WORKSPACE PACKAGES -->

Workspace membership coordinates development but does not make companion
packages features of `graphql-orm`. See
[ADR-0007](../decisions/ADR-0007-seven-package-workspace-boundaries.md).
