---
title: Upgrade through the security remediation release
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-15
review_by: 2027-02-01
supersedes: []
---

# Upgrade through the security remediation release

Use this sequence when upgrading an application that consumes the ORM and any
authentication, storage or backup companions. The package migration guides own
the exact API changes; this guide coordinates the order across packages.

## Prepare one coherent dependency graph

Choose a reviewed workspace release and retain its release manifest. Use that
same workspace tag for runtime, macros and companion dependencies, and retain
the manifest's full source commit in the application's reviewed pin record.
Read the [generated inventory](../reference/workspace-packages.md), the
[ORM migration guide](../../MIGRATION.md), the
[storage migration guide](../../crates/graphql-orm-storage/MIGRATION.md) and the
[backup migration guide](../../crates/graphql-orm-backup/MIGRATION.md).
The manifest records the exact external authentication revision; use that same
source identity in hosts that depend on its principal types directly. Read the
authentication repository's migration guide at that revision before deploying
new store implementations. Regenerate the application lockfile and inspect its
selected backend/provider graph. Do not enable all alternative database features
as a validation shortcut.

Prepare each application's actual deployment image and backend separately.
MSSQL images now need platform TLS dependencies; configure full-session
encryption and verified server identity according to the
[SQL Server guide](../reference/graphql-orm/mssql.md). Validate private CA and
hostname configuration against an owned test server before rollout.

