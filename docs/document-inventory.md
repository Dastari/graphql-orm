---
title: Documentation disposition inventory
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2026-11-01
supersedes: []
---

# Documentation disposition inventory

This inventory records the first documentation-governance cleanup. It covers
tracked, first-party Markdown. Embedded skill packages under `crates/*/.agents/`
are vendor-maintained operational inputs and are outside this inventory.

Disposition meanings:

- **keep**: current and correctly located; add metadata and repair links.
- **rewrite**: retain the topic but replace stale or ledger-shaped content.
- **split**: divide mixed-purpose content between current guidance and history.
- **supersede**: replace with a new canonical document and retain the old one as
  archived evidence.
- **archive**: historical evidence that is not current guidance.
- **delete**: redundant content with no durable evidence value.

No first-party document is deleted in this run. Historical release, plan,
investigation, prompt, and handoff material is retained under `docs/archive/`
or `docs/plans/completed/`.

The source proposal also named FAME, frontend, `PrivilegedWrite`, and shell
substrate decisions. This repository contains none of those systems or active
requirements/design documents, so the cleanup does not fabricate authority for
them. The initial ADR set instead covers the equivalent durable decisions that
actually exist here: documentation authority, package boundaries, operation
metadata, authentication/authorization/assurance, restore readiness, and
streaming storage/backup layering.

## Workspace documents

| Source | Disposition | Canonical result |
| --- | --- | --- |
| `README.md` | rewrite | concise workspace entry point linked to `docs/README.md` |
| `AGENTS.md` | rewrite | workspace invariants plus documentation authority rules |
| `CHANGELOG.md` | keep | canonical `graphql-orm` release chronology |
| `MIGRATION.md` | keep | canonical `graphql-orm` migration history; repair moved links |
| `.github/PULL_REQUEST_TEMPLATE.md` | create | required documentation-impact declaration |
| `.cursor/rules/documentation.mdc` | create | editor-facing mirror of the authority and lifecycle rules |

## Central documentation

| Source | Disposition | Canonical result |
| --- | --- | --- |
| `docs/README.md` | rewrite | central authority index |
| `docs/agql-auth-bridge.md` | keep/move | `docs/reference/graphql-orm/agql-auth-bridge.md` |
| `docs/auth.md` | rewrite/move | `docs/architecture/authentication-and-authorization.md` |
| `docs/backends.md` | keep/move | `docs/reference/graphql-orm/backends.md` |
| `docs/backup.md` | keep/move | `docs/reference/graphql-orm/backup-runtime.md` |
| `docs/binary-keys-and-indexes.md` | keep/move | `docs/reference/graphql-orm/binary-keys-and-indexes.md` |
| `docs/composite-mutations.md` | keep/move | `docs/reference/graphql-orm/composite-mutations.md` |
| `docs/consumer-monorepo-migration-agent-prompt.md` | archive | `docs/archive/2026/consumer-monorepo-migration-agent-prompt.md` |
| `docs/cross-backend-tenant.md` | keep/move | `docs/reference/graphql-orm/cross-backend-tenant.md` |
| `docs/development.md` | split/rewrite | `docs/development/testing.md` and `docs/operations/release/process.md` |
| `docs/entities-and-relations.md` | keep/move | `docs/reference/graphql-orm/entities-and-relations.md` |
| `docs/error-codes.md` | keep/move | `docs/reference/graphql-orm/error-codes.md` |
| `docs/federation.md` | keep/move | `docs/reference/graphql-orm/federation.md` |
| `docs/getting-started.md` | rewrite/move | `docs/development/setup.md` |
| `docs/monorepo-consolidation.md` | supersede | `docs/plans/completed/monorepo-consolidation/README.md` |
| `docs/mssql.md` | keep/move | `docs/reference/graphql-orm/mssql.md` |
| `docs/operation-assurance.md` | keep/move | `docs/architecture/operation-assurance.md` |
| `docs/pagination-migration.md` | keep/move | `docs/reference/graphql-orm/pagination-migration.md` |
| `docs/portable-persistence.md` | keep/move | `docs/architecture/portable-persistence.md` |
| `docs/postgres-testing.md` | rewrite/move | `docs/operations/runbooks/postgres-testing.md` |
| `docs/postgres.md` | keep/move | `docs/reference/graphql-orm/postgres.md` |
| `docs/read-projections.md` | keep/move | `docs/reference/graphql-orm/read-projections.md` |
| `docs/release-notes.md` | archive | `docs/archive/2026/graphql-orm-release-notes.md`; `CHANGELOG.md` is authoritative |
| `docs/repository-only-entities.md` | keep/move | `docs/reference/graphql-orm/repository-only-entities.md` |
| `docs/resolver-operation-metadata.md` | keep/move | `docs/reference/graphql-orm/resolver-operation-metadata.md` |
| `docs/retention-maintenance.md` | keep/move | `docs/operations/runbooks/retention-maintenance.md` |
| `docs/runtime-and-writes.md` | keep/move | `docs/reference/graphql-orm/runtime-and-writes.md` |
| `docs/runtime-queries.md` | keep/move | `docs/reference/graphql-orm/runtime-queries.md` |
| `docs/runtime-records.md` | keep/move | `docs/reference/graphql-orm/runtime-records.md` |
| `docs/runtime-relations.md` | keep/move | `docs/reference/graphql-orm/runtime-relations.md` |
| `docs/runtime-schema-ir.md` | keep/move | `docs/reference/graphql-orm/runtime-schema-ir.md` |
| `docs/schema-management.md` | keep/move | `docs/reference/graphql-orm/schema-management.md` |
| `docs/schema-modules-and-leases.md` | keep/move | `docs/architecture/schema-modules-and-leases.md` |
| `docs/storage-streaming-range-boundary.md` | archive/investigation | `docs/investigations/2026/storage-streaming-range-boundary.md` |
| `docs/strict-authorization.md` | keep/move | `docs/reference/graphql-orm/strict-authorization.md` |

