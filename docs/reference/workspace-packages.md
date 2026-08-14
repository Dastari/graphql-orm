---
title: Generated workspace package inventory
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-11
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
| `graphql-orm` | `0.22.1` | `crates/graphql-orm` | `sqlite` | `graphql-orm-macros`, `graphql-orm-operation-catalog`, `graphql-orm-router-protocol` (dev-only) |
| `graphql-orm-ai` | `0.78.5` | `crates/graphql-orm-ai` | `sqlite` | `graphql-orm`, `graphql-orm-ai-tool-profiles`, `graphql-orm-storage` |
| `graphql-orm-ai-tool-profiles` | `0.4.1` | `crates/graphql-orm-ai-tool-profiles` | none | `graphql-orm-operation-catalog`, `graphql-orm-router-protocol` (dev-only) |
| `graphql-orm-backup` | `0.7.1` | `crates/graphql-orm-backup` | `local` | `graphql-orm` (optional), `graphql-orm-storage` |
| `graphql-orm-macros` | `0.22.1` | `crates/graphql-orm-macros` | `sqlite` | none |
| `graphql-orm-operation-catalog` | `0.2.0` | `crates/graphql-orm-operation-catalog` | none | `graphql-orm-router-protocol` (optional) |
| `graphql-orm-router` | `0.1.4` | `crates/graphql-orm-router` | none | `graphql-orm-router-protocol` |
| `graphql-orm-router-protocol` | `0.2.1` | `crates/graphql-orm-router-protocol` | none | none |
| `graphql-orm-storage` | `0.6.1` | `crates/graphql-orm-storage` | `local` | none |

External exact-revision dependency:

- `agql-auth` requirement `^0.15.0`, source `git+https://github.com/Dastari/agql-auth.git?rev=e841ffd382082ad7419be259fe957f949b956ff7`, consumed by `graphql-orm`, `graphql-orm-ai`, `graphql-orm-router`.

<!-- END GENERATED WORKSPACE PACKAGES -->

Workspace membership coordinates development but does not make companion
packages features of `graphql-orm`. See
[ADR-0009](../decisions/ADR-0009-nine-package-backend-neutral-discovery-boundaries.md).