For owned PostgreSQL String defaults and actual default changes, follow the
[ORM migration guide](../../MIGRATION.md#0480-to-0481-postgresql-string-default-round-trips)
and its subsequent default-alteration/unsupported-change sections. Review actual
physical state and guarded repair versions; do not rewrite migration history to
hide an ineffective earlier alteration.

The updated S3 SDK requires Rust 1.94.1 or later for S3-enabled builds; validate
the application's selected provider toolchain independently of router-only MSRV
checks. The storage migration guide records the owning dependency transition.

Hosts implementing the router's `AuthenticationProvider` must also migrate
custom `current_time` overrides to return `Result<SystemTime, AuthenticationError>`.
Observe the same clock used for validation and fail closed when it is unavailable
or cannot be represented; retain the default method for an ordinary system clock.
The agql adapter obtains this observation directly from its validator. Follow the
[router migration guide](../../crates/graphql-orm-router/MIGRATION.md) for the
0.6 API change and connection-expiry contract.

## Coordinate stateful transitions

Stop and drain all old backup writers before admitting upgraded writers to a
shared repository. Follow the backup guide's explicit offline lock recovery
procedure; elapsed time alone never establishes that recovery is safe. Custom
repositories and storage providers must implement atomic creation and
single-attempt lock deletion before use. Mixed old/new lock clients are unsafe.

Protect local or mounted storage roots and every ancestor against replacement by
untrusted actors. The local sweep skips links; ordinary reads and writes still
require an exclusively trusted filesystem tree. Applications facing hostile
local path replacement need a different confinement mechanism.

Migrate durable refresh records to persist `grant_source`, `roles` and
`authorization_roles` transactionally. Historical `Legacy` records cannot
refresh and require reauthentication; never label all historical records as
password-authorized to bypass that boundary. Host/OIDC/host-verified issuance
requires an explicit `RefreshGrantAuthority` for refresh, preserving both the
original grant and current-authority ceilings. Missing authority denies refresh.

For authentication challenge-store upgrades, quiesce old workers and invalidate
outstanding challenges as required by the authentication migration guide.
Implement atomic attempt reservation and consume semantics in durable adapters;
read/modify/write emulation does not preserve the admission bound. Configure
bounded abuse-state capacity and host admission for the workload. The built-in
password worker service has a fixed process-wide bound of two workers and 32
queued jobs; size host admission and overload responses around that bound.
Treat `AuthServiceUnavailable` from saturation as temporary unavailability and
fail closed; do not interpret it as invalid credentials or bypass verification.

Invalidate external caches or persisted cursors derived from the old
authorization-partition/relation-key encodings. Existing ordinary database rows
do not need rewriting solely for those key changes. Back up application state
before applying any separate application schema migration.

## Install application policy before exposing operations

Authenticate at the trusted transport, then inject one coherent accepted-request
bundle per request or authenticated connection. Follow the
[authentication bridge guide](../reference/graphql-orm/agql-auth-bridge.md) to
provide authoritative current-user/grant and token-status checks, a trusted
clock, binding authorization and the application's business policy. Sensitive
operations require fresh checks at every decision; an always-active checker or
stale cached grants cannot satisfy immediate revocation.
These checks establish authority at decision time; they do not atomically fence
a database write against revocation racing that write. Where that stronger
guarantee is required, the host must coordinate the authority version and write
within an appropriate transaction or equivalent serialized boundary.

For AI tools, update the host's `GraphqlRequestContextFactory` and authenticated
executor to construct that same envelope from the full freshly resolved
principal. AI tool approval confirms intent; it does not replace current
authentication or ORM authorization at execution. Persisted runs and approval
records must not restore an old user's grants into a new request. Follow the
[AI GraphQL execution contract](../../crates/graphql-orm-ai/docs/remote-graphql-execution.md)
for remote operation and credential bindings, and rehearse revocation between
approval and execution in the application's adapter.

Install entity, row and field policies for the exposed schema. Use the
[strict authorization guide](../reference/graphql-orm/strict-authorization.md)
to identify missing declarations and providers, including custom operations.
The default declared-policy mode does not create tenant isolation or authorize
unmarked host code. Raw pools, low-level repository/query APIs and custom
resolvers retain explicit host enforcement obligations.

For runtime consumers, adopt [checked complete-key reads](../reference/graphql-orm/runtime-queries.md#checked-complete-key-reads)
with current host authority and a required row restriction, and
[checked original filter intent/offline matching](../reference/graphql-orm/runtime-queries.md#checked-original-intent-and-offline-matching)
only within its documented host-attested storage/visibility premises. These APIs
do not authorize themselves; dependency lists or offline matches do not replace
current policy or native RLS. The owning migration/reference documents define
the exact compatibility and limit contracts.

Review subscriptions as a separate release boundary. Authorization is repeated
for each event; unsupported nested/custom selections fail closed. Deleted
preimages require the explicit default-deny row-policy hook described in the
[runtime and writes reference](../reference/graphql-orm/runtime-and-writes.md).
Do not turn that hook on merely to recover old event delivery: it must establish
the required current authority even when the database row no longer exists.
Reconnect with a fresh accepted credential after authority changes.

Configure finite protected-scan budgets and application page limits before
exposing residual callback or fallback-search collections. Exercise budget
exhaustion and treat it as an error, without presenting partial totals as exact.
Complete native SQL authorization paths retain their documented pagination
contract. Review the [pagination migration guide](../reference/graphql-orm/pagination-migration.md)
for those separate modes.

## Rehearse backup and restore within explicit limits

Choose finite buffer, decoded-payload and stage limits that accommodate the
application's legitimate snapshots. Confirm custom providers, object indexes and
sinks implement native streaming where needed. The limits bound the specified
collection/decoding boundaries; they are not a whole-process memory guarantee.
Update exhaustive error matches for typed capacity and restore-progress errors.

Restore into disposable staging targets and an isolated object namespace.
Do not publish them until database and object restore plus verification succeed.
A later stage can fail after earlier stages committed, and object checksum
failure can follow sink writes. Progress is diagnostic evidence, not permission
to resume blindly into a dirty target. Historical object-size metadata is
normalized from verified bytes without rewriting the original manifest.

## Accept the application rollout

Run the application's selected backend/provider checks with synthetic fixtures.
Include missing context, removed privileges, revoked sessions, cross-user
relations, subscription delivery, concurrent identities, bounded-scan exhaustion,
mutation rollback and interrupted restore. Test custom adapters and policy
callbacks in addition to library defaults. Repeat the dependency advisory review
for the application's unified feature graph; the workspace's scoped
[dependency dispositions](../reference/dependency-security-disposition.md) do not
cover unrelated host APIs or enabled upstream features.

Use a staged rollout with an application-specific rollback plan. Restoring an
older binary can reintroduce the corrected authorization or lock behavior;
coordinate traffic and stateful workers before any rollback. Library test
results do not certify an application's policy configuration or data migration.