New central documents are the system context, package inventory, governance
ADRs, templates, release process, GraphQL workflow, storage/backup boundary,
incident index, and the current AI production-readiness plan.

## Component-local documentation

The following package documents remain beside their code because they explain
package-local mechanics. They receive metadata, current workspace dependency
guidance, and repaired links:

- all package `README.md`, `AGENTS.md`, `CHANGELOG.md`, and `MIGRATION.md` files;
- all topical `crates/graphql-orm-ai/docs/*.md` guides except the two entries
  below;
- backup `architecture.md`, `cloud-provider-direction.md`,
  `restore-semantics.md`, `smb.md`, `snapshot-format.md`, and `usage.md`;
- storage `architecture.md`, `backup-integration.md`, `blob-store.md`,
  `development.md`, `native-smb.md`, `recording-streams.md`, `streaming.md`, and
  `usage.md`.

| Source | Disposition | Canonical result |
| --- | --- | --- |
| `crates/graphql-orm-ai/docs/completion-plan.md` | supersede/archive | `docs/archive/2026/graphql-orm-ai-completion-ledger.md` and `docs/plans/active/ai-production-readiness/README.md` |
| `crates/graphql-orm-ai/docs/implementation-status.md` | split/rewrite | concise local current-state page plus `docs/archive/2026/graphql-orm-ai-implementation-ledger.md` |
| `crates/graphql-orm-backup/docs/digitise-native-smb.md` | archive | `docs/archive/2026/digitise-native-smb-integration-brief.md` |
| `crates/graphql-orm-backup/docs/graphql-orm-agent-brief.md` | archive | `docs/archive/2026/graphql-orm-backup-agent-brief.md` |
| `crates/graphql-orm-backup/docs/plan.md` | supersede/archive | `docs/archive/2026/graphql-orm-backup-plan.md` |
| `crates/graphql-orm-backup/docs/provider-roadmap.md` | supersede/archive | `docs/plans/backlog/backup-providers/README.md` plus `docs/archive/2026/graphql-orm-backup-provider-roadmap.md` |
| `crates/graphql-orm-storage/docs/agent-update.md` | archive | `docs/archive/2026/graphql-orm-storage-agent-update.md` |
| `crates/graphql-orm-storage/docs/plan.md` | supersede/archive | `docs/archive/2026/graphql-orm-storage-plan.md` |
| `crates/graphql-orm-storage/docs/provider-roadmap.md` | supersede/archive | `docs/plans/backlog/storage-providers/README.md` plus `docs/archive/2026/graphql-orm-storage-provider-roadmap.md` |
| `crates/graphql-orm-storage/docs/release-notes.md` | archive | `docs/archive/2026/graphql-orm-storage-release-notes.md`; package `CHANGELOG.md` is authoritative |

## Enforcement scope

CI governs all tracked first-party `*.md` files. It excludes embedded
`crates/*/.agents/**` skill packages and the GitHub pull-request form, whose
syntax is owned by their respective tools. CI checks required metadata,
metadata enums and review dates, local links, unique ADR numbers, active-plan
placement, forbidden stale paths, generated package inventory drift, and the
pull-request documentation-impact declaration.
