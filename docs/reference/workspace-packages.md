---
title: Generated workspace package inventory
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-16
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
| `graphql-orm` | `0.89.0` | `crates/graphql-orm` | `sqlite` | `graphql-orm-ai-tool-profiles` (dev-only), `graphql-orm-macros`, `graphql-orm-operation-catalog`, `graphql-orm-router-protocol` (dev-only) |
| `graphql-orm-ai` | `0.120.0` | `crates/graphql-orm-ai` | `sqlite` | `graphql-orm`, `graphql-orm` (dev-only), `graphql-orm-ai-tool-profiles`, `graphql-orm-storage` |
| `graphql-orm-ai-tool-profiles` | `0.20.0` | `crates/graphql-orm-ai-tool-profiles` | none | `graphql-orm-operation-catalog`, `graphql-orm-router-protocol` (dev-only) |
| `graphql-orm-backup` | `0.12.0` | `crates/graphql-orm-backup` | `local` | `graphql-orm` (optional), `graphql-orm-storage` |
| `graphql-orm-macros` | `0.89.0` | `crates/graphql-orm-macros` | `sqlite` | none |
| `graphql-orm-operation-catalog` | `0.7.0` | `crates/graphql-orm-operation-catalog` | none | `graphql-orm-router-protocol` (optional) |
| `graphql-orm-router` | `0.8.1` | `crates/graphql-orm-router` | none | `graphql-orm-router-protocol` |
| `graphql-orm-router-protocol` | `0.2.1` | `crates/graphql-orm-router-protocol` | none | none |
| `graphql-orm-storage` | `0.8.0` | `crates/graphql-orm-storage` | `local` | none |

External Git dependency:

- `agql-auth` requirement `^0.24.0`, source `git+https://github.com/Dastari/agql-auth.git?tag=v0.24.0`, consumed by `graphql-orm`, `graphql-orm-ai`, `graphql-orm-router`.

<!-- END GENERATED WORKSPACE PACKAGES -->

Workspace membership coordinates development but does not make companion
packages features of `graphql-orm`. See
[ADR-0009](../decisions/ADR-0009-nine-package-backend-neutral-discovery-boundaries.md).
