---
title: Cumulative upgrade guide
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-25
review_by: 2027-02-15
supersedes: []
---

# Cumulative upgrade guide

This guide lists every workspace release, newest first, so a host that is many
releases behind can plan its upgrade in one pass. Each entry names the package
version that carries the change, marks it **Action needed** or **No action**,
and links to the detailed section in the package's `MIGRATION.md` or
`CHANGELOG.md`.

The package `MIGRATION.md` and `CHANGELOG.md` files stay authoritative for what
each change requires (see
[ADR-0001](../decisions/ADR-0001-documentation-authority-and-lifecycle.md)).
This guide is an index into them. Each summary is a short extract from the
linked section. Read the linked section before you act on an entry.

## How to upgrade across many releases

Choose the target workspace release, then work in this order.

1. **Rebind.** Point every workspace package at the one target workspace tag
   (`tag = "workspace-YYYY.MM.DD.N"`) and record the commit from that release's
   manifest, as its release notes describe. Use the `agql-auth` selector the
   target release requires for every direct Auth dependency, so host, ORM, AI
   and router share one Auth source. Then refresh the lockfile, including any
   nested lockfiles in your own fixtures. The **Workspace** entries and the
   Auth-alignment entries below name each selector change.
2. **Re-approve.** Regenerate the artefacts your deployment reviews and pins,
   and review them again before you ship: SDL, operation catalogues, router
   descriptors, AI capability manifests and indexes, and provider-session
   bindings, as the entries in your range require. Several
   releases change fingerprints or authorization declarations, so a binding
   approved on an older release may no longer match.
3. **Migrate.** Work through every **Action needed** entry after your current
   release, up to and including the target, oldest first. That means reading
   this page from the bottom of your range upwards. Apply source changes, AI
   schema-module versions and managed schema migrations as each linked section
   says. Re-plan unapplied migration plans with the target version before you
   execute them.

Entries marked **No action** need nothing beyond the rebind. An entry marked
**See changelog** has no migration section of its own. Read the linked
changelog to decide.

## Reading an entry

- **Packages** lists every package version in the release. Versions in bold
  changed since the previous workspace release. `graphql-orm` and
  `graphql-orm-macros` always move together.
- A package can advance by several versions in one workspace release. The
  intermediate versions were merged to main but never tagged, so each one has
  its own entry under the release that first published it.
- **Workspace** entries are workspace-wide notes from the root `MIGRATION.md`
  that have no package version, such as Auth selector changes. They are listed
  under the first release that contained them.
- `workspace-2026.08.12.1` is the oldest tagged release. For earlier history,
  see the package changelogs.

## Releases

### Unreleased

Changes merged to main after the latest workspace release.

Versions on main (bold where they differ from the latest release): **`graphql-orm` 0.227.0**; **`graphql-orm-macros` 0.227.0**; `graphql-orm-operation-catalog` 0.16.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.31.0; `graphql-orm-ai-tool-profiles` 0.32.0; **`graphql-orm-ai` 0.167.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.19.0.

