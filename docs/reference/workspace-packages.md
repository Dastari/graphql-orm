---
title: Generated workspace package inventory
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-01
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
| `graphql-orm` | `0.17.0` | `crates/graphql-orm` | `sqlite` | `graphql-orm-macros` |
| `graphql-orm-ai` | `0.58.0` | `crates/graphql-orm-ai` | `sqlite` | `graphql-orm`, `graphql-orm-backup` (optional), `graphql-orm-storage` |
| `graphql-orm-backup` | `0.7.0` | `crates/graphql-orm-backup` | `local` | `graphql-orm` (optional), `graphql-orm-storage` |
| `graphql-orm-macros` | `0.17.0` | `crates/graphql-orm-macros` | `sqlite` | none |
| `graphql-orm-storage` | `0.6.0` | `crates/graphql-orm-storage` | `local` | none |

External exact-revision dependency:

- `agql-auth` requirement `^0.13.0`, source `git+https://github.com/Dastari/agql-auth.git?rev=d6b9cef663d52125c52f3fb90d4155ee25d34775`, consumed by `graphql-orm`, `graphql-orm-ai`.

<!-- END GENERATED WORKSPACE PACKAGES -->

Workspace membership coordinates development but does not make companion
packages features of `graphql-orm`. See
[ADR-0002](../decisions/ADR-0002-workspace-package-boundaries.md).
