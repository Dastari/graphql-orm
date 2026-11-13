---
title: Generated workspace package inventory
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-13
review_by: 2027-02-13
supersedes: []
---

# Workspace packages

This table is generated from `cargo metadata`. Do not edit content between the
markers; run `python3 scripts/generate-workspace-inventory.py` after manifest
changes.

<!-- BEGIN GENERATED WORKSPACE PACKAGES -->

Versions are not recorded here so version bumps do not conflict. Read each
package version from its `Cargo.toml` and the released versions, external
requirements and pinned revisions from the release manifest.

| Package | Path | Default features | Direct internal dependencies |
| --- | --- | --- | --- |
| `graphql-orm` | `crates/graphql-orm` | `sqlite` | `graphql-orm-ai-tool-profiles` (dev-only), `graphql-orm-macros`, `graphql-orm-operation-catalog`, `graphql-orm-router-protocol` (dev-only) |
| `graphql-orm-ai` | `crates/graphql-orm-ai` | `sqlite` | `graphql-orm`, `graphql-orm` (dev-only), `graphql-orm-ai-tool-profiles`, `graphql-orm-operation-catalog`, `graphql-orm-storage` |
| `graphql-orm-ai-tool-profiles` | `crates/graphql-orm-ai-tool-profiles` | none | `graphql-orm-operation-catalog`, `graphql-orm-router-protocol` (dev-only) |
| `graphql-orm-backup` | `crates/graphql-orm-backup` | `local` | `graphql-orm` (optional), `graphql-orm-storage` |
| `graphql-orm-macros` | `crates/graphql-orm-macros` | `sqlite` | none |
| `graphql-orm-operation-catalog` | `crates/graphql-orm-operation-catalog` | none | `graphql-orm-router-protocol` (optional) |
| `graphql-orm-router` | `crates/graphql-orm-router` | none | `graphql-orm-router-protocol` |
| `graphql-orm-router-protocol` | `crates/graphql-orm-router-protocol` | none | none |
| `graphql-orm-storage` | `crates/graphql-orm-storage` | `local` | none |

External Git dependency:

- `agql-auth`, source repository `https://github.com/Dastari/agql-auth.git`, consumed by `graphql-orm`, `graphql-orm-ai`, `graphql-orm-router`.

<!-- END GENERATED WORKSPACE PACKAGES -->

Workspace membership coordinates development but does not make companion
packages features of `graphql-orm`. See
[ADR-0009](../decisions/ADR-0009-nine-package-backend-neutral-discovery-boundaries.md).
x