- **Action needed** — ORM/macros 0.227.0 removes the 0.215.0 ASCII VARCHAR conversion and validates unsigned inputs before selecting update and upsert targets (#860). Hosts relying on that seek under a legacy `SQL_` collation can move to a Windows collation or index a persisted computed Unicode column. [Migration](../../MIGRATION.md#02260-to-02270-sql-server-varchar-and-bulk-update-validation)
- **No action** unless opting in to generated nested create in ORM/macros 0.226.0 (#353). Opted-in parents expose typed child lists on their create input. [Migration](../../MIGRATION.md#02250-to-02260-generated-nested-create)
- **Action needed** for hosts that construct their own `AiApprovalBinding` (use `prepared.argument_hash()`); **no action** for hosts using the built-in AI approval service. AI 0.167.0 verifies the stored provider plan hash when executing an approved generated mutation. Hosts that construct a binding for a prepared generated mutation must use its plan digest. [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01660-to-01670-approved-generated-mutations)
- **Action needed** — ORM/macros 0.225.0 keep unparseable partial-index predicates opaque on SQLite and PostgreSQL and mark filtered SQL Server indexes whose definition is unavailable (#880). Existing databases with such indexes see new plan steps or reports. [Migration](../../MIGRATION.md#02240-to-02250-opaque-partial-index-predicates)
- **No action** for AI 0.166.0. The legacy approval-index upgrade still admits the original plain index with ORM 0.225.0. [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01650-to-01660)
- **No action** — ORM/macros 0.224.0 add an opt-in that lets a mutation hook's nested writes be authorized from the originating request, and pass an explicit `PolicyAuthority` to entity, row and repository field policies (#291). [Migration](../../MIGRATION.md#02230-to-02240-explicit-authority-for-hooks-and-workers)
- **No action** — AI 0.165.0: host-owned repository field decisions on the crate's internal handle go to the host policy's authority-aware methods (#291). A host that overrides those methods should check it admits them. [Migration](../../crates/graphql-orm-ai/MIGRATION.md#01640-to-01650-authority-aware-host-field-decisions)
- **Action needed** for drifted SQL Server columns in ORM/macros 0.223.0 and AI 0.164.0. Check live types for opted-in columns and review plan guides and Query Store forced plans. Generated exact string filters can seek on declared BIN2 Unicode key columns, including AI run state (#867). [ORM migration](../../MIGRATION.md#02220-to-02230-sql-server-exact-string-filter-seeks) · [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01630-to-01640)
- **No action** — ORM/macros 0.222.0 adds three opt-in options for generated upserts: a private conflict key bound from the verified request (`upsert_bound_key`), a typed, size-bounded transient input for the transform and hooks (`transient_input`), and a hook acknowledgement that answers a retry without writing (`upsert_acknowledge`) (#908). [Migration](../../MIGRATION.md#02210-to-02220-principal-bound-upserts)
- **No action** — AI tool profiles 0.32.0 refuses GraphQL tool profiles, loaded manifests and typed capabilities that would carry a transient mutation argument (`@transientInput`), so AI runs never retain one (#908). Validate loaded manifests against the owning SDL before registering them. [Migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0320-transient-mutation-arguments)
- **No action** — ORM/macros 0.221.0 and operation catalogue 0.16.0 add opt-in, bounded replay for generated Changed streams (#668). [Migration](../../MIGRATION.md#02200-to-02210-generated-changed-replay)

### workspace-2026.11.25.2

Commit `b3fead984bde` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.25.2)

Packages (changed in bold): **`graphql-orm` 0.220.0**; **`graphql-orm-macros` 0.220.0**; `graphql-orm-operation-catalog` 0.15.0; `graphql-orm-storage` 0.18.3; **`graphql-orm-backup` 0.31.0**; **`graphql-orm-ai-tool-profiles` 0.31.0**; **`graphql-orm-ai` 0.163.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.18.0.

- **No action** — ORM/macros 0.220.0 and backup 0.31.0 keep legacy journal keys by default. PostgreSQL hosts can opt into canonical keys and start a new full backup chain (#592). [ORM migration](../../MIGRATION.md#02190-to-02200-canonical-postgresql-journal-keys) · [backup migration](../../crates/graphql-orm-backup/MIGRATION.md#0300-to-0310-canonical-postgresql-journal-keys)
- **Action needed** when host code matches `ProviderError::InvalidConfiguration` from adapter construction, or when `provider_profile_endpoint_issues` lists profiles. No schema or data migration. AI 0.163.0 identifies legacy skill schemas and validates endpoint paths on profile saves. AI tool profiles 0.31.0 adds the typed path error (#816, #817). Listed profiles already failed adapter initialization before this release. Adapter path failures now return `ProviderError::InvalidEndpointPath` instead of `InvalidConfiguration`. Any edit to a grandfathered profile, including a display-name-only edit, fails until its URL is corrected. [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01620-to-01630-skill-schemas-and-provider-endpoints) · [AI tool profiles migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0310-provider-endpoint-path-error)
- **Action needed** — ORM/macros 0.219.0 enforces target query assurance on runtime node lookups, runtime relations and generated Federation entity lookups (#902). Handle step-up errors on these reads. [Migration](../../MIGRATION.md#02180-to-02190-indirect-query-assurance)

### workspace-2026.11.25.1

Commit `7e5df65822ab` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.25.1)

Packages (changed in bold): **`graphql-orm` 0.218.0**; **`graphql-orm-macros` 0.218.0**; **`graphql-orm-operation-catalog` 0.15.0**; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.30.0; **`graphql-orm-ai-tool-profiles` 0.30.0**; **`graphql-orm-ai` 0.162.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.18.0.

- **No action** — ORM/macros 0.218.0 adds the opt-in `server_only_key` relation option: a composite relation joins on a private partition key that stays off every generated GraphQL surface; handwritten resolvers, trusted SQL ordering and search documents remain the host's responsibility (#898). [Migration](../../MIGRATION.md#02170-to-02180-server-only-relation-join-keys)
- **Action needed** — ORM/macros 0.217.0 fixes relation traversal assurance (#899). Handle step-up errors when a relation target has required query assurance. [Migration](../../MIGRATION.md#02160-to-02170-relation-target-assurance)
- **No action** — Operation catalogue 0.15.0 marks field-policy-guarded relationships as `subgraph_only` (#544). [Migration](../../MIGRATION.md#operation-catalogue-0150)
- **No action** — ORM/macros 0.216.0 let PostgreSQL choose the join for RLS search documents (#555). [Migration](../../MIGRATION.md#02150-to-02160-postgresql-rls-search)
- **No action** — AI tool profiles 0.30.0 and AI 0.162.0 speed up capability catalogue compilation without changing fingerprints, provider definitions or describe output (#865). [Migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0300-capability-catalogue-compilation)
- **No action** — ORM/macros 0.215.0 adds opt-in declared SQL Server parameter types and parses column identifiers in the ALTER COLUMN collation lookup (#522, #849). [Migration](../../MIGRATION.md#02140-to-02150-sql-server-parameter-types-and-column-identifiers)
- **No action** — AI 0.161.0 uses ORM 0.215.0 and adapts one internal SQL Server call; no AI migration (#522). [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01600-to-01610)

### workspace-2026.11.24.4

Commit `cf7800e200f4` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.24.4)

Packages (changed in bold): **`graphql-orm` 0.214.0**; **`graphql-orm-macros` 0.214.0**; `graphql-orm-operation-catalog` 0.14.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.30.0; `graphql-orm-ai-tool-profiles` 0.29.0; **`graphql-orm-ai` 0.160.0**; `graphql-orm-router-protocol` 0.3.1; **`graphql-orm-router` 0.18.0**.

- **Action needed** — ORM/macros 0.214.0, AI 0.160.0 and router 0.18.0 use `agql-auth` `v0.27.0` (#857). Upgrade direct Auth dependencies with these packages and review the Auth store and validator changes. [ORM migration](../../MIGRATION.md#02130-to-02140-agql-auth-v0270) · [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01590-to-01600) · [Router migration](../../crates/graphql-orm-router/MIGRATION.md#0170-to-0180-agql-auth-v0270)
- **No action** — ORM/macros 0.213.0 fixes parallel compile UI and consumer fixture setup (#887, #888). [Migration](../../MIGRATION.md#02120-to-02130-test-harness-coordination)
- **No action** — AI 0.159.0 adds opt-in resolved entity-reference delivery
  for Codex app-server (#721).
  [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01580-to-01590)

### workspace-2026.11.24.3

Commit `5452163efcba` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.24.3)

Packages (changed in bold): **`graphql-orm` 0.212.0**; **`graphql-orm-macros` 0.212.0**; `graphql-orm-operation-catalog` 0.14.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.30.0; `graphql-orm-ai-tool-profiles` 0.29.0; **`graphql-orm-ai` 0.158.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.212.0 enables an admitted owned SQLite plan to drop one unused column while preserving unrelated indexes and incoming foreign keys (#882). [Migration](../../MIGRATION.md#02110-to-02120-scoped-sqlite-column-drop)
- **Action needed** for AI 0.158.0. Apply the guarded external run-state index migration on SQL Server and drain workers on all backends before epoch rotation (#848). [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01570-to-01580)

### workspace-2026.11.24.2

Commit `2f2b6c9d78da` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.24.2)

Packages (changed in bold): **`graphql-orm` 0.211.0**; **`graphql-orm-macros` 0.211.0**; **`graphql-orm-operation-catalog` 0.14.0**; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.30.0; `graphql-orm-ai-tool-profiles` 0.29.0; `graphql-orm-ai` 0.157.0; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.211.0 and operation catalogue 0.14.0 use schema-bound metadata when described roots return a registered generated entity (#883). [Migration](../../MIGRATION.md#02100-to-02110-described-root-result-metadata)
- **No action** — ORM/macros 0.210.0 and operation catalogue 0.13.0: opt-in `update_expected_version = "required"` and `update_assurance(select, levels)` on generated updates. The catalogue lists conditional assurance levels. Other entities are unchanged (#851). [Migration](../../MIGRATION.md#02090-to-02100-required-expectedversion-and-conditional-update-assurance-operation-catalogue-0130)

### workspace-2026.11.24.1

Commit `10fa8e866e55` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.24.1)

Packages (changed in bold): **`graphql-orm` 0.209.0**; **`graphql-orm-macros` 0.209.0**; `graphql-orm-operation-catalog` 0.12.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.30.0; **`graphql-orm-ai-tool-profiles` 0.29.0**; **`graphql-orm-ai` 0.157.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — AI tool profiles 0.29.0 accepts the server-injected page limit in generated keyset-list plans (#881). Capability fingerprints stay stable. [Migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0290-generated-keyset-page-limits)
- **Action needed** for stores upgrading from a pre-0.83 AI schema with
  the legacy approval lookup index. AI 0.157.0 checks the live index and
  refuses duplicate tool-call keys before DDL. ORM/macros 0.209.0 expose
  the read-only preflight for external migrations (#878).
  [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01560-to-01570) · [ORM migration](../../MIGRATION.md#02080-to-02090-external-migration-preflight)

### workspace-2026.11.23.3

Commit `69e83e1b874a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.23.3)

Packages (changed in bold): **`graphql-orm` 0.208.0**; **`graphql-orm-macros` 0.208.0**; `graphql-orm-operation-catalog` 0.12.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.30.0; **`graphql-orm-ai-tool-profiles` 0.28.0**; **`graphql-orm-ai` 0.156.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.208.0 add opt-in bounded set deletes to typed transactions (#852). [Migration](../../MIGRATION.md#02070-to-02080-bounded-set-based-deletes)
- **No action** — AI 0.156.0 and AI tool profiles 0.28.0: a model-context benchmark for the fixed capability broker, and broker calls whose cost no longer grows with the index for a host that supplies one index; describe and `describe_type` no longer regenerate compact schemas, and index-set admission refuses a deserialized index that breaks its own limits or stores an entry under another ID; compiled indexes are unaffected (#551). No binding, fingerprint or schema change. [AI](../../crates/graphql-orm-ai/MIGRATION.md#01550-to-01560-capability-model-context-benchmark) · [tool profiles](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0280-index-set-and-signature-reuse)
- **No action** — ORM/macros 0.207.0: security fix (#876). SQLite renders every closure read predicate in the original form again; from 0.200.0 through 0.206.0, a closure whose pointer column's collation treats values as equal that the key column distinguishes (for example a `NOCASE` or `RTRIM` pointer with a `BINARY` key) could reach a different verdict, so an `Any` closure could deny rows the original admits. On PostgreSQL and SQL Server, directly nested closures over built-in column tests can use fewer target SQL copies and bind parameters (#785). [Migration](../../MIGRATION.md#02060-to-02070-sqlite-closure-rendering-and-nested-closure-read-predicates)

### workspace-2026.11.23.2

Commit `70a5010ecf57` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.23.2)

Packages (changed in bold): **`graphql-orm` 0.206.0**; **`graphql-orm-macros` 0.206.0**; `graphql-orm-operation-catalog` 0.12.0; `graphql-orm-storage` 0.18.3; **`graphql-orm-backup` 0.30.0**; `graphql-orm-ai-tool-profiles` 0.27.1; `graphql-orm-ai` 0.155.0; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.206.0 and backup 0.30.0 support the SQLite
  and PostgreSQL backup adapters in builds that enable other ORM backends
  (#870).
  [ORM migration](../../MIGRATION.md#02050-to-02060-backend-typed-backup-runtime) · [Backup migration](../../crates/graphql-orm-backup/MIGRATION.md#0290-to-0300-adapter-backend-selection)
- **No action** — ORM/macros 0.205.0 share the generated Changed stream implementation to reduce compile work (part of #677 and #358). [Migration](../../MIGRATION.md#02040-to-02050-generated-changed-subscriptions)
- **No action** — ORM/macros 0.205.0 support combining `changed_started = true` and `key_only_deletes` on generated Changed subscriptions. [Changelog](../../CHANGELOG.md#unreleased---ormmacros-02050)
- **No action** — ORM/macros 0.204.0 and backup 0.29.0 add an opt-in for
  databases whose backup and restore are owned by another product.
  [ORM migration](../../MIGRATION.md#02030-to-02040-externally-managed-backup-and-restore) · [Backup migration](../../crates/graphql-orm-backup/MIGRATION.md#0281-to-0290-externally-managed-backup-and-restore)

### workspace-2026.11.23.1

Commit `249433ad5128` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.23.1)

Packages (changed in bold): **`graphql-orm` 0.203.0**; **`graphql-orm-macros` 0.203.0**; `graphql-orm-operation-catalog` 0.12.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.28.1; `graphql-orm-ai-tool-profiles` 0.27.1; **`graphql-orm-ai` 0.155.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — AI 0.155.0 supports composing exported AI roots through
  `schema_roots!` with either GraphQL naming setting (#858).
  [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01540-to-01550)
- **Action needed** when adopting SQL Server with AI 0.154.0. Install the restore marker, apply the external schema plan, and use an explicitly writable pool (#813). [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01530-to-01540)
- **Action needed** for AI 0.154.0 on SQLite, PostgreSQL, and SQL Server. Drain active workers before the module epoch rotates and invalidates old leases. SQLite and PostgreSQL need no data migration (#813). [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01530-to-01540)
- **Action needed** for ORM/macros 0.203.0 on SQL Server. Review cleanup jobs and cascaded deletes before applying new append-only triggers. There is no trigger opt-out for append-only entities (#813). [ORM migration](../../MIGRATION.md#02020-to-02030-durable-ai-sql-server-prerequisites)
- **Action needed** for ORM/macros 0.203.0 on SQL Server. Replan and resolve declarations that map to the same physical table. SQL Server now refuses these conflicts (#813). [ORM migration](../../MIGRATION.md#02020-to-02030-durable-ai-sql-server-prerequisites)

### workspace-2026.11.22.4

Commit `ac210f98e867` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.22.4)

Packages (changed in bold): **`graphql-orm` 0.202.0**; **`graphql-orm-macros` 0.202.0**; **`graphql-orm-operation-catalog` 0.12.0**; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.28.1; `graphql-orm-ai-tool-profiles` 0.27.1; `graphql-orm-ai` 0.153.0; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.202.0 groups additional integration tests. Source checkouts can use the grouped selectors to run individual tests. [Migration](../../MIGRATION.md#02010-to-02020-test-and-repository-hygiene)
- **No action** — ORM/macros 0.202.0 documents the blocking filesystem operations in `write_migration_file`. [Changelog](../../CHANGELOG.md#unreleased---ormmacros-02020)
- **No action** — ORM/macros 0.201.0 and operation catalogue 0.12.0 return an error for duplicate entity identities through the generated resolver catalogue and fallible schema builders (#829). [Migration](../../MIGRATION.md#02000-to-02010-duplicate-entity-identities-operation-catalogue-0120)
- **No action** — ORM/macros 0.200.0: standalone closure read predicates can use fewer target SQL copies, and the reference describes incomplete descendant walks and non-unique-key fan-out. [Migration](../../MIGRATION.md#01990-to-02000-closure-read-predicates)

### workspace-2026.11.22.3

Commit `57feee84d00c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.22.3)

Packages (changed in bold): **`graphql-orm` 0.199.0**; **`graphql-orm-macros` 0.199.0**; `graphql-orm-operation-catalog` 0.11.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.28.1; `graphql-orm-ai-tool-profiles` 0.27.1; **`graphql-orm-ai` 0.153.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.199.0: derives can opt out of generated mutation and subscription fields. Default schemas keep their original Rust root structs (#677). [Migration](../../MIGRATION.md#01980-to-01990-generated-mutation-and-subscription-roots)
- **No action** — ORM/macros 0.199.0: generated writes, scalar getters, and relation resolvers return field errors when database context is missing (#542). [Migration](../../MIGRATION.md#01980-to-01990-generated-mutation-and-subscription-roots)
- **No action** — ORM/macros 0.199.0: generated writes and Changed subscriptions qualify standard paths, mutation shape errors point to the declaration, and implicit update timestamps use backend identifier quoting (#542). [Changelog](../../CHANGELOG.md#unreleased---ormmacros-01990)
- **No action** — ORM/macros 0.198.0: AI restore pages check payload size before
  loading on SQLite and PostgreSQL (#532). [Migration](../../MIGRATION.md#01970-to-01980-restore-page-size-checks)
- **Action needed** — AI 0.153.0: configure provider profile and adapter instance
  re-admission and use the pinned effort and prepared manifest for compaction. Pin a legacy
  session with verified evidence or start a new chat (#532).
  [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01520-to-01530)
- **Action needed** — AI 0.153.0: direct callers must start a replacement
  lease before adopting its completed tool batch (#532).
  [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01520-to-01530)
- **No action** — AI 0.153.0: restore collection reads bounded pages, batches
  attachment parent lookups, and fails closed when its aggregate row-data
  budget is exceeded (#532).
  [Restore collection](../../crates/graphql-orm-ai/docs/recovery-and-restore.md)
- **No action** — AI 0.153.0: bounded debug codes identify content-protection
  failures, and provider tool validation reuses validators for exact offers
  (#818, #819).
  [AI changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01530)
- **No action** — ORM/macros 0.197.0. SQL Server scalar reads preserve driver diagnostics and accept `SMALLINT` for boolean reads (#541). [Migration](../../MIGRATION.md#01960-to-01970-database-binding-and-decoding)
- **No action** — ORM/macros 0.197.0. The database binding guide describes typed UUID and binary NULLs, fallible JSON decoding, and the legacy text-backed array fallback (#541). [Migration](../../MIGRATION.md#01960-to-01970-database-binding-and-decoding)
- **No action** — ORM/macros 0.197.0. Runtime search supports collections with columns named `__gom_search_key` or `__gom_search_score` and keeps existing cursors and indexes compatible (#821). [Migration](../../MIGRATION.md#01960-to-01970-database-binding-and-decoding)

### workspace-2026.11.22.2

Commit `2edc9d18c167` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.22.2)

Packages (changed in bold): **`graphql-orm` 0.196.0**; **`graphql-orm-macros` 0.196.0**; **`graphql-orm-operation-catalog` 0.11.0**; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.28.1; `graphql-orm-ai-tool-profiles` 0.27.1; `graphql-orm-ai` 0.152.0; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.196.0. Hosts can opt in to typed computed-field invalidations with commit-only publishing and an explicit disclosure policy (#842). [Migration](../../MIGRATION.md#01950-to-01960-computed-field-invalidations)
- **No action** — ORM/macros 0.195.0 groups selected integration-test targets (#771). Application hosts can upgrade without configuration changes. [Migration](../../MIGRATION.md#01940-to-01950-integration-test-consolidation)
- **No action** — ORM/macros 0.194.0, operation catalog 0.11.0: `schema_roots!` also emits `graphql_orm_resolver_catalog()`, a separate document listing the generated relationship fields and Federation entity resolvers a schema serves, and the catalogue adds an opt-in RFC 8785 encoder. Existing SDL, catalogues, fingerprints and receiver bindings are unchanged (#544). [Migration](../../MIGRATION.md#01930-to-01940-resolver-catalogue-operation-catalogue-0110)

### workspace-2026.11.22.1

Commit `ddae79d28a19` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.22.1)

Packages (changed in bold): **`graphql-orm` 0.193.0**; **`graphql-orm-macros` 0.193.0**; `graphql-orm-operation-catalog` 0.10.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.28.1; `graphql-orm-ai-tool-profiles` 0.27.1; **`graphql-orm-ai` 0.152.0**; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.193.0: a `GraphQLEntity`-generated object can host handwritten resolver fields with `#[graphql_orm(object_fields)]` and `#[graphql_object_fields]`; entities that do not opt in are unchanged (#808). [Migration](../../MIGRATION.md#01920-to-01930-handwritten-fields-on-generated-objects)
- **No action** — ORM/macros 0.192.0: set `propagate_change_action = "updated"` on an upward relation to report child creates and deletes as parent updates (#812). [Migration](../../MIGRATION.md#01910-to-01920-propagated-parent-actions)
- **No action** — AI 0.152.0: the local-harness and Grok ACP adapters can send resolved entity references to their local process. Off by default: a registration opts in with `with_resolved_entity_reference_delivery`, and until then every reference is still the unavailable marker. Review what the process retains before opting in. The Codex app-server adapter stays marker-only (#721, #828). [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01512-to-01520-opt-in-entity-references-on-local-process-adapters)

### workspace-2026.11.21.3

Commit `14eff311f18e` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.21.3)

Packages (changed in bold): **`graphql-orm` 0.191.0, `graphql-orm-macros` 0.191.0**; `graphql-orm-operation-catalog` 0.10.0; `graphql-orm-storage` 0.18.3; **`graphql-orm-backup` 0.28.1**; `graphql-orm-ai-tool-profiles` 0.27.1; `graphql-orm-ai` 0.151.2; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **No action** — ORM/macros 0.191.0 and backup 0.28.1: use
  `Database::read_snapshot_transaction` or its auth-aware counterpart
  to opt in to policy-aware snapshot reads (#831). The optional ORM backup
  adapter refuses restore writes inside the snapshot callback.
  [ORM migration](../../MIGRATION.md#01900-to-01910-read-snapshot-transactions) · [Backup migration](../../crates/graphql-orm-backup/MIGRATION.md#0280-to-0281-read-snapshot-write-refusal)

### workspace-2026.11.21.2

Commit `3111331f3a31` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.21.2)

Packages (changed in bold): **`graphql-orm` 0.190.0, `graphql-orm-macros` 0.190.0**; `graphql-orm-operation-catalog` 0.10.0; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.28.0; `graphql-orm-ai-tool-profiles` 0.27.1; `graphql-orm-ai` 0.151.2; `graphql-orm-router-protocol` 0.3.1; `graphql-orm-router` 0.17.0.

- **Action needed** — ORM/macros 0.190.0: fix any keyset order with an incomplete null-order clause or trailing tokens (#542). [Migration](../../MIGRATION.md#01890-to-01900-generated-query-resolvers)
- **No action** — ORM/macros 0.190.0: generated query resolver emission is shared across writable and read-only entity profiles (#723), and generated queries, Changed subscriptions and the NodeId object field return field errors when required schema data is missing (#542). [Migration](../../MIGRATION.md#01890-to-01900-generated-query-resolvers)
- **No action** — ORM/macros 0.189.0: generated root search qualifies client filter columns with the entity table and keeps native search helper columns distinct from entity columns (#799). [Migration](../../MIGRATION.md#01880-to-01890-root-search-filter-binding)
- **No action** — AI tool profiles 0.27.1: replayable subscriptions reuse compiled plan and condition validators (#543). [Migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0271-subscription-validator-reuse)
- **No action** — AI 0.151.2: skill content protection emits fixed debug reason codes and the proposal registry reuses validators (#543). [Migration](../../crates/graphql-orm-ai/MIGRATION.md#01512-skill-content-protection-diagnostics)

### workspace-2026.11.21.1

Commit `5c92c045f810` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.21.1)

Packages (changed in bold): **`graphql-orm` 0.188.0, `graphql-orm-macros` 0.188.0**; **`graphql-orm-operation-catalog` 0.10.0**; `graphql-orm-storage` 0.18.3; **`graphql-orm-backup` 0.28.0**; **`graphql-orm-ai-tool-profiles` 0.27.0**; **`graphql-orm-ai` 0.151.1**; `graphql-orm-router-protocol` 0.3.1; **`graphql-orm-router` 0.17.0**.

- **Action needed** — ORM/macros 0.188.0: read predicates bind every column to the row it describes: through the table path at the root of a statement (`"records"."owner_id" = ?`) and through an alias that no table or alias in scope has, such as `__gom_row_1`, inside relation, `not_related`, closure and count subqueries. Policies built with the `ReadPredicate` constructors and generated where inputs admit the same rows (#796). Act if a table lacks a column a read policy names (schema drift, an unapplied migration): such a column used to resolve against an outer row, and the read now fails. Act if you implement `ReadVisibilityFilter` by hand: its SQL may now only be a single-row expression (no `SELECT`/`FROM`/`JOIN`/`WITH`/set operators, comments, `U&` forms or adjacent strings; use `related`, `not_related` or closures to read another table), and wherever its predicate is evaluated in a correlated subquery every column must be written as `__gom_self.<column>` with `table_name` returning the table. Act too if a PostgreSQL policy names a mixed-case column declared without quotes, if a policy is near the 1 MiB rendered-size budget, if a table is named `__gom_self`, `__gom_row_<n>`, `__gom_visibility_<n>` or `__gom_closure_<n>`, or if you compare relation-count ordering SQL text. [Migration](../../MIGRATION.md#01870-to-01880-read-predicate-columns-bound-to-their-row) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01880)

This is the single **breaking release**: every change that breaks hosts or invalidates stored fingerprints, bindings, approvals or sessions. The [migration guide](../../MIGRATION.md#01860-to-01870-breaking-release) has one section for it, in upgrade order; the entry numbers below are that section's.

Part 1, schema and storage:

- **Action needed** — ORM/macros 0.187.0 (1.1): default plans preserve undeclared host tables; lossy type conversions, integrity-removing steps, column constraint removal and retention-purge relaxation require the existing destructive gate, and widening is backend-aware. Re-plan and review pending migrations (#441, #568, #569). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (1.2): re-install PostgreSQL journal recorders; `TRUNCATE` and cascades into tables without recorders fail closed, and live validation detects host-added cascade gaps. Avoid role or database journal-off defaults (#592). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — AI 0.151.0, ORM/macros 0.187.0 (1.3): apply AI schema module `0.83.0` (three unique indexes); applying a migration refuses repeated unique keys before any DDL, and upgrading an AI store with a legacy webhook-receipt index needs `AiSchemaModule::admit_index_replacements` or explicit destructive admission (#532). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release) · [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01502-to-01510-breaking-release)

Part 2, build configuration and Rust API:

- **Action needed** — ORM/macros 0.187.0, AI 0.151.0 (2.1): the `resolver-case-*`, `argument-case-*` and `field-case-*` features (and AI's `graphql-case-pascal`) are replaced by the `GRAPHQL_ORM_RESOLVER_CASE`, `GRAPHQL_ORM_ARGUMENT_CASE`, `GRAPHQL_ORM_FIELD_CASE` (and `GRAPHQL_ORM_AI_CASE`) build environment variables; names are unchanged (#508). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — macros 0.187.0 (2.2): the unused `mysql` feature of `graphql-orm-macros` is removed; delete it from feature lists. [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (2.3): only for hosts that build a schema from generated roots by hand: install `GeneratedInputValidation`; without it, generated resolvers that take an input object refuse with `INPUT_VALIDATION_REQUIRED` (#753). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (2.4, 2.5): declare the schema page maximum and migrate oversized internal reads, including host `EntityQuery::paginate`/`EntitySearchQuery::paginate` reads, to exact-or-refused paging; resolve client pages in custom resolvers with `resolve_client_page` (#667, #674). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (2.6): add a fallback to exhaustive `SqlValue` matches and handle `Temporal` in host hooks reading date changes (#521). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (2.7): handle `CursorKeyTooLarge` in exhaustive runtime query/relation error matches, and match it instead of `ResourceLimit` plus `detail()` (#580). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (2.8): replace policy-free relation preload methods and direct `RelationLoader` use (prelude import, `new`, hand-built keys, string-key loads) with authorized queries; register the loader in handwritten schemas with `relation_data_loader` (#445). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — backup 0.28.0 (2.9): deletion results expose snapshot ids, and result/error types are non-exhaustive with a dedicated snapshot admission error. Update literals and exhaustive matches. [Migration](../../MIGRATION.md#01860-to-01870-breaking-release) · [backup migration](../../crates/graphql-orm-backup/MIGRATION.md#0272-to-0280-breaking-release)

Part 3, defaults and behaviour:

- **Action needed** — ORM/macros 0.187.0 (3.1): generated GraphQL bulk writes are narrowed to the caller's read visibility by default; rows a caller may write but not read need a `write_target_prefilter`, and `read_visibility` must not return `Err` for callers whose bulk writes should work. `narrow_bulk_writes_to_read_visibility(false)` opts out. `mssql_snapshot_bulk_write_targets` stays a separate opt-in, off (#752). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (3.2): a `Database` refuses a client page offset above 10,000 with `PAGE_LIMIT_EXCEEDED` by default; opt out with `max_page_offset(None)` or page deeper with keyset cursors (#456). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (3.3): assign public codes to deliberate callback refusals, or opt in to uncoded text exposure per `Database`/schema (#682). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (3.4): only for row policies that negate `ReadPredicate::related`: the relation is three-valued, so its negation no longer admits rows with a NULL key or only UNKNOWN related values; `not_related` is `related(..).not()`, and a relation chain deeper than ten levels cannot be negated (#770). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — ORM/macros 0.187.0 (3.5): use decimal strings for runtime `RecordVersion` and integer live keys, and regenerate descriptors (#528, item 4). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — router 0.17.0 (3.6): composed `@authenticated` and `@requiresScopes` are enforced on nested fields, fragments, aliases, interfaces and unions; a refused selection refuses the whole operation before any subgraph request. Released routers never enforced them, so keep subgraph guards (#729, #485). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release) · [router migration](../../crates/graphql-orm-router/MIGRATION.md#0168-to-0170-breaking-release)
- **Action needed** — AI 0.151.0 (3.7): `HighImpact` approvals always require recent MFA; the approval lifetime cap has a 7-day ceiling (#465). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)

Part 4, fingerprints, bindings, approvals and sessions:

- **Action needed** — ORM/macros 0.187.0, catalogue 0.10.0, AI tool profiles 0.27.0 (4.1): generated-operation fingerprints no longer include the Rust module path; every operation, catalogue and router-export fingerprint changes once. Regenerate exports and manifests and request stale approvals again (#683). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release)
- **Action needed** — AI 0.151.0 (4.2): fixed-broker provider sessions rebind once, then survive catalogue changes; `capability_stale` and `selection_stale` are separate codes (#551). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release) · [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01502-to-01510-breaking-release)
- **Action needed** — AI 0.151.0, AI tool profiles 0.27.0 (4.3): capability broker version 2 (compact signatures and `describe_type`, one entity capability per entity, inverted-index search) and the fixed broker by default for large surfaces, with an opt-out. Host capability policy must decide entity entries (#551). [Migration](../../MIGRATION.md#01860-to-01870-breaking-release) · [tool profiles migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0261-to-0270-breaking-release)

No action:

- **No action** — ORM/macros 0.187.0: opt in per SQL Server pool to native binding for temporal text; review legacy `datetime` storage and comparisons before enabling it (#521). [Migration](../../MIGRATION.md#also-in-this-release-no-action)
- **No action** — ORM/macros 0.187.0: opt-in key-only deletes; review key-existence disclosure before adopting them (#666). [Migration](../../MIGRATION.md#also-in-this-release-no-action)
- **No action** — router 0.17.0: the opt-in `refusalOutcome` setting adds `extensions.outcome: "refused"` to each error of an operation refused before any subgraph request; off by default (#764). [Migration](../../MIGRATION.md#also-in-this-release-no-action)
- **No action** — AI 0.151.0, AI tool profiles 0.27.0: runtime live key inputs compile under default string identities, and the runtime's own keyset windows larger than a host's page maximum are read completely in pages. [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01502-to-01510-breaking-release)
- **No action** — AI 0.151.1: warnings-denied Clippy and Rustdoc lanes pass for `postgres,provider-openai` and `mssql`; the changes are test-only gating and a doc link, with no runtime, API or persistence change (#814). [Migration](../../crates/graphql-orm-ai/MIGRATION.md#01510-to-01511-lint-and-documentation-lanes)

### workspace-2026.11.20.1

Commit `ffd85c8075a7` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.20.1)

Packages (changed in bold): **`graphql-orm` 0.186.0, `graphql-orm-macros` 0.186.0**; `graphql-orm-operation-catalog` 0.9.1; `graphql-orm-storage` 0.18.3; `graphql-orm-backup` 0.27.2; `graphql-orm-ai-tool-profiles` 0.26.1; `graphql-orm-ai` 0.150.2; `graphql-orm-router-protocol` 0.3.1; **`graphql-orm-router` 0.16.8**.

- **No action** — ORM/macros 0.186.0: the owned PostgreSQL test harness waits for TCP readiness before connecting. [Migration](../../MIGRATION.md#01850-to-01860-owned-postgresql-test-readiness)
- **Action needed** — ORM/macros 0.185.0: if you compare generated SQL Server filtered-index DDL, update expected string literals to include the `N` prefix, even for ASCII values. [Migration](../../MIGRATION.md#01830-to-01850-sql-server-filtered-index-literals)
- **No action** — router 0.16.8: the operations runbook records the startup hand-off and ownership rationale, and a few source comments are clearer; no behaviour, configuration or public API changes. [Migration](../../crates/graphql-orm-router/MIGRATION.md#0167-to-0168-operations-documentation)

### workspace-2026.11.19.2

Commit `3e9c34a5fcc9` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.19.2)

Packages (changed in bold): **`graphql-orm` 0.183.0, `graphql-orm-macros` 0.183.0**; **`graphql-orm-operation-catalog` 0.9.1**; **`graphql-orm-storage` 0.18.3**; **`graphql-orm-backup` 0.27.2**; **`graphql-orm-ai-tool-profiles` 0.26.1**; **`graphql-orm-ai` 0.150.2**; **`graphql-orm-router-protocol` 0.3.1**; **`graphql-orm-router` 0.16.7**.

- **No action** — ORM/macros 0.183.0, operation catalog 0.9.1, storage 0.18.3, backup 0.27.2, tool profiles 0.26.1, AI 0.150.2, router protocol 0.3.1 and router 0.16.7: install documentation now points to the workspace package inventory and release manifest, and the dependency directions in the workspace guide match Cargo metadata, checked in CI. [Migration](../../MIGRATION.md#01820-to-01830-install-documentation)
- **Action needed** — ORM/macros 0.182.0: SQL identifier quoting. `quote_identifier`, `render_identifier`, their path forms and the derive macros pass input through unchanged only when it is exactly one well-formed delimited identifier, and quote anything else as one literal name (#782). Act only if code passes an already-quoted qualified path such as `"app"."users"` to a single-identifier function, which should use the `_path` form, declares a `table`/`db_column` part that only looks delimited (or contains NUL), which the derive now refuses, or relies on a path part the old shared splitter read as delimited but the database does not, such as PostgreSQL `[s.t`. Paths are now split with each backend's own delimiters. Introspection reports names that need delimiting (bracketed or dotted SQL Server index and constraint names, PostgreSQL foreign-key targets such as `"Archive"."Parent"`) delimited; act only if you compare introspected names by text. Read predicates over a table-qualified `db_column` now reference its column through aliases, and a qualifier that names another table is refused by the derives at compile time and by read predicates (`AUTHORIZATION_MISCONFIGURED`); on SQL Server the qualifier must match the table's exact case. PostgreSQL now reports every cross-schema foreign-key target with a delimited schema (`"plain_other".target`); act if you compare that text. SQL Server brackets names outside its regular-identifier rule, such as `x²` (#783), and keeps every name it accepted bare byte-identical. [Migration](../../MIGRATION.md#01810-to-01820-sql-identifier-quoting) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01820)

### workspace-2026.11.19.1

Commit `a6308d55cdcb` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.19.1)

Packages (changed in bold): **`graphql-orm` 0.181.0, `graphql-orm-macros` 0.181.0**; `graphql-orm-operation-catalog` 0.9.0; `graphql-orm-storage` 0.18.2; **`graphql-orm-backup` 0.27.1**; **`graphql-orm-ai-tool-profiles` 0.26.0**; **`graphql-orm-ai` 0.150.1**; `graphql-orm-router-protocol` 0.3.0; **`graphql-orm-router` 0.16.6**.

- **No action** — ORM/macros 0.181.0: the transitive-closure read predicates now work on SQL Server, as a correlated breadth-first search with the same semantics as the CTE form (no DDL). `related_closure_within` accepts at most 64 hops on SQL Server (`READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH`); the default bound is 64 everywhere. [Migration](../../MIGRATION.md#01800-to-01810-closure-read-predicates-on-sql-server)
- **Action needed** — ORM/macros 0.181.0: only for read policies over the new composition budget, now refused at build with `AUTHORIZATION_MISCONFIGURED`. The limits per predicate are closures nested more than two deep, over 1 MiB of rendered SQL, or over 256 expression levels. Every backend also limits `CASE` nesting to 10, and relations to 32 nested and 128 in total (9 and 9 on SQL Server), with each closure counting as 16 relations (1 on SQL Server). On SQL Server, the limits are also 75 nested subqueries and 900 weighted levels. Per statement, relation filters and count orderings may add at most 1 MiB and 64 relations (5 on SQL Server), and a generated `orderBy` list may hold 16 terms, so a client cannot multiply a deep policy into an out-of-memory statement. SQL Server plan errors can still fail individual reads and execution time is not bounded: set PostgreSQL `statement_timeout`, and a query timeout and Resource Governor limits on SQL Server. These policies previously rendered oversized statements, overflowed the stack or failed every read. `and`/`or` folds of any length are now flattened and no longer crash rendering. Lists render as balanced pairs, so long generated `or` lists also run on SQLite. [Migration](../../MIGRATION.md#01800-to-01810-closure-read-predicates-on-sql-server)
- **No action** — ORM/macros 0.180.0: new opt-in `mssql_snapshot_bulk_write_targets` (off by default, separate from narrowing) lets narrowed SQL Server bulk writes select targets from snapshots and no longer lock or wait on hidden rows; with it on, the bulk write's transaction is not serializable and a concurrent change to the candidates refuses it with a retryable `SERVICE_UNAVAILABLE`. [Migration](../../MIGRATION.md#01791-to-01800-opt-in-sql-server-snapshot-bulk-write-targets) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01800)
- **No action** — ORM/macros 0.179.1, backup 0.27.1 and AI 0.150.1: test-owned PostgreSQL containers are memory-capped; no runtime change. [Migration](../../MIGRATION.md#01790-to-01791-test-owned-postgresql-memory-cap) · [backup](../../crates/graphql-orm-backup/MIGRATION.md#0270-to-0271) · [AI](../../crates/graphql-orm-ai/MIGRATION.md#01500-to-01501)
- **No action** — AI 0.150.0 and tool profiles 0.26.0 reuse compiled validators. Existing approvals and durable state values remain compatible. [AI migration](../../crates/graphql-orm-ai/MIGRATION.md#01490-to-01500) · [tool profiles migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0250-to-0260)
- **No action** — Router 0.16.6 shares outbound retrieval, reduces redundant WebSocket encoding and scopes the Hive plugin handoff to startup. [Migration](../../crates/graphql-orm-router/MIGRATION.md#0165-to-0166)

### workspace-2026.11.18.2

Commit `3e15e21135bd` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.18.2)

Packages (changed in bold): **`graphql-orm` 0.179.0, `graphql-orm-macros` 0.179.0**; `graphql-orm-operation-catalog` 0.9.0; **`graphql-orm-storage` 0.18.2**; `graphql-orm-backup` 0.27.0; `graphql-orm-ai-tool-profiles` 0.25.0; `graphql-orm-ai` 0.149.0; `graphql-orm-router-protocol` 0.3.0; **`graphql-orm-router` 0.16.5**.

- **Action needed** — ORM/macros 0.179.0: only for a SQLite name declared with a leading U+FEFF, which is now delimited and keeps the mark. [Migration](../../MIGRATION.md#01770-to-01790-identifier-quoting-and-canonical-json-properties)
- **Action needed** — router 0.16.5: only if your issuer writes `iss` as an array or adds a `crit` header member (even `null`); such bearer tokens are now refused by both the JWKS and `auth-agql` providers, as is a protected header over 8 KiB encoded. [Migration](../../crates/graphql-orm-router/MIGRATION.md#0164-to-0165)
- **No action** — storage 0.18.2: key-validation property tests only.
- **No action** — Router 0.16.4 adds a failed authentication refresh counter to the authenticated metrics response. [Migration](../../crates/graphql-orm-router/MIGRATION.md#0162-to-0164)
- **Action needed** — Storage 0.18.1 returns an I/O error if a local listing cannot verify the reserved directory identity. Callers that previously expected to proceed after such an error must handle it. [Migration](../../crates/graphql-orm-storage/MIGRATION.md#0170-to-0181)
- **No action** — ORM/macros 0.177.0: opt-in `DatabaseBuilder::narrow_bulk_writes_to_read_visibility(true)` narrows generated GraphQL `updateMany`/`deleteMany` targets to the row policy's SQL read visibility (rows outside it are not evaluated, counted or, on PostgreSQL, locked); off by default, bulk writes unchanged. Before enabling: write-only rows need a `write_target_prefilter`, and a `read_visibility` error refuses the bulk write. [migration](../../MIGRATION.md#01760-to-01770-bulk-write-target-visibility-and-coded-dependent-delete-refusals)
- **No action** — ORM/macros 0.177.0: a `dependent_deletes` root delete refusal reports its `errorCode` (`NOT_FOUND`) under `mutation_error_codes`; the error-code reference lists `SUBSCRIPTIONS_NOT_SUPPORTED` and `MUTATION_NOT_ALLOWED_OVER_HTTP_GET` and states that a bulk `errorCode` can be a host refusal's code. [migration](../../MIGRATION.md#01760-to-01770-bulk-write-target-visibility-and-coded-dependent-delete-refusals) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01770)
- **No action** — Router 0.16.2: wire-test coverage only (a subgraph's `extensions.outcome` passes through); behavior unchanged. [migration](../../crates/graphql-orm-router/MIGRATION.md#0161-to-0162)
- **No action** — Router 0.16.1: Only the wire tests change; router behavior, configuration and public APIs are unchanged. [migration](../../crates/graphql-orm-router/MIGRATION.md#0160-to-0161) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0161-unreleased)

### workspace-2026.11.18.1

Commit `999e3c02abed` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.18.1)

Packages (changed in bold): **`graphql-orm` 0.176.0, `graphql-orm-macros` 0.176.0**; `graphql-orm-operation-catalog` 0.9.0; `graphql-orm-storage` 0.17.0; `graphql-orm-backup` 0.27.0; `graphql-orm-ai-tool-profiles` 0.25.0; **`graphql-orm-ai` 0.149.0**; `graphql-orm-router-protocol` 0.3.0; `graphql-orm-router` 0.16.0.

- **No action** — ORM/macros 0.176.0: transitive read-policy closures. `ReadPredicate::related_closure` ("some reachable row matches P"), `related_closure_all` ("no reachable row fails P") and `related_closure_within` (an explicit hop bound, at most 1,024; the default is 64) render a recursive CTE on SQLite and PostgreSQL and fail closed on UNKNOWN values, unreachable or ambiguous steps and the depth bound. SQL Server refuses them with `AUTHORIZATION_MISCONFIGURED`. [Migration](../../MIGRATION.md#01750-to-01760-transitive-read-policy-closures)
- **No action** — AI 0.149.0: OpenAI Responses (also xAI and OpenAI-compatible) `response.incomplete` output-limit stops and Ollama `length` stops complete with exact usage and a visible notice, and a cut-off tool request is never run; the conversation bootstrap reads its whole tool-call window, so `reset_required` no longer fires just because a conversation holds more tool calls than the page maximum. No schema change. [Migration](../../crates/graphql-orm-ai/MIGRATION.md#01480-to-01490-output-limit-stops-and-the-bootstrap-tool-call-window) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01490)

### workspace-2026.11.17.4

Commit `917077ed1a4e` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.17.4)

Packages (changed in bold): **`graphql-orm` 0.174.0, `graphql-orm-macros` 0.174.0**; **`graphql-orm-operation-catalog` 0.9.0**; **`graphql-orm-storage` 0.17.0**; **`graphql-orm-backup` 0.27.0**; **`graphql-orm-ai-tool-profiles` 0.25.0**; **`graphql-orm-ai` 0.148.0**; **`graphql-orm-router-protocol` 0.3.0**; **`graphql-orm-router` 0.16.0**.

- **No action** — ORM/macros 0.174.0: read-policy negation. `ReadPredicate::not_related` is the fail-closed "no related row matching P" (NULL keys and UNKNOWN related rows are not admitted); `ReadPredicate::is_null` and `is_not_null` state NULL handling explicitly. `not()` keeps SQL three-valued logic. [Migration](../../MIGRATION.md#01730-to-01740-read-policy-negation)
- **Action needed** — ORM/macros 0.174.0: only for policies that call `.not()` on a predicate that can render the deny fallback `1 = 0`: an empty `in_list` or `or` list, a list over 1,000 operands, or an invalid filter value. Previously, negating such a predicate admitted all rows; it is now refused with `AUTHORIZATION_MISCONFIGURED`. Omit the term when the list is empty, use `is_not_null`, or keep the list within the operand limit. [Migration](../../MIGRATION.md#01730-to-01740-read-policy-negation)
- **Action needed** — ORM/macros 0.173.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../MIGRATION.md#01720-to-01730-process-and-toolchain)
- **Action needed** — ORM/macros 0.173.0 release operations: scoped publication and router distribution require a same-repository merged PR or approved-review URL. [migration](../../MIGRATION.md#01720-to-01730-process-and-toolchain)
- **Action needed** — Operation catalogue 0.9.0: build with Rust 1.97.1 or newer; serialized contracts are unchanged. [changelog](../../CHANGELOG.md#unreleased---ormmacros-01730)
- **Action needed** — Storage 0.17.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../crates/graphql-orm-storage/MIGRATION.md#0170)
- **Action needed** — Backup 0.27.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../crates/graphql-orm-backup/MIGRATION.md#0270)
- **Action needed** — AI tool profiles 0.25.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0250)
- **Action needed** — AI 0.148.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01480)
- **Action needed** — Router protocol 0.3.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../crates/graphql-orm-router-protocol/MIGRATION.md#030)
- **Action needed** — Router 0.16.0: build with Rust 1.97.1 or newer; no data migration. [migration](../../crates/graphql-orm-router/MIGRATION.md#0160)

### workspace-2026.11.17.3

Commit `97e5e764bd7c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.17.3)

Packages (changed in bold): **`graphql-orm` 0.172.0, `graphql-orm-macros` 0.172.0**; **`graphql-orm-operation-catalog` 0.8.1**; `graphql-orm-storage` 0.16.0; `graphql-orm-backup` 0.26.0; `graphql-orm-ai-tool-profiles` 0.24.0; `graphql-orm-ai` 0.147.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.5.

- **No action** — ORM/macros 0.172.0, operation catalogue 0.8.1: opt-in client-facing metadata. `schema_roots!(client_metadata: true)` emits a build-time client metadata export (`Changed` carried fields, `lifetime(...)` expiry fields, relationship key mappings); every `schema_roots!` emits `graphql_orm_interpretation_fingerprint(salt)`; the pagination reference states that pages are independent reads (no snapshot token). Without the options, SDL, catalogues and fingerprints are unchanged. [Migration](../../MIGRATION.md#ormmacros-01720-and-operation-catalogue-081-client-facing-metadata)
- **Action needed** — ORM/macros 0.172.0: only for clients of an entity you opt in with `changed_started = true` (its `Changed` event's `Action`, `ChangeKind`, `Id` and `Key` become nullable and it gains `Started`) or `aggregate_source = true` (its aggregate root returns `[<Entity>AggregateResultRow!]!`): regenerate typed client code and any approvals that name those types; and only if your `schema_roots!` scope already defines `graphql_orm_interpretation_fingerprint`: rename it. [Migration](../../MIGRATION.md#ormmacros-01720-and-operation-catalogue-081-client-facing-metadata)
- **No action** — ORM/macros 0.171.0: opt-in client error contract: `SUBSCRIPTION_SELECTION_UNSUPPORTED` for an unstreamable `Changed` selection (`DatabaseBuilder::subscription_selection_unsupported_code`), `extensions.outcome` on generated mutation errors (`DatabaseBuilder::mutation_error_outcome`), and `errorCode` on single-row results of entities declared with `mutation_error_codes`. Nothing changes without them. [Migration](../../MIGRATION.md#01700-to-01710-opt-in-client-error-contract) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01710)

### workspace-2026.11.17.2

Commit `b3549ce012d8` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.17.2)

Packages (changed in bold): **`graphql-orm` 0.170.0, `graphql-orm-macros` 0.170.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.16.0; `graphql-orm-backup` 0.26.0; `graphql-orm-ai-tool-profiles` 0.24.0; **`graphql-orm-ai` 0.147.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.5.

- **No action** — ORM/macros 0.170.0: generated `schema_builder` helpers refuse a scalar, list or enum sent for an input object, and a variable whose declared type does not fit its position, with `invalid_graphql_input` before any resolver runs. Field arguments are checked; directive arguments are left to async-graphql. Bulk-write filter authority is unchanged and now tested on SQLite and PostgreSQL. [Migration](../../MIGRATION.md#01690-to-01700-input-object-values-on-static-schemas-and-bulk-write-filter-authority)
- **Action needed** — ORM/macros 0.170.0: only for hosts that build a schema from generated roots by hand: add `.extension(graphql_orm::graphql::input_validation::GeneratedInputValidation)`; without it the schema does not get the check. [Migration](../../MIGRATION.md#01690-to-01700-input-object-values-on-static-schemas-and-bulk-write-filter-authority)
- **Action needed** — ORM/macros 0.170.0: only for client documents that place a variable where the GraphQL rule "All Variable Usages Are Allowed" does not admit its declared type (for example a nullable variable without a default at a required argument, or a nullable or null variable in a oneof field). Generated static and runtime GraphQL schemas now refuse such a request with `invalid_graphql_input` instead of running it when its value happened to coerce; declare the variable with the position's type. [Migration](../../MIGRATION.md#01690-to-01700-input-object-values-on-static-schemas-and-bulk-write-filter-authority)
- **Action needed** — AI 0.147.0: apply AI schema module `0.82.0`. Lease recovery now waits a clock-skew grace past expiry (a quarter of the lease TTL, at most 30 s); set `with_lease_recovery_grace(Duration::ZERO)` only if you rely on immediate recovery. Hosts with `store_responses = true` install `OpenAiStoredResponseDeletionService` (bound with `with_provider_profile_id`) and schedule `delete_stored_provider_responses` to delete stored responses after session deletion; a custom `AiAgentRunControl` should delegate `record_retained_provider_response` to `OrmAiRunService`. [Migration](../../crates/graphql-orm-ai/MIGRATION.md#01460-to-01470-lease-grace-provider-stops-and-stored-response-deletion) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01470)
- **No action** — AI 0.147.0: Anthropic `max_tokens`, `model_context_window_exceeded` and `pause_turn` and chat-completions `length` complete metered with a visible notice instead of failing after the spend; dated-snapshot model echoes are accepted unless an Anthropic pin is declared, which stays exact; a provider-file 404 never proves absence. [Migration](../../crates/graphql-orm-ai/MIGRATION.md#01460-to-01470-lease-grace-provider-stops-and-stored-response-deletion)

### workspace-2026.11.17.1

Commit `b6c41d21d3bc` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.17.1)

Packages (changed in bold): **`graphql-orm` 0.169.0, `graphql-orm-macros` 0.169.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.16.0; `graphql-orm-backup` 0.26.0; `graphql-orm-ai-tool-profiles` 0.24.0; `graphql-orm-ai` 0.146.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.15.5**.

- **No action** — ORM/macros 0.169.0: opt-in client page offset bound on every backend (`DatabaseBuilder::max_page_offset`), refused with `PAGE_LIMIT_EXCEEDED`; offsets stay unbounded by default. A SQL Server `DECIMAL(38,38)` value is a decode error instead of a driver panic, and rendered `ALTER COLUMN` keeps a column's collation. [Migration](../../MIGRATION.md#01670-to-01690-sql-server-correctness-and-a-client-offset-bound) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01690)
- **Action needed** — ORM/macros 0.169.0: only if you read `introspect_mssql_schema`, compare SQL Server `ColumnDef::sql_type`, or hand-made the restore marker table: introspection now returns indexes and no invented `[id]` key, SQL Server key `String` columns are `NVARCHAR(450)`, and `install_restore_marker` checks the marker's primary key and `CHECK` constraint. [Migration](../../MIGRATION.md#01670-to-01690-sql-server-correctness-and-a-client-offset-bound)
- **No action** — Router 0.15.5: Only the wire tests change; router behavior, configuration and public APIs are unchanged. [migration](../../crates/graphql-orm-router/MIGRATION.md#0154-to-0155) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0155-unreleased)
- **No action** — ORM/macros 0.167.0: test-only; the macros unit tests no longer assume the SQLite backend [migration](../../MIGRATION.md#ormmacros-01670-test-only-backend-agnostic-macros-unit-tests)

### workspace-2026.11.16.4

Commit `9674f5c71677` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.16.4)

Packages (changed in bold): **`graphql-orm` 0.166.0, `graphql-orm-macros` 0.166.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.16.0; `graphql-orm-backup` 0.26.0; **`graphql-orm-ai-tool-profiles` 0.24.0**; **`graphql-orm-ai` 0.146.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.4.

- **No action** — ORM/macros 0.166.0: opt-in dependent-delete effect verification and transaction-bound row limits; unsupported plans refuse [migration](../../MIGRATION.md#ormmacros-01660-guarded-dependent-deletes)
- **No action** — AI 0.146.0: delivery selection stays unchanged unless the host opts into `prefer_fixed_broker_for_large_surfaces`; opting in requires a retained-session rebind [migration](../../crates/graphql-orm-ai/MIGRATION.md#01450-to-01460-capability-scale) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01460)
- **No action** — AI 0.146.0: fixed-broker `describe` returns each repeated input type once under `planSchema.types`; execute arguments and fingerprints are unchanged [migration](../../crates/graphql-orm-ai/MIGRATION.md#01450-to-01460-capability-scale) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01460)
- **No action** — AI 0.146.0: opt-in reviewed static read tools behind the fixed broker, host-held epoch anchor for file-level restores, and dependent-effect policy for generated deletes [migration](../../crates/graphql-orm-ai/MIGRATION.md#01450-to-01460-capability-scale) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01460)
- **No action** — Tool profiles 0.24.0: one oversized generated root is excluded and reported instead of failing its target; opt-in strict query roots; shared type dictionary rendering; precomputed search terms [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0230-to-0240-capability-scale) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#unreleased---0240)

### workspace-2026.11.16.3

Commit `cd8072d8e576` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.16.3)

Packages (changed in bold): **`graphql-orm` 0.165.0, `graphql-orm-macros` 0.165.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.16.0; **`graphql-orm-backup` 0.26.0**; `graphql-orm-ai-tool-profiles` 0.23.0; `graphql-orm-ai` 0.145.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.4.

- **Action needed** — ORM/macros 0.165.0: MSSQL hosts must install the canonical restore marker table (`MssqlPool::install_restore_marker()` on an external-writable pool) and grant the runtime role SELECT on it; until then every statement is refused with `RestoreStateUnavailable`, which names the missing install or grant (it is not `RestoreInProgress`). Hosts restoring runtime search must backfill after restore; ignored field-write denials now preserve stored values. [Migration](../../MIGRATION.md#01640-to-01650-atomic-restore-search-reset-and-kept-values-on-ignored-denials) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01650-backup-0260)
- **No action** — ORM/macros 0.165.0: SQLite and PostgreSQL snapshot restores now commit atomically. [Migration](../../MIGRATION.md#01640-to-01650-atomic-restore-search-reset-and-kept-values-on-ignored-denials)
- **Action needed** — Backup 0.26.0: opt in to host-held snapshot records and record snapshot ids before deletion to prevent replay of pruned snapshots. Existing APIs remain source compatible (**No action** to keep compiling). [Migration](../../crates/graphql-orm-backup/MIGRATION.md#to-0260-host-held-snapshot-records) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md)

### workspace-2026.11.16.2

Commit `a59874e8973f` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.16.2)

Packages (changed in bold): **`graphql-orm` 0.164.0, `graphql-orm-macros` 0.164.0**; `graphql-orm-operation-catalog` 0.8.0; **`graphql-orm-storage` 0.16.0**; **`graphql-orm-backup` 0.25.0**; `graphql-orm-ai-tool-profiles` 0.23.0; `graphql-orm-ai` 0.145.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.4.

- **No action** — ORM/macros 0.164.0: opt-in page-limit refusal and declared maxima with schema-local complexity bounds; existing `PaginationConfig` literals remain valid. See [migration details](../../MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).
- **No action** — ORM/macros 0.164.0: per-read keyset and authorized-scan maxima through `EntityQuery::with_max_limit`. See [migration details](../../MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).
- **No action** — ORM/macros 0.164.0: opt-in runtime 64-bit identity strings. See [migration details](../../MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).
- **Action needed** — ORM/macros 0.164.0: only if you match the codes for a cursor key that is too long: runtime GraphQL returns `cursor_key_too_large` instead of `invalid_request` or `invalid_cursor`, and host errors return `ResourceLimit` with `detail() == Some("cursor_key_too_large")` instead of `InvalidRequest`; error enums stay source-compatible. See [migration details](../../MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).
- **Action needed** — ORM/macros 0.164.0: clients expecting immediate subscription lag termination must allow termination at the next authority tick. See [migration details](../../MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).
- **Action needed** — Storage 0.16.0 / Backup 0.25.0: Windows hosts only. `LocalStorageBackend::new`, `LocalBackupRepository::new` and `LocalBackupRepository::open_existing` no longer exist on Windows; call `LocalStorageBackend::new_trusted_windows_root`, `LocalBackupRepository::new_trusted_windows_root` or `LocalBackupRepository::open_existing_trusted_windows_root` only for a host-protected tree (links are not enforced as a security boundary in that mode). On Unix, a failed fallback-publication rollback is a non-retryable, outcome-unknown error [migration](../../crates/graphql-orm-storage/MIGRATION.md#0150-to-0160-local-links-listing-pages-and-canonical-keys) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0160-unreleased)
- **No action** — ORM/macros 0.163.0: hand-emitted wrapper objects reduce compile cost; generated SDL, catalogues and fingerprints are unchanged [migration](../../MIGRATION.md#01590-to-01630-wrapper-object-compile-cost) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01630)
- **Action needed** — Storage 0.16.0: Unix local storage refuses filesystem links and hard-linked files below its root, resolving every operation relative to held directory handles; only for roots that contain links or hard links [migration](../../crates/graphql-orm-storage/MIGRATION.md#0150-to-0160-local-links-listing-pages-and-canonical-keys) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0160-unreleased)
- **Action needed** — Storage 0.16.0: a local listing page can hold fewer keys than its limit while it returns a continuation; only for code that calls `list_blobs_page` directly and stops at a short page [migration](../../crates/graphql-orm-storage/MIGRATION.md#0150-to-0160-local-links-listing-pages-and-canonical-keys) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0160-unreleased)
- **No action** — Storage 0.16.0: opt-in canonical keys for SMB and local targets (`validate_canonical_blob_key`, `with_canonical_keys`); `content_type` is ignored on local and SMB, now documented [migration](../../crates/graphql-orm-storage/MIGRATION.md#0150-to-0160-local-links-listing-pages-and-canonical-keys) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0160-unreleased)
- **Action needed** — Backup 0.25.0: storage 0.16.0; only for local repositories whose root contains links or hard links [migration](../../crates/graphql-orm-backup/MIGRATION.md#to-0250-storage-0160) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0250-unreleased)

### workspace-2026.11.16.1

Commit `1a4706bd119a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.16.1)

Packages (changed in bold): **`graphql-orm` 0.162.0, `graphql-orm-macros` 0.162.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.15.0; `graphql-orm-backup` 0.23.0; `graphql-orm-ai-tool-profiles` 0.23.0; `graphql-orm-ai` 0.145.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.4.

- **No action** — ORM/macros 0.162.0: context-bound authorized pages for handwritten resolvers (`EntityQuery::fetch_authorized_page`); additive [migration](../../MIGRATION.md#01620-context-bound-authorized-pages) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01620)

### workspace-2026.11.15.6

Commit `5cf776710c8e` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.15.6)

Packages (changed in bold): **`graphql-orm` 0.159.0, `graphql-orm-macros` 0.159.0**; `graphql-orm-operation-catalog` 0.8.0; **`graphql-orm-storage` 0.15.0**; **`graphql-orm-backup` 0.23.0**; `graphql-orm-ai-tool-profiles` 0.23.0; `graphql-orm-ai` 0.145.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.15.4**.

- **Action needed** — ORM/macros 0.159.0: PostgreSQL table-name case [migration](../../MIGRATION.md#01580-to-01590-postgresql-table-name-case) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01590)
- **No action** — Storage 0.15.0: bounded SMB destructor cleanup [migration](../../crates/graphql-orm-storage/MIGRATION.md#0140-to-0150-bounded-smb-destructor-cleanup) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0150-unreleased)
- **No action** — Backup 0.23.0: PostgreSQL mixed-case table restore [migration](../../crates/graphql-orm-backup/MIGRATION.md#to-0230-postgresql-mixed-case-table-restore) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0230-unreleased)
- **No action** — Router 0.15.4: Only the wire tests change; router behavior, configuration and public APIs are unchanged. [migration](../../crates/graphql-orm-router/MIGRATION.md#0153-to-0154) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0154-unreleased)

### workspace-2026.11.15.5

Commit `c3d111af7713` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.15.5)

Packages (changed in bold): **`graphql-orm` 0.158.0, `graphql-orm-macros` 0.158.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.14.0; `graphql-orm-backup` 0.22.0; `graphql-orm-ai-tool-profiles` 0.23.0; **`graphql-orm-ai` 0.145.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.3.

- **Action needed** — ORM/macros 0.158.0: runtime budget, search helper names and typed refusals [migration](../../MIGRATION.md#01570-to-01580-runtime-budget-search-helper-names-and-typed-refusals) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01580)
- **Action needed** — AI 0.145.0: entity references [migration](../../crates/graphql-orm-ai/MIGRATION.md#01440-to-01450-entity-references) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01450)

### workspace-2026.11.15.4

Commit `ada47f002d37` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.15.4)

Packages (changed in bold): `graphql-orm` 0.157.0, `graphql-orm-macros` 0.157.0; `graphql-orm-operation-catalog` 0.8.0; **`graphql-orm-storage` 0.14.0**; **`graphql-orm-backup` 0.22.0**; `graphql-orm-ai-tool-profiles` 0.23.0; **`graphql-orm-ai` 0.144.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.15.3**.

- **No action** — Storage 0.14.0: cancel-safe SMB handle close [migration](../../crates/graphql-orm-storage/MIGRATION.md#0130-to-0140-cancel-safe-smb-handle-close) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0140-unreleased)
- **No action** — Backup 0.22.0: storage 0.14.0 [migration](../../crates/graphql-orm-backup/MIGRATION.md#to-0220-storage-0140) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0220-unreleased)
- **Action needed** — AI 0.144.0: AI follow-ups [migration](../../crates/graphql-orm-ai/MIGRATION.md#01430-to-01440-ai-follow-ups) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01440)
- **No action** — Router 0.15.3: Node lookups no longer hold a plugin context read lock while they execute. [migration](../../crates/graphql-orm-router/MIGRATION.md#0152-to-0153) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0153-unreleased)

### workspace-2026.11.15.3

Commit `c19507c2c37b` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.15.3)

Packages (changed in bold): **`graphql-orm` 0.157.0, `graphql-orm-macros` 0.157.0**; `graphql-orm-operation-catalog` 0.8.0; `graphql-orm-storage` 0.13.0; `graphql-orm-backup` 0.21.0; `graphql-orm-ai-tool-profiles` 0.23.0; `graphql-orm-ai` 0.143.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.2.

- **Action needed** — ORM/macros 0.157.0: migrations, change journal and retention [migration](../../MIGRATION.md#01560-to-01570-migrations-change-journal-and-retention) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01570)

### workspace-2026.11.15.2

Commit `122c48812582` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.15.2)

Packages (changed in bold): **`graphql-orm` 0.156.0, `graphql-orm-macros` 0.156.0**; `graphql-orm-operation-catalog` 0.8.0; **`graphql-orm-storage` 0.13.0**; **`graphql-orm-backup` 0.21.0**; `graphql-orm-ai-tool-profiles` 0.23.0; **`graphql-orm-ai` 0.143.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.15.2**.

- **Action needed** — ORM/macros 0.156.0: queries, filters and relation loading [migration](../../MIGRATION.md#01550-to-01560-queries-filters-and-relation-loading) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01560)
- **Action needed** — ORM/macros 0.155.0: runtime GraphQL guard and row-level security [migration](../../MIGRATION.md#01520-to-01550-runtime-graphql-guard-and-row-level-security) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — ORM/macros 0.152.0: batched row-security probe [migration](../../MIGRATION.md#01510-to-01520-batched-row-security-probe) · [changelog](../../CHANGELOG.md#unreleased---ormmacros-01520-ai-01430)
- **Action needed** — ORM/macros 0.151.0: SQL Server correctness [migration](../../MIGRATION.md#01490-to-01510-sql-server-correctness) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — Storage 0.13.0: SMB sweep, error retryability and messages [migration](../../crates/graphql-orm-storage/MIGRATION.md#0120-to-0130-smb-sweep-error-retryability-and-messages) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0130-unreleased)
- **Action needed** — Storage 0.12.0: S3 conditional creates, key errors, SMB configuration [migration](../../crates/graphql-orm-storage/MIGRATION.md#0110-to-0120-s3-conditional-creates-key-errors-smb-configuration) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0120-unreleased)
- **Action needed** — Backup 0.21.0: storage 0.13.0 [migration](../../crates/graphql-orm-backup/MIGRATION.md#to-0210-storage-0130) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0210-unreleased)
- **Action needed** — Backup 0.19.0: storage 0.12.0 [migration](../../crates/graphql-orm-backup/MIGRATION.md#0180-to-0190-storage-0120) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0190-unreleased)
- **Action needed** — AI 0.143.0: runtime and provider bug batch [migration](../../crates/graphql-orm-ai/MIGRATION.md#01420-to-01430-runtime-and-provider-bug-batch) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01430)
- **No action** — Router 0.15.2: Established WebSocket operations can resume following role-authority refresh without an intermittent policy-context lock stall. [migration](../../crates/graphql-orm-router/MIGRATION.md#0151-to-0152) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0152-unreleased)
- **No action** — Router 0.15.1: Test fixture stability only; no consumer migration is required. [migration](../../crates/graphql-orm-router/MIGRATION.md#0150-to-0151) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0151-unreleased)

### workspace-2026.11.14.2

Commit `ea52751aa823` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.14.2)

Packages (changed in bold): **`graphql-orm` 0.149.0, `graphql-orm-macros` 0.149.0**; **`graphql-orm-operation-catalog` 0.8.0**; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.18.0; **`graphql-orm-ai-tool-profiles` 0.23.0**; **`graphql-orm-ai` 0.142.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.15.0.

- **Action needed** — ORM/macros 0.149.0: host-only generated input contracts [migration](../../MIGRATION.md#01470-to-01490-host-only-generated-input-contracts) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — Operation catalogue 0.8.0: generated catalogue trust boundary [migration](../../MIGRATION.md#operation-catalogue-080-breaking-generated-catalogue-trust-boundary) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — AI tool profiles 0.23.0: explicit handwritten input disclosure [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0220-to-0230-explicit-handwritten-input-disclosure) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#unreleased---0230)
- **Action needed** — AI 0.142.0: exact tool registration and derived control-plane roots [migration](../../crates/graphql-orm-ai/MIGRATION.md#01400-to-01420-exact-tool-registration-and-derived-control-plane-roots) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01420)

### workspace-2026.11.14.1

Commit `ee2811173f76` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.14.1)

Packages (changed in bold): **`graphql-orm` 0.147.0, `graphql-orm-macros` 0.147.0**; `graphql-orm-operation-catalog` 0.7.4; `graphql-orm-storage` 0.11.0; **`graphql-orm-backup` 0.18.0**; `graphql-orm-ai-tool-profiles` 0.22.0; **`graphql-orm-ai` 0.140.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.15.0**.

- **Action needed** — ORM/macros 0.147.0: write paths, hooks and authority [migration](../../MIGRATION.md#01440-to-01470-write-paths-hooks-and-authority) · [changelog](../../CHANGELOG.md#unreleased)
- **No action** — ORM/macros 0.144.0: Test infrastructure only: no API or data migration required. [migration](../../MIGRATION.md#graphql-orm-01440) · [changelog](../../CHANGELOG.md#unreleased)
- **No action** — ORM/macros 0.144.0: Test infrastructure only: no API or data migration required. [migration](../../MIGRATION.md#graphql-orm-macros-01440) · [changelog](../../CHANGELOG.md#unreleased)
- **No action** — Backup 0.18.0: Test infrastructure only: no API or data migration required. [migration](../../crates/graphql-orm-backup/MIGRATION.md#graphql-orm-backup-0180) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0180-unreleased)
- **No action** — AI 0.140.0: Test infrastructure only: no API or data migration required. [migration](../../crates/graphql-orm-ai/MIGRATION.md#graphql-orm-ai-01400)
- **Action needed** — Router 0.15.0: router hardening [migration](../../crates/graphql-orm-router/MIGRATION.md#0140-to-0150-router-hardening) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0150-unreleased)

### workspace-2026.11.13.13

Commit `c4508478b355` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.13)

Packages (changed in bold): **`graphql-orm` 0.143.0, `graphql-orm-macros` 0.143.0**; `graphql-orm-operation-catalog` 0.7.4; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.17.0; `graphql-orm-ai-tool-profiles` 0.22.0; **`graphql-orm-ai` 0.139.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.14.0.

- **Action needed** — ORM/macros 0.143.0: one authorization contract [migration](../../MIGRATION.md#01410-to-01430-one-authorization-contract) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — AI 0.139.0: custody of the crate's own records under a host field policy [migration](../../crates/graphql-orm-ai/MIGRATION.md#01380-to-01390-custody-of-the-crates-own-records-under-a-host-field-policy) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01390)

### workspace-2026.11.13.12

Commit `0c417846063a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.12)

Packages (changed in bold): `graphql-orm` 0.141.0, `graphql-orm-macros` 0.141.0; `graphql-orm-operation-catalog` 0.7.4; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.17.0; `graphql-orm-ai-tool-profiles` 0.22.0; **`graphql-orm-ai` 0.138.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.14.0.

- **Action needed** — AI 0.138.0: Apply `AiSchemaModule` version `0.78.0` with the normal module migration. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01370-to-01380) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01380)

### workspace-2026.11.13.11

Commit `a5211243ce7c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.11)

Packages (changed in bold): **`graphql-orm` 0.141.0, `graphql-orm-macros` 0.141.0**; `graphql-orm-operation-catalog` 0.7.4; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.17.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.136.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.14.0.

- **Action needed** — ORM/macros 0.141.0: write transforms fail closed [migration](../../MIGRATION.md#01390-to-01410-write-transforms-fail-closed) · [changelog](../../CHANGELOG.md#unreleased)

### workspace-2026.11.13.10

Commit `aab573d4fcf3` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.10)

Packages (changed in bold): **`graphql-orm` 0.139.0, `graphql-orm-macros` 0.139.0**; **`graphql-orm-operation-catalog` 0.7.4**; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.17.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.136.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.14.0.

- **Action needed** — ORM/macros 0.139.0: optional stable entity identity [migration](../../MIGRATION.md#01370-to-01390-optional-stable-entity-identity) · [changelog](../../CHANGELOG.md#unreleased)
- **No action** — Operation catalogue 0.7.4: ORM and macros 0.139.0 , catalogue 0.7.4: add optional `stable_type_id` on entity declarations to preserve the entity identity input across Rust module or crate moves … [changelog](../../CHANGELOG.md#unreleased)

### workspace-2026.11.13.9

Commit `19e42ca447f5` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.9)

Packages (changed in bold): **`graphql-orm` 0.137.0, `graphql-orm-macros` 0.137.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.17.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.136.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.14.0.

- **No action** — ORM/macros 0.137.0: smaller generated update code [migration](../../MIGRATION.md#01360-to-01370-smaller-generated-update-code) · [changelog](../../CHANGELOG.md#unreleased)

### workspace-2026.11.13.8

Commit `0c53b3846d39` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.8)

Packages (changed in bold): **`graphql-orm` 0.136.0, `graphql-orm-macros` 0.136.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; **`graphql-orm-backup` 0.17.0**; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.136.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.14.0.

- **Action needed** — ORM/macros 0.136.0: administrative backup and search reads [migration](../../MIGRATION.md#01330-to-01360-administrative-backup-and-search-reads) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — Backup 0.17.0: snapshot consistency and repository identity [migration](../../crates/graphql-orm-backup/MIGRATION.md#0160-to-0170-snapshot-consistency-and-repository-identity) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0170-unreleased)

### workspace-2026.11.13.7

Commit `cc95df949030` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.7)

Packages (changed in bold): `graphql-orm` 0.133.0, `graphql-orm-macros` 0.133.0; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.16.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.136.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.14.0**.

- **Action needed** — Router 0.14.0: persisted documents [migration](../../crates/graphql-orm-router/MIGRATION.md#0130-to-0140-persisted-documents) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0140-unreleased)

### workspace-2026.11.13.6

Commit `add584ea9802` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.6)

Packages (changed in bold): **`graphql-orm` 0.133.0, `graphql-orm-macros` 0.133.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.16.0; `graphql-orm-ai-tool-profiles` 0.22.0; **`graphql-orm-ai` 0.136.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — ORM/macros 0.133.0: reserved runtime table names [migration](../../MIGRATION.md#01320-to-01330-reserved-runtime-table-names) · [changelog](../../CHANGELOG.md#unreleased)
- **Action needed** — ORM/macros 0.133.0: repository limits and offsets count visible rows [migration](../../MIGRATION.md#01320-to-01330-repository-limits-and-offsets-count-visible-rows) · [changelog](../../CHANGELOG.md#unreleased)
- **No action** — AI 0.136.0: ORM 0.133.0 reserved runtime table names [migration](../../crates/graphql-orm-ai/MIGRATION.md#01350-to-01360-orm-01330-reserved-runtime-table-names) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01360)

### workspace-2026.11.13.5

Commit `36ff8e4c01be` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.5)

Packages (changed in bold): **`graphql-orm` 0.132.0, `graphql-orm-macros` 0.132.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.16.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.135.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — ORM/macros 0.132.0: PostgreSQL catalog reads [migration](../../MIGRATION.md#01310-to-01320-postgresql-catalog-reads) · [changelog](../../CHANGELOG.md#01320)

### workspace-2026.11.13.4

Commit `3c06a16c3c36` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.4)

Packages (changed in bold): **`graphql-orm` 0.131.0, `graphql-orm-macros` 0.131.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.16.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.135.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — ORM/macros 0.131.0: native search column collisions [migration](../../MIGRATION.md#01300-to-01310-native-search-column-collisions) · [changelog](../../CHANGELOG.md#01310)

### workspace-2026.11.13.3

Commit `f479a9410a3f` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.3)

Packages (changed in bold): **`graphql-orm` 0.130.0, `graphql-orm-macros` 0.130.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.11.0; `graphql-orm-backup` 0.16.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.135.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — ORM/macros 0.130.0: fail-closed authority defaults [migration](../../MIGRATION.md#01290-to-01300-fail-closed-authority-defaults) · [changelog](../../CHANGELOG.md#01300)

### workspace-2026.11.13.2

Commit `de242a4b1ab0` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.2)

Packages (changed in bold): `graphql-orm` 0.129.0, `graphql-orm-macros` 0.129.0; `graphql-orm-operation-catalog` 0.7.3; **`graphql-orm-storage` 0.11.0**; **`graphql-orm-backup` 0.16.0**; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.135.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — Storage 0.11.0: SMB signing, conditional-create outcomes, S3 messages [migration](../../crates/graphql-orm-storage/MIGRATION.md#0100-to-0110-smb-signing-conditional-create-outcomes-s3-messages) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0110-unreleased)
- **Action needed** — Backup 0.16.0: storage 0.11.0 [migration](../../crates/graphql-orm-backup/MIGRATION.md#0150-to-0160-storage-0110) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0160-unreleased)

### workspace-2026.11.13.1

Commit `7da7ebf68260` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.13.1)

Packages (changed in bold): `graphql-orm` 0.129.0, `graphql-orm-macros` 0.129.0; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.10.0; `graphql-orm-backup` 0.15.0; `graphql-orm-ai-tool-profiles` 0.22.0; **`graphql-orm-ai` 0.135.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — AI 0.135.0: An upload body longer than its ticket's expected byte count now fails as soon as it passes that count, instead of after it has been stored. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01340-to-01350) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01350)

### workspace-2026.11.12.7

Commit `54026ef442cc` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.7)

Packages (changed in bold): **`graphql-orm` 0.129.0, `graphql-orm-macros` 0.129.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.10.0; `graphql-orm-backup` 0.15.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.134.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.13.0.

- **Action needed** — ORM/macros 0.129.0: field authority for filters, ordering, search and relation targets [migration](../../MIGRATION.md#01280-to-01290-field-authority-for-filters-ordering-search-and-relation-targets) · [changelog](../../CHANGELOG.md#01290)

### workspace-2026.11.12.6

Commit `98468e81a991` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.6)

Packages (changed in bold): `graphql-orm` 0.128.0, `graphql-orm-macros` 0.128.0; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.10.0; `graphql-orm-backup` 0.15.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.134.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.13.0**.

- **Action needed** — Router 0.13.0: outbound transport and admin authentication [migration](../../crates/graphql-orm-router/MIGRATION.md#0120-to-0130-outbound-transport-and-admin-authentication) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0130-unreleased)

### workspace-2026.11.12.5

Commit `20acaf08b8ee` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.5)

Packages (changed in bold): **`graphql-orm` 0.128.0, `graphql-orm-macros` 0.128.0**; `graphql-orm-operation-catalog` 0.7.3; `graphql-orm-storage` 0.10.0; `graphql-orm-backup` 0.15.0; `graphql-orm-ai-tool-profiles` 0.22.0; **`graphql-orm-ai` 0.134.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.12.0.

- **Action needed** — ORM/macros 0.128.0: repository limits are exact or refused [migration](../../MIGRATION.md#01270-to-01280-repository-limits-are-exact-or-refused) · [changelog](../../CHANGELOG.md#01280)
- **Action needed** — ORM/macros 0.128.0: SQL Server encryption and pool defaults [migration](../../MIGRATION.md#01270-to-01280-sql-server-encryption-and-pool-defaults) · [changelog](../../CHANGELOG.md#01280)
- **No action** — AI 0.134.0: internal reads honour their own bounds [migration](../../crates/graphql-orm-ai/MIGRATION.md#01330-to-01340-internal-reads-honour-their-own-bounds) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01340)

### workspace-2026.11.12.4

Commit `0d9d968b983f` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.4)

Packages (changed in bold): `graphql-orm` 0.127.0, `graphql-orm-macros` 0.127.0; `graphql-orm-operation-catalog` 0.7.3; **`graphql-orm-storage` 0.10.0**; **`graphql-orm-backup` 0.15.0**; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.133.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.12.0.

- **Action needed** — Storage 0.10.0: SMB 3.1.1 minimum, reserved namespace, string prefixes [migration](../../crates/graphql-orm-storage/MIGRATION.md#093-to-0100-smb-311-minimum-reserved-namespace-string-prefixes) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#0100-unreleased)
- **Action needed** — Backup 0.15.0: storage 0.10.0 [migration](../../crates/graphql-orm-backup/MIGRATION.md#0140-to-0150-storage-0100) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0150-unreleased)

### workspace-2026.11.12.3

Commit `da1782bfe2c3` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.3)

Packages (changed in bold): **`graphql-orm` 0.127.0, `graphql-orm-macros` 0.127.0**; `graphql-orm-operation-catalog` 0.7.3; **`graphql-orm-storage` 0.9.3**; `graphql-orm-backup` 0.14.0; `graphql-orm-ai-tool-profiles` 0.22.0; `graphql-orm-ai` 0.133.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.12.0**.

- **Action needed** — ORM/macros 0.127.0: repository callers keep public hook refusal codes [migration](../../MIGRATION.md#01260-to-01270-repository-callers-keep-public-hook-refusal-codes) · [changelog](../../CHANGELOG.md#01270)
- **No action** — Storage 0.9.3: digest-pinned managed Samba fixture [migration](../../crates/graphql-orm-storage/MIGRATION.md#092-to-093-digest-pinned-managed-samba-fixture) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#093-unreleased)
- **No action** — Router 0.12.0: opt-in byte routes [migration](../../crates/graphql-orm-router/MIGRATION.md#0110-to-0120-opt-in-byte-routes) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0120-unreleased)

### workspace-2026.11.12.2

Commit `3f002bcbcebe` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.2)

Packages (changed in bold): **`graphql-orm` 0.126.0, `graphql-orm-macros` 0.126.0**; **`graphql-orm-operation-catalog` 0.7.3**; `graphql-orm-storage` 0.9.2; `graphql-orm-backup` 0.14.0; **`graphql-orm-ai-tool-profiles` 0.22.0**; **`graphql-orm-ai` 0.133.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.11.0**.

- **Action needed** — ORM/macros 0.126.0: ORM runtime/macros 0.126.0 and operation catalogue 0.7.3 ship together with AI 0.133.0, tool profiles 0.22.0 and router 0.11.0. [migration](../../MIGRATION.md#01250-to-01260) · [changelog](../../CHANGELOG.md#01260)
- **See changelog** — Operation catalogue 0.7.3: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#01260)
- **Action needed** — AI tool profiles 0.22.0: complete capability ranking [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0211-to-0220-complete-capability-ranking) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#unreleased---0220)
- **Action needed** — AI 0.133.0: AI 0.133.0 depends on published Auth `v0.26.0`, locked to commit `7d016f6a6bc3e697382e2e05865f596862189240`, and ORM 0.126.0. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01320-to-01330) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01330)
- **Action needed** — Router 0.11.0: The `auth-agql` public constructors take types from published Auth 0.26.0. [migration](../../crates/graphql-orm-router/MIGRATION.md#0100-to-0110) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0110-unreleased)

### workspace-2026.11.12.1

Commit `81cc161e9f65` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.12.1)

Packages (changed in bold): `graphql-orm` 0.125.0, `graphql-orm-macros` 0.125.0; `graphql-orm-operation-catalog` 0.7.2; **`graphql-orm-storage` 0.9.2**; **`graphql-orm-backup` 0.14.0**; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.132.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **No action** — Storage 0.9.2: ordered and durable local storage [migration](../../crates/graphql-orm-storage/MIGRATION.md#091-to-092-ordered-and-durable-local-storage) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#092)
- **Action needed** — Backup 0.14.0: lock records, locked reads and write-side limits [migration](../../crates/graphql-orm-backup/MIGRATION.md#0130-to-0140-lock-records-locked-reads-and-write-side-limits) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0140-unreleased)
- **Action needed** — AI 0.132.0: credential authority for provider endpoint changes [migration](../../crates/graphql-orm-ai/MIGRATION.md#01310-to-01320-credential-authority-for-provider-endpoint-changes) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01320)

### workspace-2026.11.11.11

Commit `663479fc887a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.11)

Packages (changed in bold): `graphql-orm` 0.125.0, `graphql-orm-macros` 0.125.0; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.9.1; **`graphql-orm-backup` 0.13.0**; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.131.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — Backup 0.13.0: authenticated manifests and constrained restore targets [migration](../../crates/graphql-orm-backup/MIGRATION.md#0121-to-0130-authenticated-manifests-and-constrained-restore-targets) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0130-unreleased)

### workspace-2026.11.11.10

Commit `a25f25b79077` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.10)

Packages (changed in bold): `graphql-orm` 0.125.0, `graphql-orm-macros` 0.125.0; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.9.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.131.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **No action** — AI 0.131.0: legacy delegated stored principal references [migration](../../crates/graphql-orm-ai/MIGRATION.md#01300-to-01310-legacy-delegated-stored-principal-references) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01310)

### workspace-2026.11.11.9

Commit `7430eaa60428` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.9)

Packages (changed in bold): **`graphql-orm` 0.125.0, `graphql-orm-macros` 0.125.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.9.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.130.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.125.0: PostgreSQL records change-journal entries in the database [migration](../../MIGRATION.md#01250-postgresql-records-change-journal-entries-in-the-database) · [changelog](../../CHANGELOG.md#01250)

### workspace-2026.11.11.8

Commit `fc8bdb2cd4f7` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.8)

Packages (changed in bold): `graphql-orm` 0.124.0, `graphql-orm-macros` 0.124.0; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.9.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.130.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — AI 0.130.0: delegated and sessionless session credentials [migration](../../crates/graphql-orm-ai/MIGRATION.md#01290-to-01300-delegated-and-sessionless-session-credentials) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01300)

### workspace-2026.11.11.7

Commit `32e96a3bd088` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.7)

Packages (changed in bold): `graphql-orm` 0.124.0, `graphql-orm-macros` 0.124.0; `graphql-orm-operation-catalog` 0.7.2; **`graphql-orm-storage` 0.9.1**; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.129.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **No action** — Storage 0.9.1: backend-owned temporary names [migration](../../crates/graphql-orm-storage/MIGRATION.md#090-to-091-backend-owned-temporary-names) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#091)
- **Action needed** — AI 0.129.0: delegated and sessionless request credentials [migration](../../crates/graphql-orm-ai/MIGRATION.md#01280-to-01290-delegated-and-sessionless-request-credentials) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01290)

### workspace-2026.11.11.6

Commit `67a36b10aafe` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.6)

Packages (changed in bold): `graphql-orm` 0.124.0, `graphql-orm-macros` 0.124.0; `graphql-orm-operation-catalog` 0.7.2; **`graphql-orm-storage` 0.9.0**; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.128.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — Storage 0.9.0: SMB resource bounds and server identity [migration](../../crates/graphql-orm-storage/MIGRATION.md#081-to-090-smb-resource-bounds-and-server-identity) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#090-unreleased)

### workspace-2026.11.11.5

Commit `119511e7d342` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.5)

Packages (changed in bold): `graphql-orm` 0.124.0, `graphql-orm-macros` 0.124.0; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.128.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — AI 0.128.0: enforced runtime closure after restore [migration](../../crates/graphql-orm-ai/MIGRATION.md#01270-to-01280-enforced-runtime-closure-after-restore) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01280)

### workspace-2026.11.11.4

Commit `099951b60ad6` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.4)

Packages (changed in bold): **`graphql-orm` 0.124.0, `graphql-orm-macros` 0.124.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.127.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.124.0: assurance evaluators take a request check [migration](../../MIGRATION.md#01240-assurance-evaluators-take-a-request-check) · [changelog](../../CHANGELOG.md#01240)

### workspace-2026.11.11.3

Commit `15fb1ddf9216` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.3)

Packages (changed in bold): **`graphql-orm` 0.123.0, `graphql-orm-macros` 0.123.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.127.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.123.0: runtime date-time and JSON input must be readable [migration](../../MIGRATION.md#01230-runtime-date-time-and-json-input-must-be-readable) · [changelog](../../CHANGELOG.md#01230)

### workspace-2026.11.11.2

Commit `551fb1e988df` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.2)

Packages (changed in bold): **`graphql-orm` 0.122.0, `graphql-orm-macros` 0.122.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.127.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.122.0: operation assurance on runtime, custom and Federation roots [migration](../../MIGRATION.md#01220-operation-assurance-on-runtime-custom-and-federation-roots) · [changelog](../../CHANGELOG.md#01220)

### workspace-2026.11.11.1

Commit `0d67c5bb537b` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.11.1)

Packages (changed in bold): **`graphql-orm` 0.121.0, `graphql-orm-macros` 0.121.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.127.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.121.0: schema stages refuse destructive steps by default [migration](../../MIGRATION.md#01210-schema-stages-refuse-destructive-steps-by-default) · [changelog](../../CHANGELOG.md#01210)

### workspace-2026.11.10.3

Commit `a09599f7df11` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.10.3)

Packages (changed in bold): **`graphql-orm` 0.120.0, `graphql-orm-macros` 0.120.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.127.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.120.0: PostgreSQL RLS enforcement and ORM-managed table coverage [migration](../../MIGRATION.md#01200-postgresql-rls-enforcement-and-orm-managed-table-coverage) · [changelog](../../CHANGELOG.md#01200)
- **Action needed** — ORM/macros 0.119.0: foreign-key dependency ordering [migration](../../MIGRATION.md#01190-foreign-key-dependency-ordering) · [changelog](../../CHANGELOG.md#01190)

### workspace-2026.11.10.2

Commit `c8fd3a149325` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.10.2)

Packages (changed in bold): **`graphql-orm` 0.118.0, `graphql-orm-macros` 0.118.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.1; `graphql-orm-backup` 0.12.1; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.127.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.10.0.

- **Action needed** — ORM/macros 0.118.0: authorized relation count ordering [migration](../../MIGRATION.md#01180-authorized-relation-count-ordering) · [changelog](../../CHANGELOG.md#01180)

### workspace-2026.11.10.1

Commit `26fe3f25b490` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.10.1)

Packages (changed in bold): `graphql-orm` 0.113.0, `graphql-orm-macros` 0.113.0; `graphql-orm-operation-catalog` 0.7.2; **`graphql-orm-storage` 0.8.1**; **`graphql-orm-backup` 0.12.1**; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.127.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.10.0**.

- **No action** — Storage 0.8.1: SMB response verification (GHSA-439h-2cx8-xwgx) [migration](../../crates/graphql-orm-storage/MIGRATION.md#080-to-081-smb-response-verification-ghsa-439h-2cx8-xwgx) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#081-unreleased)
- **No action** — Backup 0.12.1: fail-closed pruning (GHSA-439h-2cx8-xwgx) [migration](../../crates/graphql-orm-backup/MIGRATION.md#0120-to-0121-fail-closed-pruning-ghsa-439h-2cx8-xwgx) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0121-unreleased)
- **No action** — AI 0.127.0: alternate-backend test hygiene [migration](../../crates/graphql-orm-ai/MIGRATION.md#01260-to-01270-alternate-backend-test-hygiene) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01270)
- **Action needed** — Router 0.10.0: subscriptions are WebSocket-only [migration](../../crates/graphql-orm-router/MIGRATION.md#091-to-0100-subscriptions-are-websocket-only) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#0100-unreleased)

### workspace-2026.11.09.9

Commit `72bef967ffcd` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.9)

Packages (changed in bold): **`graphql-orm` 0.113.0, `graphql-orm-macros` 0.113.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.126.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.9.1**.

- **Action needed** — ORM/macros 0.113.0: SQLite internal-name introspection [migration](../../MIGRATION.md#01130-sqlite-internal-name-introspection) · [changelog](../../CHANGELOG.md#01130)
- **No action** — ORM/macros 0.112.0: isolated consumer fixture builds [migration](../../MIGRATION.md#01110-to-01120-isolated-consumer-fixture-builds) · [changelog](../../CHANGELOG.md#01120)
- **No action** — Router 0.9.1: introspection declaration order [migration](../../crates/graphql-orm-router/MIGRATION.md#091-introspection-declaration-order) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#091-unreleased)

### workspace-2026.11.09.8

Commit `0ea79040f675` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.8)

Packages (changed in bold): **`graphql-orm` 0.111.0, `graphql-orm-macros` 0.111.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.126.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.9.0**.

- **Action needed** — ORM/macros 0.111.0: SQLite column/check/FK rebuild preservation [migration](../../MIGRATION.md#01110-sqlite-columncheckfk-rebuild-preservation) · [changelog](../../CHANGELOG.md#01110)
- **No action** — ORM/macros 0.110.0: MSSQL composite-key regression coverage [migration](../../MIGRATION.md#01090-to-01100-mssql-composite-key-regression-coverage) · [changelog](../../CHANGELOG.md#01100)
- **No action** — ORM/macros 0.109.0: fixture lockfiles for router 0.8.3 [migration](../../MIGRATION.md#01090-fixture-lockfiles-for-router-083) · [changelog](../../CHANGELOG.md#01090)
- **Action needed** — Router 0.9.0: subscription continuity [migration](../../crates/graphql-orm-router/MIGRATION.md#083-to-090-subscription-continuity) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#090-unreleased)
- **Action needed** — Router 0.8.3: terminal WebSocket errors [migration](../../crates/graphql-orm-router/MIGRATION.md#082-to-083-terminal-websocket-errors) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#083-unreleased)

### workspace-2026.11.09.7

Commit `f6608aa1eb96` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.7)

Packages (changed in bold): **`graphql-orm` 0.108.0, `graphql-orm-macros` 0.108.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.126.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.108.0: composite unique migrations [migration](../../MIGRATION.md#01080-composite-unique-migrations) · [changelog](../../CHANGELOG.md#01080)

### workspace-2026.11.09.6

Commit `70595072647c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.6)

Packages (changed in bold): **`graphql-orm` 0.107.0, `graphql-orm-macros` 0.107.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.126.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.107.0: conditional relation conversion [migration](../../MIGRATION.md#01060-to-01070-conditional-relation-conversion)

### workspace-2026.11.09.5

Commit `a12023c1703b` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.5)

Packages (changed in bold): **`graphql-orm` 0.106.0, `graphql-orm-macros` 0.106.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.126.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.106.0: SQLite rebuild cleanup data safety [migration](../../MIGRATION.md#01050-to-01060-sqlite-rebuild-cleanup-data-safety)

### workspace-2026.11.09.4

Commit `555a4a117fd4` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.4)

Packages (changed in bold): **`graphql-orm` 0.105.0, `graphql-orm-macros` 0.105.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.126.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.105.0: static relation readability [migration](../../MIGRATION.md#01040-to-01050-static-relation-readability)
- **Action needed** — ORM/macros 0.104.0: empty order input diagnostic [migration](../../MIGRATION.md#01030-to-01040-empty-order-input-diagnostic)
- **No action** — AI 0.126.0: alternate-backend test compilation [migration](../../crates/graphql-orm-ai/MIGRATION.md#01240-to-01260-alternate-backend-test-compilation) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01260)

### workspace-2026.11.09.3

Commit `402d86138d08` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.3)

Packages (changed in bold): **`graphql-orm` 0.103.0, `graphql-orm-macros` 0.103.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.125.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.103.0: static-to-runtime readability [migration](../../MIGRATION.md#01020-to-01030-static-to-runtime-readability)
- **No action** — AI 0.125.0: private-key session activity [migration](../../crates/graphql-orm-ai/MIGRATION.md#01240-to-01250-private-key-session-activity) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01250)

### workspace-2026.11.09.2

Commit `2a6bbeaa5a53` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.2)

Packages (changed in bold): **`graphql-orm` 0.102.0, `graphql-orm-macros` 0.102.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.124.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.102.0: create authorization and append-only composite keys [migration](../../MIGRATION.md#01000-to-01020-create-authorization-and-append-only-composite-keys)

### workspace-2026.11.09.1

Commit `a9fd882afc99` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.09.1)

Packages (changed in bold): **`graphql-orm` 0.100.0, `graphql-orm-macros` 0.100.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.124.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.100.0: CacheSync authority timing [migration](../../MIGRATION.md#0990-to-01000-cachesync-authority-timing)

### workspace-2026.11.08.2

Commit `4fb9817fbb07` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.08.2)

Packages (changed in bold): **`graphql-orm` 0.99.0, `graphql-orm-macros` 0.99.0**; `graphql-orm-operation-catalog` 0.7.2; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.124.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.2.

- **Action needed** — ORM/macros 0.99.0: dependent-delete catalogue integrity [migration](../../MIGRATION.md#0980-to-0990-dependent-delete-catalogue-integrity)

### workspace-2026.11.08.1

Commit `a5d862f8d53e` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.08.1)

Packages (changed in bold): **`graphql-orm` 0.98.0, `graphql-orm-macros` 0.98.0**; **`graphql-orm-operation-catalog` 0.7.2**; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; `graphql-orm-ai` 0.124.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.8.2**.

- **Action needed** — ORM/macros 0.98.0: unreadable fields, relations and safe errors [migration](../../MIGRATION.md#0970-to-0980-unreadable-fields-relations-and-safe-errors)
- **No action** — ORM/macros 0.97.0: optional dependent deletes [migration](../../MIGRATION.md#0960-to-0970-optional-dependent-deletes)
- **Action needed** — ORM/macros 0.96.0: Changed admission and subscriber revocation [migration](../../MIGRATION.md#0951-to-0960-changed-admission-and-subscriber-revocation)
- **No action** — ORM/macros 0.95.1: in-memory SQLite lifetime [migration](../../MIGRATION.md#0950-to-0951-in-memory-sqlite-lifetime)
- **See changelog** — Operation catalogue 0.7.2: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#01080)
- **No action** — Router 0.8.2: owned wire-test listener addresses [migration](../../crates/graphql-orm-router/MIGRATION.md#081-to-082-owned-wire-test-listener-addresses) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#082-unreleased)

### workspace-2026.11.07.2

Commit `e194facfb8f0` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.07.2)

Packages (changed in bold): **`graphql-orm` 0.95.0, `graphql-orm-macros` 0.95.0**; `graphql-orm-operation-catalog` 0.7.1; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.21.1; **`graphql-orm-ai` 0.124.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **Action needed** — ORM/macros 0.95.0: opt-in runtime identity and live contracts [migration](../../MIGRATION.md#0940-to-0950-opt-in-runtime-identity-and-live-contracts)
- **No action** — ORM/macros 0.95.0: runtime search catalog parity [migration](../../MIGRATION.md#0940-to-0950-runtime-search-catalog-parity)
- **No action** — AI 0.124.0: runtime catalog contract fixtures [migration](../../crates/graphql-orm-ai/MIGRATION.md#01230-to-01240-runtime-catalog-contract-fixtures) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01240)
- **No action** — AI 0.124.0: media fixture isolation [migration](../../crates/graphql-orm-ai/MIGRATION.md#ai-01230-to-01240-media-fixture-isolation) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01240)
- **No action** — AI 0.124.0: optional Codex read-marker tolerance [migration](../../crates/graphql-orm-ai/MIGRATION.md#ai-01230-to-01240-optional-codex-read-marker-tolerance) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01240)

### workspace-2026.11.07.1

Commit `63335a5d12ff` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.07.1)

Packages (changed in bold): **`graphql-orm` 0.94.0, `graphql-orm-macros` 0.94.0**; **`graphql-orm-operation-catalog` 0.7.1**; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; **`graphql-orm-ai-tool-profiles` 0.21.1**; **`graphql-orm-ai` 0.123.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **No action** — ORM/macros 0.94.0: paginated relations over SQL expression columns [migration](../../MIGRATION.md#0930-to-0940-paginated-relations-over-sql-expression-columns)
- **Action needed** — ORM/macros 0.94.0: relations beside a generated object [migration](../../MIGRATION.md#0930-to-0940-relations-beside-a-generated-object)
- **Action needed** — ORM/macros 0.94.0: single-row callback error privacy [migration](../../MIGRATION.md#0930-to-0940-single-row-callback-error-privacy)
- **No action** — ORM/macros 0.94.0: exact receiver-purpose admission (opt in) [migration](../../MIGRATION.md#0930-to-0940-exact-receiver-purpose-admission-opt-in)
- **See changelog** — Operation catalogue 0.7.1: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#01080)
- **No action** — AI tool profiles 0.21.1: exact receiver-purpose admission (opt in) [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0210-to-0211-exact-receiver-purpose-admission-opt-in) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#unreleased---0211)
- **No action** — AI 0.123.0: exact receiver-purpose admission (opt in) [migration](../../crates/graphql-orm-ai/MIGRATION.md#01220-to-01230-exact-receiver-purpose-admission-opt-in) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01230)
- **No action** — AI 0.123.0: Codex executable version admission [migration](../../crates/graphql-orm-ai/MIGRATION.md#01220-to-01230-codex-executable-version-admission) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01230)

### workspace-2026.11.06.3

Commit `f0722a09d81a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.06.3)

Packages (changed in bold): **`graphql-orm` 0.93.0, `graphql-orm-macros` 0.93.0**; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; **`graphql-orm-ai-tool-profiles` 0.21.0**; **`graphql-orm-ai` 0.122.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **No action** — ORM/macros 0.93.0: after-commit callback documented [migration](../../MIGRATION.md#0920-to-0930-after-commit-callback-documented)
- **Action needed** — ORM/macros 0.93.0: transforms once per input row [migration](../../MIGRATION.md#0920-to-0930-transforms-once-per-input-row)
- **No action** — ORM/macros 0.93.0: generated mutation shape selection [migration](../../MIGRATION.md#0920-to-0930-generated-mutation-shape-selection)
- **No action** — ORM/macros 0.93.0: child-delete parent invalidations [migration](../../MIGRATION.md#0920-to-0930-child-delete-parent-invalidations)
- **No action** — ORM/macros 0.93.0: generated compile cost [migration](../../MIGRATION.md#0920-to-0930-generated-compile-cost)
- **Action needed** — AI tool profiles 0.21.0: retry refusal reasons [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0200-to-0210-retry-refusal-reasons) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0210-unreleased)
- **Action needed** — AI 0.122.0: retry refusal reasons [migration](../../crates/graphql-orm-ai/MIGRATION.md#01210-to-01220-retry-refusal-reasons) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01220)
- **No action** — AI 0.122.0: catalogue-change regression coverage [migration](../../crates/graphql-orm-ai/MIGRATION.md#ai-01210-to-01220-catalogue-change-regression-coverage) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01220)

### workspace-2026.11.06.2

Commit `33789201e200` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.06.2)

Packages (changed in bold): **`graphql-orm` 0.92.0, `graphql-orm-macros` 0.92.0**; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; **`graphql-orm-ai` 0.121.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **Action needed** — ORM/macros 0.92.0: bulk post-commit notifications [migration](../../MIGRATION.md#0911-to-0920-bulk-post-commit-notifications)
- **Action needed** — ORM/macros 0.92.0: bulk refusal payloads [migration](../../MIGRATION.md#0911-to-0920-bulk-refusal-payloads)
- **Action needed** — ORM/macros 0.92.0: rejected field writes fail the mutation [migration](../../MIGRATION.md#0911-to-0920-rejected-field-writes-fail-the-mutation)
- **Action needed** — ORM/macros 0.92.0: type-aware implicit timestamps [migration](../../MIGRATION.md#0911-to-0920-type-aware-implicit-timestamps)
- **Action needed** — ORM/macros 0.92.0: relation denial contract [migration](../../MIGRATION.md#0911-to-0920-relation-denial-contract)
- **No action** — ORM/macros 0.92.0: relation-bearing identity fields [migration](../../MIGRATION.md#0911-to-0920-relation-bearing-identity-fields)
- **Action needed** — ORM/macros 0.92.0: keyset cursors bind entity and filter [migration](../../MIGRATION.md#0911-to-0920-keyset-cursors-bind-entity-and-filter)
- **Action needed** — ORM/macros 0.92.0: authorized static collection filters [migration](../../MIGRATION.md#0911-to-0920-authorized-static-collection-filters)
- **No action** — ORM/macros 0.92.0: private and negated read predicates [migration](../../MIGRATION.md#0911-to-0920-private-and-negated-read-predicates)
- **No action** — ORM/macros 0.92.0: aggregate counts from two subgraphs, test only [migration](../../MIGRATION.md#0911-to-0920-aggregate-counts-from-two-subgraphs-test-only)
- **No action** — ORM/macros 0.92.0: selected connection queries [migration](../../MIGRATION.md#0911-to-0920-selected-connection-queries)
- **No action** — ORM/macros 0.92.0: narrow relation pagination windows [migration](../../MIGRATION.md#0911-to-0920-narrow-relation-pagination-windows)
- **No action** — ORM/macros 0.92.0: dedicated schema export builds [migration](../../MIGRATION.md#0911-to-0920-dedicated-schema-export-builds)
- **Action needed** — AI 0.121.0: finite provider session cleanup [migration](../../crates/graphql-orm-ai/MIGRATION.md#01202-to-01210-finite-provider-session-cleanup)

### workspace-2026.11.06.1

Commit `9d1a4821248c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.06.1)

Packages (changed in bold): **`graphql-orm` 0.91.1, `graphql-orm-macros` 0.91.1**; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; **`graphql-orm-ai` 0.120.2**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **No action** — ORM/macros 0.91.1: no code change (ships with AI 0.120.2) [migration](../../MIGRATION.md#ai-01201-to-01202-codex-01660-and-orm-0910-to-0911)
- **No action** — AI 0.120.2: Codex 0.166.0 lifecycle timing [migration](../../crates/graphql-orm-ai/MIGRATION.md#01201-to-01202-codex-01660-lifecycle-timing) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01202)

### workspace-2026.11.05.1

Commit `68d721afef80` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.05.1)

Packages (changed in bold): **`graphql-orm` 0.91.0, `graphql-orm-macros` 0.91.0**; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; `graphql-orm-ai` 0.120.1; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **Action needed** — ORM/macros 0.91.0: isolated runtime search ranking and fenced backfill [migration](../../MIGRATION.md#0901-to-0910-isolated-runtime-search-ranking-and-fenced-backfill)

### workspace-2026.11.04.1

Commit `adfa608d515a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.04.1)

Packages (changed in bold): **`graphql-orm` 0.90.1, `graphql-orm-macros` 0.90.1**; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; **`graphql-orm-ai` 0.120.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **Action needed** — ORM/macros 0.90.1: composite subscription authorization [migration](../../MIGRATION.md#0900-to-0901-composite-subscription-authorization)
- **Action needed** — ORM/macros 0.90.0: restrictive empty filters and complete change provenance [migration](../../MIGRATION.md#0890-to-0900-restrictive-empty-filters-and-complete-change-provenance)
- **Action needed** — Workspace: Dependency advisory remediation [migration](../../MIGRATION.md#dependency-advisory-remediation)
- **No action** — Workspace: Router binary notice refresh [migration](../../MIGRATION.md#router-binary-notice-refresh)
- **No action** — AI 0.120.1: documentation package identity [migration](../../crates/graphql-orm-ai/MIGRATION.md#01200-to-01201-documentation-package-identity) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01201)

### workspace-2026.11.01.2

Commit `5225a301f1a9` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.01.2)

Packages (changed in bold): `graphql-orm` 0.89.0, `graphql-orm-macros` 0.89.0; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; `graphql-orm-ai` 0.120.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **No action** — no host-facing entries.

### workspace-2026.11.01.1

Commit `c9465e5134ce` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.11.01.1)

Packages (changed in bold): **`graphql-orm` 0.89.0, `graphql-orm-macros` 0.89.0**; `graphql-orm-operation-catalog` 0.7.0; `graphql-orm-storage` 0.8.0; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; **`graphql-orm-ai` 0.120.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.1.

- **Action needed** — ORM/macros 0.89.0: full-text search on runtime collections [migration](../../MIGRATION.md#0880-to-0890-full-text-search-on-runtime-collections) · [changelog](../../CHANGELOG.md#0890-unreleased)
- **Action needed** — AI 0.120.0: ORM 0.89.0 [migration](../../crates/graphql-orm-ai/MIGRATION.md#01190-to-01200-orm-0890) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01200)

### workspace-2026.10.31.2

Commit `3bc5331afd53` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.31.2)

Packages (changed in bold): **`graphql-orm` 0.88.0, `graphql-orm-macros` 0.88.0**; `graphql-orm-operation-catalog` 0.7.0; **`graphql-orm-storage` 0.8.0**; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.20.0; `graphql-orm-ai` 0.119.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.8.1**.

- **Action needed** — ORM/macros 0.88.0: Changed stream continuity [migration](../../MIGRATION.md#0870-to-0880-changed-stream-continuity) · [changelog](../../CHANGELOG.md#0880-unreleased)
- **Action needed** — Storage 0.8.0: resumable multipart uploads [migration](../../crates/graphql-orm-storage/MIGRATION.md#075-to-080-resumable-multipart-uploads) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#080-unreleased)
- **No action** — Router 0.8.1: subscription error frame test [migration](../../crates/graphql-orm-router/MIGRATION.md#080-to-081-subscription-error-frame-test) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#081-unreleased)

### workspace-2026.10.31.1

Commit `8eaa491f6862` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.31.1)

Packages (changed in bold): **`graphql-orm` 0.87.0, `graphql-orm-macros` 0.87.0**; **`graphql-orm-operation-catalog` 0.7.0**; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.12.0; **`graphql-orm-ai-tool-profiles` 0.20.0**; **`graphql-orm-ai` 0.119.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.8.0.

- **Action needed** — ORM/macros 0.87.0: RecordVersion and catalogue record versions [migration](../../MIGRATION.md#0860-to-0870-recordversion-and-catalogue-record-versions) · [changelog](../../CHANGELOG.md#0870-unreleased)
- **See changelog** — Operation catalogue 0.7.0: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#0870-unreleased)
- **Action needed** — AI tool profiles 0.20.0: operation catalogue 0.7.0 [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0190-to-0200-operation-catalogue-070) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0200-unreleased)
- **Action needed** — AI 0.119.0: operation catalogue 0.7.0 [migration](../../crates/graphql-orm-ai/MIGRATION.md#01180-to-01190-operation-catalogue-070) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01190-unreleased)

### workspace-2026.10.30.4

Commit `475178e6c207` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.30.4)

Packages (changed in bold): **`graphql-orm` 0.86.0, `graphql-orm-macros` 0.86.0**; `graphql-orm-operation-catalog` 0.6.0; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.19.0; `graphql-orm-ai` 0.118.0; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.8.0**.

- **No action** — ORM/macros 0.86.0: global NodeId and router node lookup [migration](../../MIGRATION.md#0850-to-0860-global-nodeid-and-router-node-lookup) · [changelog](../../CHANGELOG.md#0860-unreleased)
- **No action** — Router 0.8.0: opt-in global node lookup [migration](../../crates/graphql-orm-router/MIGRATION.md#070-to-080-opt-in-global-node-lookup) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#080-unreleased)

### workspace-2026.10.30.3

Commit `89188006244b` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.30.3)

Packages (changed in bold): **`graphql-orm` 0.85.0, `graphql-orm-macros` 0.85.0**; `graphql-orm-operation-catalog` 0.6.0; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.19.0; `graphql-orm-ai` 0.118.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.7.0.

- **No action** — ORM/macros 0.85.0: keyed Changed subscriptions [migration](../../MIGRATION.md#0840-to-0850-keyed-changed-subscriptions) · [changelog](../../CHANGELOG.md#0850-unreleased)

### workspace-2026.10.30.2

Commit `b35af752f749` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.30.2)

Packages (changed in bold): **`graphql-orm` 0.84.0, `graphql-orm-macros` 0.84.0**; `graphql-orm-operation-catalog` 0.6.0; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.12.0; `graphql-orm-ai-tool-profiles` 0.19.0; `graphql-orm-ai` 0.118.0; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.7.0.

- **No action** — ORM/macros 0.84.0: managed record versions [migration](../../MIGRATION.md#0820-to-0840-managed-record-versions) · [changelog](../../CHANGELOG.md#0840-unreleased)

### workspace-2026.10.30.1

Commit `67caaad02580` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.30.1)

Packages (changed in bold): **`graphql-orm` 0.82.0, `graphql-orm-macros` 0.82.0**; **`graphql-orm-operation-catalog` 0.6.0**; `graphql-orm-storage` 0.7.5; **`graphql-orm-backup` 0.12.0**; **`graphql-orm-ai-tool-profiles` 0.19.0**; **`graphql-orm-ai` 0.118.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.7.0**.

- **Action needed** — ORM/macros 0.82.0: shareable aggregate result objects [migration](../../MIGRATION.md#0810-to-0820-shareable-aggregate-result-objects) · [changelog](../../CHANGELOG.md#0820-unreleased)
- **No action** — ORM/macros 0.82.0: exact static signed64 filters [migration](../../MIGRATION.md#0810-to-0820-exact-static-signed64-filters) · [changelog](../../CHANGELOG.md#0820-unreleased)
- **Action needed** — ORM/macros 0.82.0: generated cross-crate upward Changed hooks [migration](../../MIGRATION.md#0810-to-0820-generated-cross-crate-upward-changed-hooks) · [changelog](../../CHANGELOG.md#0820-unreleased)
- **No action** — ORM/macros 0.81.1: post-commit record release [migration](../../MIGRATION.md#0810-to-0811-post-commit-record-release)
- **Action needed** — ORM/macros 0.81.0: runtime actor stamps [migration](../../MIGRATION.md#0800-to-0810-runtime-actor-stamps) · [changelog](../../CHANGELOG.md#0810-unreleased)
- **Action needed** — ORM/macros 0.80.0: canonical typed filter contracts [migration](../../MIGRATION.md#0780-to-0800-canonical-typed-filter-contracts) · [changelog](../../CHANGELOG.md#0800-unreleased)
- **Action needed** — ORM/macros 0.79.0: explicit GraphQL features [migration](../../MIGRATION.md#0780-to-0790-explicit-graphql-features)
- **Action needed** — Workspace: Published agql-auth 0.24.0 selection [migration](../../MIGRATION.md#published-agql-auth-0240-selection)
- **See changelog** — Operation catalogue 0.6.0: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#0800-unreleased)
- **Action needed** — Backup 0.12.0: optional ORM public type identity [migration](../../crates/graphql-orm-backup/MIGRATION.md#0111-to-0120-optional-orm-public-type-identity) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0120-unreleased)
- **Action needed** — AI tool profiles 0.19.0: typed filter authority and semantic input ownership [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0170-to-0190-typed-filter-authority-and-semantic-input-ownership) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0190-unreleased)
- **Action needed** — AI tool profiles 0.18.0: explicit GraphQL features [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0170-to-0180-explicit-graphql-features)
- **Action needed** — AI 0.118.0: canonical filter contract type identity [migration](../../crates/graphql-orm-ai/MIGRATION.md#01160-to-01180-canonical-filter-contract-type-identity) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01180-unreleased)
- **Action needed** — AI 0.117.0: explicit GraphQL features [migration](../../crates/graphql-orm-ai/MIGRATION.md#01160-to-01170-explicit-graphql-features) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01170-unreleased)
- **Action needed** — Router 0.7.0: canonical Auth 0.24 types [migration](../../crates/graphql-orm-router/MIGRATION.md#067-to-070-canonical-auth-024-types) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#070-unreleased)

### workspace-2026.10.27.1

Commit `856fe23d08bc` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.27.1)

Packages (changed in bold): **`graphql-orm` 0.78.0, `graphql-orm-macros` 0.78.0**; `graphql-orm-operation-catalog` 0.5.0; `graphql-orm-storage` 0.7.5; **`graphql-orm-backup` 0.11.1**; `graphql-orm-ai-tool-profiles` 0.17.0; **`graphql-orm-ai` 0.116.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.7**.

- **No action** — ORM/macros 0.78.0: to-many relations in mutation payloads [migration](../../MIGRATION.md#0770-to-0780-to-many-relations-in-mutation-payloads) · [changelog](../../CHANGELOG.md#0780-unreleased)
- **Action needed** — ORM/macros 0.77.0: date-time backups and self-reference cycles [migration](../../MIGRATION.md#0760-to-0770-date-time-backups-and-self-reference-cycles) · [changelog](../../CHANGELOG.md#0770-unreleased)
- **Action needed** — ORM/macros 0.76.0: delimited reserved-word identifiers [migration](../../MIGRATION.md#0750-to-0760-delimited-reserved-word-identifiers) · [changelog](../../CHANGELOG.md#0760-unreleased)
- **Action needed** — ORM/macros 0.75.0: automatic runtime timestamps [migration](../../MIGRATION.md#0740-to-0750-automatic-runtime-timestamps) · [changelog](../../CHANGELOG.md#0750-unreleased)
- **No action** — Backup 0.11.1: package release identity [migration](../../crates/graphql-orm-backup/MIGRATION.md#0110-to-0111-package-release-identity) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0111-unreleased)
- **No action** — AI 0.116.0: policy-checked Anthropic endpoint [migration](../../crates/graphql-orm-ai/MIGRATION.md#01150-to-01160-policy-checked-anthropic-endpoint) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01160)
- **Action needed** — AI 0.115.0: Chat Completions adapter, schema module 0.75.0 [migration](../../crates/graphql-orm-ai/MIGRATION.md#01140-to-01150-chat-completions-adapter-schema-module-0750) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01150)
- **No action** — AI 0.114.0: provider TLS selection, stateless Responses replay [migration](../../crates/graphql-orm-ai/MIGRATION.md#01130-to-01140-provider-tls-selection-stateless-responses-replay) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01140)
- **No action** — Router 0.6.7: preparation ownership and fresh consumer dependencies [migration](../../crates/graphql-orm-router/MIGRATION.md#066-to-067-preparation-ownership-and-fresh-consumer-dependencies) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#067-unreleased)

### workspace-2026.10.26.1

Commit `4a760aec1c94` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.26.1)

Packages (changed in bold): **`graphql-orm` 0.74.0, `graphql-orm-macros` 0.74.0**; **`graphql-orm-operation-catalog` 0.5.0**; `graphql-orm-storage` 0.7.5; **`graphql-orm-backup` 0.11.0**; **`graphql-orm-ai-tool-profiles` 0.17.0**; **`graphql-orm-ai` 0.113.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.6**.

- **Action needed** — ORM/macros 0.74.0: bounded PostgreSQL identifiers [migration](../../MIGRATION.md#0730-to-0740-bounded-postgresql-identifiers) · [changelog](../../CHANGELOG.md#0740-unreleased)
- **Action needed** — ORM/macros 0.73.0: atomic link changes in writes [migration](../../MIGRATION.md#0720-to-0730-atomic-link-changes-in-writes) · [changelog](../../CHANGELOG.md#0730-unreleased)
- **Action needed** — ORM/macros 0.71.0: composite-key backups and link tables [migration](../../MIGRATION.md#0701-to-0710-composite-key-backups-and-link-tables) · [changelog](../../CHANGELOG.md#0710-unreleased)
- **Action needed** — ORM/macros 0.70.1: foreign-key replacement order [migration](../../MIGRATION.md#0700-to-0701-foreign-key-replacement-order) · [changelog](../../CHANGELOG.md#0701-unreleased)
- **Action needed** — ORM/macros 0.70.0: uniform refusal of hidden and missing link targets [migration](../../MIGRATION.md#0690-to-0700-uniform-refusal-of-hidden-and-missing-link-targets) · [changelog](../../CHANGELOG.md#0700-unreleased)
- **Action needed** — ORM/macros 0.69.0: many-to-many runtime relations [migration](../../MIGRATION.md#0680-to-0690-many-to-many-runtime-relations) · [changelog](../../CHANGELOG.md#0690-unreleased)
- **Action needed** — ORM/macros 0.68.0: runtime collection label fields [migration](../../MIGRATION.md#0670-to-0680-runtime-collection-label-fields) · [changelog](../../CHANGELOG.md#0680-unreleased)
- **Action needed** — ORM/macros 0.67.0: runtime relation filters and payload relations [migration](../../MIGRATION.md#0660-to-0670-runtime-relation-filters-and-payload-relations) · [changelog](../../CHANGELOG.md#0670-unreleased)
- **No action** — ORM/macros 0.66.0: opt-in runtime cascade and set-null deletes [migration](../../MIGRATION.md#0650-to-0660-opt-in-runtime-cascade-and-set-null-deletes) · [changelog](../../CHANGELOG.md#0660-unreleased)
- **Action needed** — ORM/macros 0.65.0: scalar policies with generated relations [migration](../../MIGRATION.md#0641-to-0650-scalar-policies-with-generated-relations) · [changelog](../../CHANGELOG.md#0650-unreleased)
- **Action needed** — ORM/macros 0.64.1: SQLite RESTRICT refusal classification [migration](../../MIGRATION.md#0640-to-0641-sqlite-restrict-refusal-classification) · [changelog](../../CHANGELOG.md#0641-unreleased)
- **Action needed** — ORM/macros 0.64.0: composite and read-only Changed [migration](../../MIGRATION.md#0630-to-0640-composite-and-read-only-changed) · [changelog](../../CHANGELOG.md#0640-unreleased)
- **Action needed** — ORM/macros 0.63.0: propagation relation tuples [migration](../../MIGRATION.md#0620-to-0630-propagation-relation-tuples) · [changelog](../../CHANGELOG.md#0630-unreleased)
- **No action** — ORM/macros 0.62.0: runtime keyset ordering [migration](../../MIGRATION.md#0610-to-0620-runtime-keyset-ordering) · [changelog](../../CHANGELOG.md#0620-unreleased)
- **Action needed** — Workspace: agql-auth 0.23.0 selection [migration](../../MIGRATION.md#agql-auth-0230-selection)
- **See changelog** — Operation catalogue 0.5.0: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#0680-unreleased)
- **Action needed** — Backup 0.11.0: composite-key tables [migration](../../crates/graphql-orm-backup/MIGRATION.md#0100-to-0110-composite-key-tables) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0110-unreleased)
- **Action needed** — AI tool profiles 0.17.0: operation catalog 0.5.0 [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0162-to-0170-operation-catalog-050) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0170-unreleased)
- **Action needed** — AI 0.113.0: deployment budget provisioning, schema module 0.74.0 [migration](../../crates/graphql-orm-ai/MIGRATION.md#01120-to-01130-deployment-budget-provisioning-schema-module-0740) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01130)
- **No action** — AI 0.112.0: opt-in host-bound principal projection [migration](../../crates/graphql-orm-ai/MIGRATION.md#01110-to-01120-opt-in-host-bound-principal-projection) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01120)
- **Action needed** — AI 0.111.0: schema module 0.73.0 bounded index name [migration](../../crates/graphql-orm-ai/MIGRATION.md#01101-to-01110-schema-module-0730-bounded-index-name) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01110)
- **No action** — AI 0.110.1: ORM 0.69.0 alignment [migration](../../crates/graphql-orm-ai/MIGRATION.md#01100-to-01101-orm-0690-alignment) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01101)
- **Action needed** — AI 0.110.0: operation catalog 0.5.0 [migration](../../crates/graphql-orm-ai/MIGRATION.md#01091-to-01100-operation-catalog-050) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01100)
- **No action** — Router 0.6.6: documentation package identity [migration](../../crates/graphql-orm-router/MIGRATION.md#065-to-066-documentation-package-identity) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#066-unreleased)

### workspace-2026.10.23.1

Commit `00ee7d9fbd9c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.23.1)

Packages (changed in bold): **`graphql-orm` 0.61.0, `graphql-orm-macros` 0.61.0**; **`graphql-orm-operation-catalog` 0.4.2**; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.10.0; **`graphql-orm-ai-tool-profiles` 0.16.2**; `graphql-orm-ai` 0.109.1; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.5**.

- **No action** — ORM/macros 0.61.0: subscription primary-key selection [migration](../../MIGRATION.md#0600-to-0610-subscription-primary-key-selection) · [changelog](../../CHANGELOG.md#0610-2026-10-23)
- **No action** — ORM/macros 0.60.0: generated query-shape selection [migration](../../MIGRATION.md#0590-to-0600-generated-query-shape-selection) · [changelog](../../CHANGELOG.md#0600-unreleased)
- **Action needed** — ORM/macros 0.59.0: composite-primary-key keysets [migration](../../MIGRATION.md#0580-to-0590-composite-primary-key-keysets)
- **No action** — ORM/macros 0.58.0: public host-owned timestamps [migration](../../MIGRATION.md#0570-to-0580-public-host-owned-timestamps)
- **No action** — Operation catalogue 0.4.2: operation catalog 0.4.2 / profiles 0.16.2 [migration](../../MIGRATION.md#generated-result-disclosure-patch-operation-catalog-042-profiles-0162) · [changelog](../../CHANGELOG.md#operation-catalog-042-ai-tool-profiles-0162-unreleased)
- **No action** — AI tool profiles 0.16.2: generated result disclosure [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0161-to-0162-generated-result-disclosure) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0162-unreleased)
- **No action** — Router 0.6.5: test startup diagnostics [migration](../../crates/graphql-orm-router/MIGRATION.md#064-to-065-test-startup-diagnostics) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#065-unreleased)

### workspace-2026.10.22.1

Commit `a90b5bd41f95` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.22.1)

Packages (changed in bold): **`graphql-orm` 0.57.0, `graphql-orm-macros` 0.57.0**; `graphql-orm-operation-catalog` 0.4.1; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.1; `graphql-orm-ai` 0.109.1; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.4**.

- **Action needed** — ORM/macros 0.57.0: exact SQL Server MONEY reads [migration](../../MIGRATION.md#0560-to-0570-exact-sql-server-money-reads) · [changelog](../../CHANGELOG.md#0570-2026-10-22)
- **Action needed** — Router 0.6.4: TLS certificate distrust patches [migration](../../crates/graphql-orm-router/MIGRATION.md#063-to-064-tls-certificate-distrust-patches) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#064---2026-10-22)

### workspace-2026.10.21.1

Commit `173401ff2350` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.21.1)

Packages (changed in bold): **`graphql-orm` 0.56.0, `graphql-orm-macros` 0.56.0**; **`graphql-orm-operation-catalog` 0.4.1**; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.1; **`graphql-orm-ai` 0.109.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.6.3.

- **Action needed** — ORM/macros 0.56.0: runtime descriptions and exact decimals [migration](../../MIGRATION.md#0551-to-0560-runtime-descriptions-and-exact-decimals) · [changelog](../../CHANGELOG.md#0560-2026-10-21)
- **See changelog** — Operation catalogue 0.4.1: ships with the ORM/macros changes listed for this release. [changelog](../../CHANGELOG.md#0560-2026-10-21)
- **No action** — AI 0.109.1: ORM 0.56.0 alignment [migration](../../crates/graphql-orm-ai/MIGRATION.md#01090-to-01091-orm-0560-alignment) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01091)

### workspace-2026.10.20.4

Commit `75f0f8c83b79` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.20.4)

Packages (changed in bold): **`graphql-orm` 0.55.1, `graphql-orm-macros` 0.55.1**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.1; **`graphql-orm-ai` 0.109.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.6.3.

- **No action** — ORM/macros 0.55.1: fixture lock refresh [migration](../../MIGRATION.md#0550-to-0551-fixture-lock-refresh) · [changelog](../../CHANGELOG.md#0551-unreleased)
- **Action needed** — AI 0.109.0: frozen retained tool surfaces in task-context budgets [migration](../../crates/graphql-orm-ai/MIGRATION.md#01080-to-01090-frozen-retained-tool-surfaces-in-task-context-budgets) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01090)

### workspace-2026.10.20.3

Commit `5d0672e80c96` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.20.3)

Packages (changed in bold): **`graphql-orm` 0.55.0, `graphql-orm-macros` 0.55.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.10.0; **`graphql-orm-ai-tool-profiles` 0.16.1**; **`graphql-orm-ai` 0.108.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.6.3.

- **Action needed** — ORM/macros 0.55.0: typed literal defaults and semantic field formats [migration](../../MIGRATION.md#0540-to-0550-typed-literal-defaults-and-semantic-field-formats) · [changelog](../../CHANGELOG.md#0550-unreleased)
- **No action** — AI tool profiles 0.16.1: This compatible patch adds `AiError::classification`, a distinct payload-free lower-snake-case name per variant. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0160-to-0161) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0161---2026-10-20)
- **Action needed** — AI 0.108.0: classified planner failures [migration](../../crates/graphql-orm-ai/MIGRATION.md#01074-to-01080-classified-planner-failures) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01080)

### workspace-2026.10.20.2

Commit `5932e0203cc4` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.20.2)

Packages (changed in bold): **`graphql-orm` 0.54.1, `graphql-orm-macros` 0.54.1**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.7.5; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.0; **`graphql-orm-ai` 0.107.4**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.3**.

- **Action needed** — ORM/macros 0.54.1: Pair ORM/macros 0.54.1 and AI 0.107.4 with published Auth `v0.22.1` at `86fabec09eea1eec0f675bc150e90e3809734567` for its finite signed-role-catalogue correction. [changelog](../../CHANGELOG.md#0541-unreleased)
- **Action needed** — Workspace: Signed role-catalogue transport bound [migration](../../MIGRATION.md#signed-role-catalogue-transport-bound)
- **Action needed** — AI 0.107.4: coordinated Auth source [migration](../../crates/graphql-orm-ai/MIGRATION.md#01073-to-01074-coordinated-auth-source) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased---01074)
- **Action needed** — Router 0.6.3: signed catalogue transport bound [migration](../../crates/graphql-orm-router/MIGRATION.md#062-to-063-signed-catalogue-transport-bound) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#063---2026-10-20)

### workspace-2026.10.20.1

Commit `b382f8a4fe28` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.20.1)

Packages (changed in bold): **`graphql-orm` 0.54.0, `graphql-orm-macros` 0.54.0**; `graphql-orm-operation-catalog` 0.4.0; **`graphql-orm-storage` 0.7.5**; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.0; **`graphql-orm-ai` 0.107.3**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.2**.

- **No action** — ORM/macros 0.54.0: service-owned cache synchronization [migration](../../MIGRATION.md#0530-to-0540-service-owned-cache-synchronization) · [changelog](../../CHANGELOG.md#0540-unreleased)
- **Action needed** — ORM/macros 0.53.0: post-await and owned release checks [migration](../../MIGRATION.md#0520-to-0530-post-await-and-owned-release-checks) · [changelog](../../CHANGELOG.md#0530-unreleased)
- **No action** — Storage 0.7.5: maintained protocol-validation package identity [migration](../../crates/graphql-orm-storage/MIGRATION.md#074-to-075-maintained-protocol-validation-package-identity) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#075---2026-10-18)
- **Action needed** — AI 0.107.3: maintained workspace package identity [migration](../../crates/graphql-orm-ai/MIGRATION.md#01072-to-01073-maintained-workspace-package-identity) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01073---2026-10-18)
- **Action needed** — Router 0.6.2: maintained workspace package identity [migration](../../crates/graphql-orm-router/MIGRATION.md#061-to-062-maintained-workspace-package-identity) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#062---2026-10-18)

### workspace-2026.10.19.1

Commit `4f39f1b925b5` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.19.1)

Packages (changed in bold): **`graphql-orm` 0.52.0, `graphql-orm-macros` 0.52.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.7.4; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.0; `graphql-orm-ai` 0.107.2; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.6.1.

- **No action** — ORM/macros 0.52.0: explicit native Rust 1.98.1 replay recipe [migration](../../MIGRATION.md#0510-to-0520-explicit-native-rust-1981-replay-recipe) · [changelog](../../CHANGELOG.md#0520-unreleased)

### workspace-2026.10.18.1

Commit `62fe2ddde77c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.18.1)

Packages (changed in bold): **`graphql-orm` 0.51.0, `graphql-orm-macros` 0.51.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.7.4; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.0; `graphql-orm-ai` 0.107.2; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.6.1.

- **No action** — ORM/macros 0.51.0: optional owned planning replay [migration](../../MIGRATION.md#0502-to-0510-optional-owned-planning-replay) · [changelog](../../CHANGELOG.md#0510-unreleased)

### workspace-2026.10.16.2

Commit `8e1e5698437d` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.16.2)

Packages (changed in bold): **`graphql-orm` 0.50.2, `graphql-orm-macros` 0.50.2**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.7.4; `graphql-orm-backup` 0.10.0; `graphql-orm-ai-tool-profiles` 0.16.0; **`graphql-orm-ai` 0.107.2**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.1**.

- **Action needed** — ORM/macros 0.50.2: published Auth source alignment [migration](../../MIGRATION.md#0501-to-0502-published-auth-source-alignment) · [changelog](../../CHANGELOG.md#0502-unreleased)
- **Action needed** — AI 0.107.2: published Auth selector [migration](../../crates/graphql-orm-ai/MIGRATION.md#01071-to-01072-published-auth-selector) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01072---2026-10-16)
- **Action needed** — Router 0.6.1: published Auth selector [migration](../../crates/graphql-orm-router/MIGRATION.md#060-to-061-published-auth-selector) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#061---2026-10-16)

### workspace-2026.10.16.1

Commit `e6863af4d35f` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.16.1)

Packages (changed in bold): **`graphql-orm` 0.50.1, `graphql-orm-macros` 0.50.1**; `graphql-orm-operation-catalog` 0.4.0; **`graphql-orm-storage` 0.7.4**; **`graphql-orm-backup` 0.10.0**; `graphql-orm-ai-tool-profiles` 0.16.0; `graphql-orm-ai` 0.107.1; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.6.0**.

- **Action needed** — ORM/macros 0.50.1: rendered read bind budgets [migration](../../MIGRATION.md#0500-to-0501-rendered-read-bind-budgets) · [changelog](../../CHANGELOG.md#0501-unreleased)
- **No action** — ORM/macros 0.50.0: checked filter intent and offline matching [migration](../../MIGRATION.md#0490-to-0500-checked-filter-intent-and-offline-matching) · [changelog](../../CHANGELOG.md#0500-unreleased)
- **No action** — ORM/macros 0.49.0: checked owned complete-key reads [migration](../../MIGRATION.md#048x-to-0490-checked-owned-complete-key-reads) · [changelog](../../CHANGELOG.md#0490-unreleased)
- **Action needed** — ORM/macros 0.48.3: unsupported PostgreSQL alterations fail closed [migration](../../MIGRATION.md#0482-to-0483-unsupported-postgresql-alterations-fail-closed) · [changelog](../../CHANGELOG.md#0483-unreleased)
- **Action needed** — ORM/macros 0.48.2: PostgreSQL default alterations [migration](../../MIGRATION.md#0481-to-0482-postgresql-default-alterations) · [changelog](../../CHANGELOG.md#0482-unreleased)
- **No action** — ORM/macros 0.48.1: PostgreSQL string default round trips [migration](../../MIGRATION.md#0480-to-0481-postgresql-string-default-round-trips) · [changelog](../../CHANGELOG.md#0481-unreleased)
- **Action needed** — ORM/macros 0.48.0: SQL Server platform TLS [migration](../../MIGRATION.md#0470-to-0480-sql-server-platform-tls) · [changelog](../../CHANGELOG.md#0480-unreleased)
- **Action needed** — ORM/macros 0.47.0: current authority and coherent accepted requests [migration](../../MIGRATION.md#0460-to-0470-current-authority-and-coherent-accepted-requests) · [changelog](../../CHANGELOG.md#0470-unreleased)
- **Action needed** — ORM/macros 0.46.0: bounded protected offset and search reads [migration](../../MIGRATION.md#0450-to-0460-bounded-protected-offset-and-search-reads) · [changelog](../../CHANGELOG.md#0460-unreleased)
- **Action needed** — ORM/macros 0.45.0: subscriptions authorize each release [migration](../../MIGRATION.md#0441-to-0450-subscriptions-authorize-each-release) · [changelog](../../CHANGELOG.md#0450-unreleased)
- **Action needed** — ORM/macros 0.44.1: generated resolver public errors [migration](../../MIGRATION.md#0440-to-0441-generated-resolver-public-errors) · [changelog](../../CHANGELOG.md#0441-unreleased)
- **Action needed** — ORM/macros 0.44.0: authorization and relation partition identities [migration](../../MIGRATION.md#0431-to-0440-authorization-and-relation-partition-identities) · [changelog](../../CHANGELOG.md#0440-unreleased)
- **No action** — Workspace: Test runner migration: owned database fixtures [migration](../../MIGRATION.md#test-runner-migration-owned-database-fixtures)
- **No action** — Storage 0.7.4: narrow SMB directory-stream backport [migration](../../crates/graphql-orm-storage/MIGRATION.md#073-to-074-narrow-smb-directory-stream-backport) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#074---2026-10-15)
- **Action needed** — Storage 0.7.3: corrected S3 credential cache dependency [migration](../../crates/graphql-orm-storage/MIGRATION.md#072-to-073-corrected-s3-credential-cache-dependency) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#073---2026-10-15)
- **Action needed** — Storage 0.7.2: modern S3 transport selection [migration](../../crates/graphql-orm-storage/MIGRATION.md#071-to-072-modern-s3-transport-selection) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#072---2026-10-15)
- **No action** — Storage 0.7.1: local temporary-file sweep [migration](../../crates/graphql-orm-storage/MIGRATION.md#070-to-071-local-temporary-file-sweep) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#071---2026-10-14)
- **Action needed** — Storage 0.7.0: atomic conditional creation [migration](../../crates/graphql-orm-storage/MIGRATION.md#062-to-070-atomic-conditional-creation) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#070---2026-10-14)
- **Action needed** — Backup 0.10.0: explicit buffer and decompression limits [migration](../../crates/graphql-orm-backup/MIGRATION.md#090-to-0100-explicit-buffer-and-decompression-limits) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#0100---2026-10-15)
- **Action needed** — Backup 0.9.0: restore failure progress [migration](../../crates/graphql-orm-backup/MIGRATION.md#080-to-090-restore-failure-progress) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#090---2026-10-15)
- **Action needed** — Backup 0.8.0: fail-closed writer locks [migration](../../crates/graphql-orm-backup/MIGRATION.md#072-to-080-fail-closed-writer-locks) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#080---2026-10-14)
- **Action needed** — Router 0.6.0: coherent fallible authentication clocks [migration](../../crates/graphql-orm-router/MIGRATION.md#054-to-060-coherent-fallible-authentication-clocks) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#060---2026-10-15)

### workspace-2026.10.14.1

Commit `a70d2fb06ae1` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.14.1)

Packages (changed in bold): `graphql-orm` 0.43.1, `graphql-orm-macros` 0.43.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.16.0; **`graphql-orm-ai` 0.107.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.4.

- **No action** — AI 0.107.1: This compatible patch adds `AiAgentContinuation::task_context_state` and the closed `AiAgentContinuationTaskContextState` enum. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01070-to-01071) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01071---2026-10-14)

### workspace-2026.10.13.1

Commit `2e8cb79941a4` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.13.1)

Packages (changed in bold): `graphql-orm` 0.43.1, `graphql-orm-macros` 0.43.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.16.0**; **`graphql-orm-ai` 0.107.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.4.

- **Action needed** — AI tool profiles 0.16.0: This is a source-breaking pre-1.0 minor release: the public `AiToolDescriptor` gains the optional `result_media` field. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0150-to-0160) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0160---2026-10-13)
- **No action** — AI tool profiles: Trusted `ToolPreExecutionRejection` carries an exact invocation hash and one closed public reason without resolver text, credentials or request content. [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#unreleased)
- **Action needed** — AI 0.107.0: This is a source-breaking pre-1.0 minor release under Cargo's compatibility rules. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01061-to-01070) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01070---2026-10-13)
- **No action** — AI: `AiToolAuthorizationDecision::require_authentication` distinguishes current authentication renewal from permission denial without granting execution. [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#unreleased-1)

### workspace-2026.10.12.2

Commit `3dcb51231a69` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.12.2)

Packages (changed in bold): **`graphql-orm` 0.43.1, `graphql-orm-macros` 0.43.1**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; `graphql-orm-ai` 0.106.1; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.4.

- **No action** — ORM/macros 0.43.1: host-supplied integer primary-key binding [migration](../../MIGRATION.md#0430-to-0431-host-supplied-integer-primary-key-binding) · [changelog](../../CHANGELOG.md#0431-unreleased)

### workspace-2026.10.12.1

Commit `25355ddc38a0` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.12.1)

Packages (changed in bold): **`graphql-orm` 0.43.0, `graphql-orm-macros` 0.43.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; `graphql-orm-ai` 0.106.1; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.4.

- **No action** — ORM/macros 0.43.0: opt-in runtime GraphQL mutations [migration](../../MIGRATION.md#0420-to-0430-opt-in-runtime-graphql-mutations) · [changelog](../../CHANGELOG.md#0430-unreleased)
- **No action** — ORM/macros 0.42.0: opt-in runtime GraphQL reads [migration](../../MIGRATION.md#0410-to-0420-opt-in-runtime-graphql-reads) · [changelog](../../CHANGELOG.md#0420-unreleased)
- **No action** — ORM/macros 0.41.0: transactional runtime mutations [migration](../../MIGRATION.md#0400-to-0410-transactional-runtime-mutations) · [changelog](../../CHANGELOG.md#0410---unreleased)
- **No action** — ORM/macros 0.40.0: owned runtime migration targets [migration](../../MIGRATION.md#0390-to-0400-owned-runtime-migration-targets) · [changelog](../../CHANGELOG.md#0400---unreleased)
- **No action** — ORM/macros 0.39.1: Adopt aligned ORM/macros 0.39.1. [migration](../../MIGRATION.md#runtime-string-predicates-and-postgresql-binding-0391) · [changelog](../../CHANGELOG.md#0391---unreleased)

### workspace-2026.10.03.2

Commit `df474c761c40` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.03.2)

Packages (changed in bold): **`graphql-orm` 0.39.0, `graphql-orm-macros` 0.39.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; `graphql-orm-ai` 0.106.1; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.4.

- **No action** — ORM/macros 0.39.0: Adopt aligned ORM/macros 0.39.0. [migration](../../MIGRATION.md#repository-host-timestamps-0390) · [changelog](../../CHANGELOG.md#0390---2026-10-03)

### workspace-2026.10.03.1

Commit `3bff96b90ce1` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.03.1)

Packages (changed in bold): **`graphql-orm` 0.38.0, `graphql-orm-macros` 0.38.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; `graphql-orm-ai` 0.106.1; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.4.

- **No action** — ORM/macros 0.38.0: Adopt aligned ORM/macros 0.38.0. [migration](../../MIGRATION.md#portable-group-pages-and-sql-visible-aggregates-0380) · [changelog](../../CHANGELOG.md#0380---2026-10-03)

### workspace-2026.10.01.3

Commit `809d8946b7a3` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.01.3)

Packages (changed in bold): **`graphql-orm` 0.35.2, `graphql-orm-macros` 0.35.2**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; `graphql-orm-ai` 0.106.1; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.5.4**.

- **No action** — ORM/macros 0.35.2: Adopt aligned runtime/macros 0.35.2. [migration](../../MIGRATION.md#policy-aware-transaction-counts-0352) · [changelog](../../CHANGELOG.md#0352---2026-10-01)
- **Action needed** — ORM/macros 0.35.1: Adopt aligned ORM/macros 0.35.1. [migration](../../MIGRATION.md#policy-aware-typed-projections-0351) · [changelog](../../CHANGELOG.md#0351---2026-10-01)
- **No action** — ORM/macros 0.35.0: Adopt aligned ORM/macros 0.35.0 after owner review/publication. [migration](../../MIGRATION.md#complete-text-group-pages-0350) · [changelog](../../CHANGELOG.md#0350---2026-10-01)
- **Action needed** — ORM/macros 0.33.5: authoritative generated relationship access [migration](../../MIGRATION.md#0334-to-0335-authoritative-generated-relationship-access) · [changelog](../../CHANGELOG.md#0335---2026-10-01)
- **No action** — ORM/macros 0.33.4: generated cross-crate relationships [migration](../../MIGRATION.md#0333-to-0334-generated-cross-crate-relationships) · [changelog](../../CHANGELOG.md#0334---2026-10-01)
- **No action** — Router 0.5.4: Rebuild to retain warnings-denied compilation on current Rust without raising the Rust 1.90 MSRV. [migration](../../crates/graphql-orm-router/MIGRATION.md#053-to-054) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#054---2026-10-01)

### workspace-2026.10.01.2

Commit `37e45fb7f283` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.01.2)

Packages (changed in bold): `graphql-orm` 0.33.3, `graphql-orm-macros` 0.33.3; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; **`graphql-orm-ai` 0.106.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **No action** — AI 0.106.1: Provider-session bindings now admit up to 128 static bootstrap fingerprints; the Grok SDK broker admits up to 128 exact definitions. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01060-to-01061) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01061---2026-10-01)

### workspace-2026.10.01.1

Commit `82503290c0d0` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.01.1)

Packages (changed in bold): **`graphql-orm` 0.33.3, `graphql-orm-macros` 0.33.3**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; **`graphql-orm-ai` 0.106.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **No action** — ORM/macros 0.33.3: repository aggregate enum compatibility [migration](../../MIGRATION.md#0332-to-0333-repository-aggregate-enum-compatibility) · [changelog](../../CHANGELOG.md#0333---2026-10-01)
- **No action** — ORM/macros 0.33.2: migration compatibility fixture release identity [migration](../../MIGRATION.md#0331-to-0332-migration-compatibility-fixture-release-identity) · [changelog](../../CHANGELOG.md#0332---2026-10-01)
- **No action** — AI 0.106.0: Hosts can return `AiToolAuthorizationDecision::require_authentication` for an otherwise authorized request needing fresh authentication. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01052-to-01060)

### workspace-2026.09.30.4

Commit `a90f229da664` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.30.4)

Packages (changed in bold): `graphql-orm` 0.33.1, `graphql-orm-macros` 0.33.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; **`graphql-orm-ai` 0.105.2**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **No action** — AI 0.105.2: The Codex actor now stages a bounded thread target supplied by a startup `warning` before the first response or started notification. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01051-to-01052)

### workspace-2026.09.30.3

Commit `131693bdb4a7` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.30.3)

Packages (changed in bold): `graphql-orm` 0.33.1, `graphql-orm-macros` 0.33.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; **`graphql-orm-ai` 0.105.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **No action** — AI 0.105.1: The Codex protocol actor now admits schema-defined, bounded `warning` notifications while the sole `thread/start` RPC is pending. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01050-to-01051)

### workspace-2026.09.30.2

Commit `a6f079ff29e7` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.30.2)

Packages (changed in bold): `graphql-orm` 0.33.1, `graphql-orm-macros` 0.33.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.15.0; **`graphql-orm-ai` 0.105.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **No action** — AI 0.105.0: `AiToolAuthorizationPolicy` gains a compatible `authorize_request` default method. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01040-to-01050)

### workspace-2026.09.30.1

Commit `d2b19e1730f1` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.30.1)

Packages (changed in bold): `graphql-orm` 0.33.1, `graphql-orm-macros` 0.33.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.15.0**; **`graphql-orm-ai` 0.104.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **No action** — AI tool profiles 0.15.0: Trusted hosts may emit `ToolExecutionError::RejectedBeforeExecution` only after verifying no admission, dispatch, persisted work or application effect for the exact … [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0140-to-0150)
- **No action** — AI 0.104.0: Trusted hosts may emit `ToolExecutionError::RejectedBeforeExecution` only after verifying no admission, dispatch, persisted work or application effect for the exact … [migration](../../crates/graphql-orm-ai/MIGRATION.md#01030-to-01040)

### workspace-2026.09.28.2

Commit `956f1280a078` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.28.2)

Packages (changed in bold): `graphql-orm` 0.33.1, `graphql-orm-macros` 0.33.1; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.14.0; **`graphql-orm-ai` 0.103.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.3.

- **Action needed** — AI 0.103.0: `AiSessionConnection` gains the public `activity_snapshot` field; custom Rust implementations must populate `AiSessionActivitySnapshot`. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01022-to-01030)

### workspace-2026.09.28.1

Commit `48339be0454d` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.28.1)

Packages (changed in bold): **`graphql-orm` 0.33.1, `graphql-orm-macros` 0.33.1**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.14.0; **`graphql-orm-ai` 0.102.2**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.5.3**.

- **No action** — ORM/macros 0.33.1: Federation fixture release identity [migration](../../MIGRATION.md#0330-to-0331-federation-fixture-release-identity) · [changelog](../../CHANGELOG.md#0331---2026-09-28)
- **No action** — AI 0.102.2: This patch corrects test fixtures that assumed generated creation timestamps matched explicit lease timestamps; AI schema module remains `0.68.0`. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01021-to-01022) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01022---2026-09-28)
- **Action needed** — AI 0.102.1: Apply AI schema module `0.68.0` through the normal managed schema workflow. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01020-to-01021) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01021---2026-09-28)
- **No action** — Router 0.5.3: Rebuild the router from the reviewed lockfile to adopt Hive 0.2.19 and the `domain` 0.12.3 security fixes for RUSTSEC-2026-0310. [migration](../../crates/graphql-orm-router/MIGRATION.md#052-to-053) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#053---2026-09-27)

### workspace-2026.09.26.1

Commit `fb0a367d9fab` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.26.1)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.14.0**; **`graphql-orm-ai` 0.102.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI tool profiles 0.14.0: `AiGraphqlToolProfile::automatic_mutation` is an explicit alternative to the supervised mutation constructor. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0131-to-0140) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0140---2026-09-26)
- **Action needed** — AI 0.102.0: Adopt AI tool profiles `0.14.0` from the same reviewed workspace release. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01010-to-01020) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01020---2026-09-26)

### workspace-2026.09.25.2

Commit `997075a3f263` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.25.2)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.13.1; **`graphql-orm-ai` 0.101.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI 0.101.0: Provider and retained-session heartbeat maintenance now continues polling the provider's in-flight persistence while renewal waits. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01002-to-01010) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01010---2026-09-25)

### workspace-2026.09.25.1

Commit `51a1711aef96` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.25.1)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.13.1**; `graphql-orm-ai` 0.100.2; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI tool profiles 0.13.1: The validated upper bound for `AiGraphqlQueryCapabilityLimits::maximum_result_records` is now 100,000. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0130-to-0131)

### workspace-2026.09.24.4

Commit `a094ceeaae9b` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.24.4)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.13.0; **`graphql-orm-ai` 0.100.2**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI 0.100.2: GraphQL SDL, existing stored recovery outcomes and schema module are unchanged. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01001-to-01002) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01002---2026-09-24)

### workspace-2026.09.24.3

Commit `e6b6113a20e9` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.24.3)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.13.0; **`graphql-orm-ai` 0.100.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI 0.100.1: Rust APIs, GraphQL SDL, schema module and stored failure semantics are unchanged. [migration](../../crates/graphql-orm-ai/MIGRATION.md#01000-to-01001) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01001---2026-09-24)

### workspace-2026.09.24.2

Commit `0542f5b6c08b` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.24.2)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.13.0**; **`graphql-orm-ai` 0.100.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI tool profiles 0.13.0: The non-exhaustive `AiError` adds `ProviderExecutionLimit`, `ProviderUsageIncomplete` and `ProviderUsageInvalid`. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0120-to-0130)
- **Action needed** — AI 0.100.0: Adopt AI tool profiles `0.13.0` from the same workspace release. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0991-to-01000) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#01000---2026-09-24)

### workspace-2026.09.24.1

Commit `3b2b18896b4c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.24.1)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.12.0; **`graphql-orm-ai` 0.99.1**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI 0.99.1: Egress audit recording now acquires the backend state-machine transaction before checking existing evidence and retries only classified retryable transactions, up to … [migration](../../crates/graphql-orm-ai/MIGRATION.md#0990-to-0991)

### workspace-2026.09.23.3

Commit `13987e64c43d` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.23.3)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.12.0**; **`graphql-orm-ai` 0.99.0**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI tool profiles 0.12.0: Handle the two additional non-exhaustive `AiError` variants with public codes `AI_SESSION_EXECUTION_UNBOUND` and `AI_SESSION_EXECUTION_UNAVAILABLE`. [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0110-to-0120)
- **Action needed** — AI 0.99.0: Adopt AI `0.99.0` and AI tool profiles `0.12.0` from one reviewed workspace release. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0985-to-0990)

### workspace-2026.09.23.2

Commit `ef6a780cda38` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.23.2)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.11.0; **`graphql-orm-ai` 0.98.5**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI 0.98.5: A durably persisted dynamic-tool result with no model-visible input now advances the responder's lease proof before rejecting disclosure. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0985) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0985---2026-09-23)

### workspace-2026.09.23.1

Commit `b0924bc5eb08` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.23.1)

Packages (changed in bold): `graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.11.0; **`graphql-orm-ai` 0.98.4**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.2.

- **No action** — AI 0.98.4: Inline dynamic and retained providers no longer stall when a heartbeat or live output timer encounters a tool holding the shared run lease. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0984) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0984---2026-09-23)

### workspace-2026.09.22.1

Commit `0278bee62474` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.22.1)

Packages (changed in bold): **`graphql-orm` 0.33.0, `graphql-orm-macros` 0.33.0**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.11.0; **`graphql-orm-ai` 0.98.3**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.5.2**.

- **No action** — ORM/macros 0.33.0: authorization-aware pagination [migration](../../MIGRATION.md#0320-to-0330-authorization-aware-pagination) · [changelog](../../CHANGELOG.md#0330---2026-09-21)
- **Action needed** — ORM/macros 0.32.0: Federation entity keys and a shareable `PageInfo` [migration](../../MIGRATION.md#0311-to-0320-federation-entity-keys-and-a-shareable-pageinfo) · [changelog](../../CHANGELOG.md#0320---2026-09-17)
- **No action** — AI 0.98.3: `provider-grok-acp` is additive and disabled by default. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0983) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0983---2026-09-22)
- **No action** — Router 0.5.2: The release adds test coverage only. [migration](../../crates/graphql-orm-router/MIGRATION.md#051-to-052) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#052---2026-09-17)

### workspace-2026.09.15.3

Commit `f1db17a7da1a` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.15.3)

Packages (changed in bold): **`graphql-orm` 0.31.1, `graphql-orm-macros` 0.31.1**; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.11.0; `graphql-orm-ai` 0.98.2; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.1.

- **No action** — ORM/macros 0.31.1: patched SQL Server XML dependency [migration](../../MIGRATION.md#0310-to-0311-patched-sql-server-xml-dependency) · [changelog](../../CHANGELOG.md#0311---2026-09-15)

### workspace-2026.09.15.2

Commit `e26f213502f7` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.15.2)

Packages (changed in bold): `graphql-orm` 0.31.0, `graphql-orm-macros` 0.31.0; `graphql-orm-operation-catalog` 0.4.0; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; `graphql-orm-ai-tool-profiles` 0.11.0; **`graphql-orm-ai` 0.98.2**; `graphql-orm-router-protocol` 0.2.1; `graphql-orm-router` 0.5.1.

- **No action** — AI 0.98.2: Hosts may install one `Arc<AiRunAuthorization>` with `with_run_authorization` on `OrmAiSessionService` and `OrmAiRunDispositionService`. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0982) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0982---2026-09-15)
- **No action** — AI 0.98.1: Hosts may opt into `AiCapabilityDiscoveryBroker::with_discovery_cache(ttl, maximum_searches)` to reuse discovery candidates across runs of the same principal and AI … [migration](../../crates/graphql-orm-ai/MIGRATION.md#0981) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0981---2026-09-15)

### workspace-2026.09.15.1

Commit `dfe6e1a9f289` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.15.1)

Packages (changed in bold): **`graphql-orm` 0.31.0, `graphql-orm-macros` 0.31.0**; **`graphql-orm-operation-catalog` 0.4.0**; `graphql-orm-storage` 0.6.2; `graphql-orm-backup` 0.7.2; **`graphql-orm-ai-tool-profiles` 0.11.0**; **`graphql-orm-ai` 0.98.0**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.5.1**.

- **No action** — ORM/macros 0.31.0: native SQL Server query notifications [migration](../../MIGRATION.md#0300-to-0310-native-sql-server-query-notifications) · [changelog](../../CHANGELOG.md#0310---2026-09-07)
- **Action needed** — ORM/macros 0.30.0: checked calendar-date filters [migration](../../MIGRATION.md#0290-to-0300-checked-calendar-date-filters) · [changelog](../../CHANGELOG.md#0300---2026-08-31)
- **See changelog** — Operation catalogue 0.4.0: no dedicated section. [changelog](../../CHANGELOG.md)
- **Action needed** — AI tool profiles 0.11.0: typed result-budget failures [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0104-to-0110-typed-result-budget-failures) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0110---2026-09-03)
- **No action** — AI tool profiles 0.10.4: resolver-first semantic discovery ranking [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0103-to-0104-resolver-first-semantic-discovery-ranking) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0104---2026-09-02)
- **Action needed** — AI tool profiles 0.10.3: proof-bearing pre-dispatch provider failure [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0102-to-0103-proof-bearing-pre-dispatch-provider-failure) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0103---2026-09-01)
- **Action needed** — AI 0.98.0: Conversation bootstrap now includes nullable `AiConversationRunSummary::failure_disposition` (`FailureDisposition` with Pascal naming), using the existing … [migration](../../crates/graphql-orm-ai/MIGRATION.md#0980) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0980---2026-09-15)
- **No action** — AI 0.97.2: Codex 0.154 `deprecationNotice` notifications are admitted only while the exact retained `thread/resume` response is pending. [migration](../../crates/graphql-orm-ai/MIGRATION.md#0972) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0972---2026-09-10)
- **No action** — AI 0.97.1: preserve remote tool authorization failures [migration](../../crates/graphql-orm-ai/MIGRATION.md#0970-to-0971-preserve-remote-tool-authorization-failures) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0971---2026-09-04)
- **Action needed** — AI 0.97.0: recoverable application-tool result budgets [migration](../../crates/graphql-orm-ai/MIGRATION.md#0962-to-0970-recoverable-application-tool-result-budgets) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0970---2026-09-03)
- **No action** — AI 0.96.2: schema-aligned fixed-broker execution wrappers [migration](../../crates/graphql-orm-ai/MIGRATION.md#0961-to-0962-schema-aligned-fixed-broker-execution-wrappers) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0962---2026-09-02)
- **No action** — AI 0.96.1: bounded assistant-item lifecycle continuation [migration](../../crates/graphql-orm-ai/MIGRATION.md#0960-to-0961-bounded-assistant-item-lifecycle-continuation) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0961---2026-09-02)
- **Action needed** — AI 0.96.0: retained-plan recovery and no-dispatch retry proof [migration](../../crates/graphql-orm-ai/MIGRATION.md#09514-to-0960-retained-plan-recovery-and-no-dispatch-retry-proof) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0960---2026-09-02)
- **Action needed** — AI 0.95.14: retained-resume warning interleaving [migration](../../crates/graphql-orm-ai/MIGRATION.md#09513-to-09514-retained-resume-warning-interleaving) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#09514---2026-09-01)
- **No action** — AI 0.95.13: retained web-search adoption on a fresh process [migration](../../crates/graphql-orm-ai/MIGRATION.md#09512-to-09513-retained-web-search-adoption-on-a-fresh-process) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#09513---2026-09-01)
- **Action needed** — AI 0.95.12: retained-resume readiness and pre-dispatch proof [migration](../../crates/graphql-orm-ai/MIGRATION.md#09511-to-09512-retained-resume-readiness-and-pre-dispatch-proof) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#09512---2026-09-01)
- **No action** — AI 0.95.11: absence-proven descriptor replacement [migration](../../crates/graphql-orm-ai/MIGRATION.md#09510-to-09511-absence-proven-descriptor-replacement) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#09511---2026-09-01)
- **No action** — Router 0.5.1: Rebuild consumers at the reviewed 0.5.1 monorepo revision and run their authenticated WebSocket acceptance tests. [migration](../../crates/graphql-orm-router/MIGRATION.md#050-to-051) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#051---2026-09-02)
- **Action needed** — Router: Workspace release dependency alignment [migration](../../crates/graphql-orm-router/MIGRATION.md#workspace-release-dependency-alignment)

### workspace-2026.08.31.1

Commit `7114dbf37352` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.08.31.1)

Packages (changed in bold): **`graphql-orm` 0.29.0, `graphql-orm-macros` 0.29.0**; `graphql-orm-operation-catalog` 0.3.0; **`graphql-orm-storage` 0.6.2**; **`graphql-orm-backup` 0.7.2**; **`graphql-orm-ai-tool-profiles` 0.10.2**; **`graphql-orm-ai` 0.95.10**; `graphql-orm-router-protocol` 0.2.1; **`graphql-orm-router` 0.5.0**.

- **Action needed** — ORM/macros 0.29.0: computed/relation ordering and complex relation composition [migration](../../MIGRATION.md#0270-to-0290-computedrelation-ordering-and-complex-relation-composition) · [changelog](../../CHANGELOG.md#0290---2026-08-31)
- **No action** — ORM/macros 0.27.0: conditional polymorphic relationships [migration](../../MIGRATION.md#0260-to-0270-conditional-polymorphic-relationships) · [changelog](../../CHANGELOG.md#0270---2026-08-26)
- **Action needed** — ORM/macros 0.26.0: agql-auth 0.18.0 alignment [migration](../../MIGRATION.md#0251-to-0260-agql-auth-0180-alignment) · [changelog](../../CHANGELOG.md#0260---2026-08-22)
- **Action needed** — ORM/macros 0.25.1: agql-auth 0.17.1 alignment [migration](../../MIGRATION.md#0250-to-0251-agql-auth-0171-alignment) · [changelog](../../CHANGELOG.md#0251---2026-08-22)
- **Action needed** — ORM/macros 0.25.0: agql-auth 0.17 alignment [migration](../../MIGRATION.md#0240-to-0250-agql-auth-017-alignment) · [changelog](../../CHANGELOG.md#0250---2026-08-22)
- **Action needed** — ORM/macros 0.24.0: agql-auth 0.16 alignment [migration](../../MIGRATION.md#0230-to-0240-agql-auth-016-alignment) · [changelog](../../CHANGELOG.md#0240---2026-08-22)
- **No action** — Storage 0.6.2: The public API, provider configuration, storage-key layout, persisted metadata, and feature defaults are unchanged. [migration](../../crates/graphql-orm-storage/MIGRATION.md#061-to-062) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#062---2026-08-31)
- **No action** — Backup 0.7.2: The public API, snapshot manifest, repository-key layout, restore preflight, persisted data, and feature defaults are unchanged. [migration](../../crates/graphql-orm-backup/MIGRATION.md#071-to-072) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#072---2026-08-31)
- **No action** — AI tool profiles 0.10.2: bounded relationship-argument projection [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0101-to-0102-bounded-relationship-argument-projection) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0102---2026-08-26)
- **Action needed** — AI tool profiles 0.10.1: provider-sized selection allow-lists [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0100-to-0101-provider-sized-selection-allow-lists) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0101---2026-08-26)
- **Action needed** — AI tool profiles 0.10.0: shape-aware discovery and record-cost metadata [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#090-to-0100-shape-aware-discovery-and-record-cost-metadata) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#0100---2026-08-23)
- **Action needed** — AI tool profiles 0.9.0: proof-bearing stateless native-item refusal [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#080-to-090-proof-bearing-stateless-native-item-refusal) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#090---2026-08-22)
- **Action needed** — AI tool profiles 0.8.0: canonical federated capability-index sets [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#070-to-080-canonical-federated-capability-index-sets) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#080---2026-08-21)
- **Action needed** — AI tool profiles 0.7.0: proof-bearing pre-transport budget denial [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#060-to-070-proof-bearing-pre-transport-budget-denial) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#070---2026-08-21)
- **No action** — AI 0.95.10: stable Codex cleanup namespace [migration](../../crates/graphql-orm-ai/MIGRATION.md#0959-to-09510-stable-codex-cleanup-namespace) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#09510---2026-08-28)
- **No action** — AI 0.95.9: provider-native retained-thread absence proof [migration](../../crates/graphql-orm-ai/MIGRATION.md#0958-to-0959-provider-native-retained-thread-absence-proof) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0959---2026-08-28)
- **No action** — AI 0.95.8: detached retained-session cleanup [migration](../../crates/graphql-orm-ai/MIGRATION.md#0957-to-0958-detached-retained-session-cleanup) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0958---2026-08-28)
- **No action** — AI 0.95.7: current-principal retained-session rotation [migration](../../crates/graphql-orm-ai/MIGRATION.md#0956-to-0957-current-principal-retained-session-rotation) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0957---2026-08-27)
- **No action** — AI 0.95.6: incomplete Codex web-search result metadata [migration](../../crates/graphql-orm-ai/MIGRATION.md#0955-to-0956-incomplete-codex-web-search-result-metadata) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0956---2026-08-27)
- **No action** — AI 0.95.5: schema-compatible Codex notification envelopes [migration](../../crates/graphql-orm-ai/MIGRATION.md#0954-to-0955-schema-compatible-codex-notification-envelopes) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0955---2026-08-26)
- **No action** — AI 0.95.4: schema-compatible Codex runtime warnings [migration](../../crates/graphql-orm-ai/MIGRATION.md#0953-to-0954-schema-compatible-codex-runtime-warnings) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0954---2026-08-26)
- **No action** — AI 0.95.3: bounded relationship-argument projection [migration](../../crates/graphql-orm-ai/MIGRATION.md#0952-to-0953-bounded-relationship-argument-projection) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0953---2026-08-26)
- **No action** — AI 0.95.2: compact closed selection allowlists [migration](../../crates/graphql-orm-ai/MIGRATION.md#0951-to-0952-compact-closed-selection-allowlists) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0952---2026-08-26)
- **No action** — AI 0.95.1: forward-compatible Codex web-search completion [migration](../../crates/graphql-orm-ai/MIGRATION.md#0950-to-0951-forward-compatible-codex-web-search-completion) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0951---2026-08-25)
- **No action** — AI 0.95.0: retained dynamic tools with governed built-ins [migration](../../crates/graphql-orm-ai/MIGRATION.md#0940-to-0950-retained-dynamic-tools-with-governed-built-ins) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0950---2026-08-25)
- **Action needed** — AI 0.94.0: shape-aware capability discovery [migration](../../crates/graphql-orm-ai/MIGRATION.md#0932-to-0940-shape-aware-capability-discovery) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0940---2026-08-23)
- **No action** — AI 0.93.2: consistent current-policy browser preview gates [migration](../../crates/graphql-orm-ai/MIGRATION.md#0931-to-0932-consistent-current-policy-browser-preview-gates) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0932---2026-08-23)
- **No action** — AI 0.93.1: public web-search configuration export [migration](../../crates/graphql-orm-ai/MIGRATION.md#0930-to-0931-public-web-search-configuration-export) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0931---2026-08-23)
- **No action** — AI 0.93.0: opt-in Codex app-server native web search [migration](../../crates/graphql-orm-ai/MIGRATION.md#0920-to-0930-opt-in-codex-app-server-native-web-search) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0930---2026-08-23)
- **Action needed** — AI 0.92.0: owner-authorized tool arguments and safe failures [migration](../../crates/graphql-orm-ai/MIGRATION.md#0911-to-0920-owner-authorized-tool-arguments-and-safe-failures) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0920---2026-08-23)
- **Action needed** — AI 0.91.1: Codex FixedBroker projection and retained session admission [migration](../../crates/graphql-orm-ai/MIGRATION.md#0910-to-0911-codex-fixedbroker-projection-and-retained-session-admission) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0911---2026-08-23)
- **Action needed** — AI 0.91.0: agql-auth 0.18.0 type-universe alignment [migration](../../crates/graphql-orm-ai/MIGRATION.md#0901-to-0910-agql-auth-0180-type-universe-alignment) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0910---2026-08-22)
- **Action needed** — AI 0.90.1: agql-auth 0.17.1 type-universe alignment [migration](../../crates/graphql-orm-ai/MIGRATION.md#0900-to-0901-agql-auth-0171-type-universe-alignment) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0901---2026-08-22)
- **Action needed** — AI 0.90.0: agql-auth 0.17 type-universe alignment [migration](../../crates/graphql-orm-ai/MIGRATION.md#0890-to-0900-agql-auth-017-type-universe-alignment) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0900---2026-08-22)
- **Action needed** — AI 0.89.0: agql-auth 0.16 type-universe alignment [migration](../../crates/graphql-orm-ai/MIGRATION.md#0880-to-0890-agql-auth-016-type-universe-alignment) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0890---2026-08-22)
- **Action needed** — AI 0.88.0: direct GPT-5.6 dynamic tools on Codex 0.148.0 [migration](../../crates/graphql-orm-ai/MIGRATION.md#0870-to-0880-direct-gpt-56-dynamic-tools-on-codex-01480) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0880---2026-08-22)
- **Action needed** — AI 0.87.0: retained dynamic-tool readiness input [migration](../../crates/graphql-orm-ai/MIGRATION.md#0860-to-0870-retained-dynamic-tool-readiness-input) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0870---2026-08-22)
- **No action** — AI 0.86.0: metered stateless native-item refusal [migration](../../crates/graphql-orm-ai/MIGRATION.md#0850-to-0860-metered-stateless-native-item-refusal) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0860---2026-08-22)
- **Action needed** — AI 0.85.0: executable federated bounded capability delivery [migration](../../crates/graphql-orm-ai/MIGRATION.md#0840-to-0850-executable-federated-bounded-capability-delivery) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0850---2026-08-21)
- **Action needed** — AI 0.84.0: budget reclamation and pre-transport denial (schema 0.62.0 to 0.63.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0830-to-0840-budget-reclamation-and-pre-transport-denial-schema-0620-to-0630) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0840---2026-08-21)
- **Action needed** — AI 0.83.0: settled retained Codex interruption [migration](../../crates/graphql-orm-ai/MIGRATION.md#0820-to-0830-settled-retained-codex-interruption) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0830---2026-08-21)
- **Action needed** — AI 0.82.0: session reliability and failure disposition [migration](../../crates/graphql-orm-ai/MIGRATION.md#0810-to-0820-session-reliability-and-failure-disposition) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0820---2026-08-20)
- **Action needed** — Router 0.5.0: Align direct `agql-auth` consumers to 0.19.0 at reviewed merged revision `1d2e9fe2e1576105212a7b340a11abf8cad0382d` so the process resolves one matcher type universe. [migration](../../crates/graphql-orm-router/MIGRATION.md#040-to-050) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#050---2026-08-24)
- **Action needed** — Router 0.4.0: Align direct `agql-auth` consumers to 0.18.0 at merged revision `527d15d28e3c295a6f6b5e6d74559a7aecdc1322`. [migration](../../crates/graphql-orm-router/MIGRATION.md#031-to-040) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#040---2026-08-22)
- **Action needed** — Router 0.3.1: Align direct `agql-auth` consumers to 0.17.1 at reviewed full revision `b792241b7d9bb46bad81dde4534ae5b39857f614` and rebuild the router from the same reviewed monorepo … [migration](../../crates/graphql-orm-router/MIGRATION.md#030-to-031) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#031---2026-08-22)
- **Action needed** — Router 0.3.0: Align direct `agql-auth` consumers to 0.17.0 at the reviewed full revision. [migration](../../crates/graphql-orm-router/MIGRATION.md#020-to-030) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#030---2026-08-22)
- **Action needed** — Router 0.2.0: Replace the reviewed full-revision pin and rebuild the router. [migration](../../crates/graphql-orm-router/MIGRATION.md#014-to-020) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#020---2026-08-22)

### workspace-2026.08.20.1

Commit `6ba2d108a91e` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.08.20.1)

Packages (changed in bold): **`graphql-orm` 0.23.0, `graphql-orm-macros` 0.23.0**; **`graphql-orm-operation-catalog` 0.3.0**; **`graphql-orm-storage` 0.6.1**; **`graphql-orm-backup` 0.7.1**; **`graphql-orm-ai-tool-profiles` 0.6.0**; **`graphql-orm-ai` 0.81.0**; **`graphql-orm-router-protocol` 0.2.1**; **`graphql-orm-router` 0.1.4**.

- **Action needed** — ORM/macros 0.23.0: explicit collection-bound semantics [migration](../../MIGRATION.md#0221-to-0230-explicit-collection-bound-semantics) · [changelog](../../CHANGELOG.md#0230---2026-08-14)
- **Action needed** — ORM/macros 0.22.1: relationship semantic argument correction [migration](../../MIGRATION.md#0220-to-0221-relationship-semantic-argument-correction) · [changelog](../../CHANGELOG.md#0221---2026-08-13)
- **Action needed** — ORM/macros 0.22.0: Update `graphql-orm` and `graphql-orm-macros` together to 0.22.0 at one reviewed full Git revision. [migration](../../MIGRATION.md#0220-canonical-graphql-semantic-catalogue) · [changelog](../../CHANGELOG.md#0220---2026-08-13)
- **Action needed** — ORM/macros 0.21.1: Update `graphql-orm` and `graphql-orm-macros` together to 0.21.1 at one reviewed full Git revision. [migration](../../MIGRATION.md#0211-agql-auth-015-session-bound-delegation-alignment) · [changelog](../../CHANGELOG.md#0211---2026-08-13)
- **See changelog** — Operation catalogue 0.3.0: no dedicated section. [changelog](../../CHANGELOG.md)
- **No action** — Storage 0.6.1: The public API, provider configuration, storage-key layout, and persisted metadata are unchanged. [migration](../../crates/graphql-orm-storage/MIGRATION.md#060-to-061) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#061---2026-08-13)
- **No action** — Backup 0.7.1: The public API, snapshot manifest, repository-key layout, restore preflight, and persisted data are unchanged. [migration](../../crates/graphql-orm-backup/MIGRATION.md#070-to-071) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#071---2026-08-13)
- **Action needed** — AI tool profiles 0.6.0: compact discovery and query-plan wire v3 [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#050-to-060-compact-discovery-and-query-plan-wire-v3) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#060---2026-08-16)
- **Action needed** — AI tool profiles 0.5.0: explicit collection-bound query plans [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#041-to-050-explicit-collection-bound-query-plans) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#050---2026-08-14)
- **Action needed** — AI tool profiles 0.4.1: exact generated relationship arguments [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#040-to-041-exact-generated-relationship-arguments) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#041---2026-08-13)
- **No action** — AI tool profiles 0.4.0: semantic query and bounded subscription capabilities [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#030-to-040-semantic-query-and-bounded-subscription-capabilities) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#040---2026-08-13)
- **Action needed** — AI 0.81.0: capability discovery and durable provider loops [migration](../../crates/graphql-orm-ai/MIGRATION.md#0800-to-0810-capability-discovery-and-durable-provider-loops) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0810---2026-08-16)
- **Action needed** — AI 0.80.0: model reasoning effort (schema 0.59.0 to 0.60.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0800-model-reasoning-effort-schema-0590-to-0600) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0800---2026-08-15)
- **No action** — AI 0.79.0: safe tool failures and provider-session deferral (schema remains 0.59.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0790-safe-tool-failures-and-provider-session-deferral-schema-remains-0590) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0790---2026-08-14)
- **No action** — AI 0.78.5: typed newly-bound Codex turn rejection (schema remains 0.59.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0785-typed-newly-bound-codex-turn-rejection-schema-remains-0590) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0785---2026-08-14)
- **No action** — AI 0.78.4: Codex omitted-required object projection (schema remains 0.59.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0784-codex-omitted-required-object-projection-schema-remains-0590) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0784---2026-08-14)
- **No action** — AI 0.78.3: exact remote read-capability delegation (schema remains 0.59.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0783-exact-remote-read-capability-delegation-schema-remains-0590) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0783---2026-08-14)
- **No action** — AI 0.78.2: mixed static and generated read plans (schema remains 0.59.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0782-mixed-static-and-generated-read-plans-schema-remains-0590) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0782---2026-08-13)
- **Action needed** — AI 0.78.1: atomic retained approval waits (schema 0.58.0 to 0.59.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0781-atomic-retained-approval-waits-schema-0580-to-0590) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0781---2026-08-13)
- **No action** — AI 0.78.0: retention traversal under bounded ORM pagination (schema remains 0.58.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0780-retention-traversal-under-bounded-orm-pagination-schema-remains-0580) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0780---2026-08-13)
- **Action needed** — AI 0.78.0: durable bounded subscription waits (schema 0.57.0 to 0.58.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0780-durable-bounded-subscription-waits-schema-0570-to-0580) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0780---2026-08-13)
- **Action needed** — AI 0.78.0: parked provider sessions across durable waits (schema 0.58.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0780-parked-provider-sessions-across-durable-waits-schema-0580) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0780---2026-08-13)
- **No action** — AI 0.78.0: generated GraphQL query and mutation capabilities (schema remains 0.58.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0780-generated-graphql-query-and-mutation-capabilities-schema-remains-0580) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0780---2026-08-13)
- **No action** — AI 0.77.1: independent provider feature builds (schema remains 0.57.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0771-independent-provider-feature-builds-schema-remains-0570) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0771---2026-08-13)
- **Action needed** — AI 0.77.0: absence-proven provider-session rebind (schema 0.56.0 to 0.57.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0770-absence-proven-provider-session-rebind-schema-0560-to-0570) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0770---2026-08-13)
- **Action needed** — AI 0.76.1: agql-auth 0.15 session-bound delegation alignment (schema remains 0.56.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0761-agql-auth-015-session-bound-delegation-alignment-schema-remains-0560) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0761---2026-08-13)
- **Action needed** — AI 0.76.0: authoritative durable run terminal events (schema 0.55.0 to 0.56.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0760-authoritative-durable-run-terminal-events-schema-0550-to-0560) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0760---2026-08-12)
- **No action** — AI 0.75.1: complete durable event replay at the ORM page limit (schema remains 0.55.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0751-complete-durable-event-replay-at-the-orm-page-limit-schema-remains-0550) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0751---2026-08-12)
- **No action** — Router protocol 0.2.1: The wire format, required-semantics behavior, public data contracts, and protocol major remain unchanged. [migration](../../crates/graphql-orm-router-protocol/MIGRATION.md#crate-020-to-021-protocol-major-remains-1) · [changelog](../../crates/graphql-orm-router-protocol/CHANGELOG.md#021---2026-08-13)
- **Action needed** — Router 0.1.4: Replace the reviewed full-revision pin and rebuild the router. [migration](../../crates/graphql-orm-router/MIGRATION.md#013-to-014) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#014---2026-08-13)

### workspace-2026.08.12.1

Commit `fac98d99e64c` · [release notes](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.08.12.1)

Packages (changed in bold): **`graphql-orm` 0.21.0, `graphql-orm-macros` 0.21.0**; **`graphql-orm-operation-catalog` 0.1.0**; **`graphql-orm-storage` 0.6.0**; **`graphql-orm-backup` 0.7.0**; **`graphql-orm-ai-tool-profiles` 0.3.0**; **`graphql-orm-ai` 0.75.0**; **`graphql-orm-router-protocol` 0.2.0**; **`graphql-orm-router` 0.1.3**.

- **Action needed** — ORM/macros 0.21.0: Update `graphql-orm` and `graphql-orm-macros` together to 0.21.0 at one reviewed full Git revision. [migration](../../MIGRATION.md#0210-backend-neutral-operation-catalog-and-companion-derives) · [changelog](../../CHANGELOG.md#0210---2026-08-11)
- **See changelog** — Operation catalogue 0.1.0: no dedicated section. [changelog](../../CHANGELOG.md)
- **Action needed** — Storage 0.6.0: native SMB consumers update the crate version; the storage trait contracts are unchanged [migration](../../crates/graphql-orm-storage/MIGRATION.md#050-to-060) · [changelog](../../crates/graphql-orm-storage/CHANGELOG.md#060)
- **Action needed** — Backup 0.7.0: repository consolidation [migration](../../crates/graphql-orm-backup/MIGRATION.md#060-to-070-repository-consolidation) · [changelog](../../crates/graphql-orm-backup/CHANGELOG.md#070)
- **Action needed** — AI tool profiles 0.3.0: canonical JSON fingerprints [migration](../../crates/graphql-orm-ai-tool-profiles/MIGRATION.md#020-to-030-canonical-json-fingerprints) · [changelog](../../crates/graphql-orm-ai-tool-profiles/CHANGELOG.md#030---2026-08-11)
- **Action needed** — AI 0.75.0: canonical Codex tools and retained bootstrap (schema remains 0.55.0) [migration](../../crates/graphql-orm-ai/MIGRATION.md#0750-canonical-codex-tools-and-retained-bootstrap-schema-remains-0550) · [changelog](../../crates/graphql-orm-ai/CHANGELOG.md#0750---2026-08-12)
- **Action needed** — Router protocol 0.2.0: `SubgraphDescriptor` adds an optional `extensions` vector. [migration](../../crates/graphql-orm-router-protocol/MIGRATION.md#crate-010-to-020-protocol-major-remains-1) · [changelog](../../crates/graphql-orm-router-protocol/CHANGELOG.md#020---2026-08-11)
- **Action needed** — Router 0.1.3: Replace the reviewed full-revision pin and rebuild the router. [migration](../../crates/graphql-orm-router/MIGRATION.md#012-to-013) · [changelog](../../crates/graphql-orm-router/CHANGELOG.md#013---2026-08-11)
