---
title: "Backend and capability acceptance matrix"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-01
supersedes: []
---

# Backend and capability acceptance matrix

This matrix records the Slice 7 acceptance boundary. A compile result is not a
production claim, and a crate-level test does not replace consumer schema,
authorization, migration, restore, or deployment acceptance.

## Backend status

| Backend/profile | Evidence in this repository | Current claim |
| --- | --- | --- |
| SQLite | Complete in-memory generated-ORM unit/integration matrix, prior-to-current schema migration, concurrency/fence/recovery/security tests, provider-profile tests, GraphQL naming, Clippy, and Rustdoc. | Supported for the implemented capability set in test-owned or deployment-reviewed SQLite stores. Applied full backup/restore is still closed. |
| PostgreSQL | Compile profiles plus an owned disposable Docker parity test. The harness generates its own container, credentials, port, and database; it never consumes `DATABASE_URL` or `TEST_DATABASE_URL`. It covers migration and representative generated-ORM concurrency/state-machine behavior. | Supported for the implemented capability set only after the consumer repeats schema composition, migration, RLS/authorization, restore, and deployment tests. Applied full backup/restore is still closed. |
| SQL Server | The owned lane shares session, run fencing and concurrency, webhook, durable subscription replay, checkpoint, compaction, and restore reconciliation scenarios with PostgreSQL. | Durable runtime services on an externally provisioned, reviewed schema. SQL Server-specific boundaries are below. |

## Capability status

| Capability | Implemented here | Host/consumer proof still required | Closed or unsupported |
| --- | --- | --- | --- |
| Session/message runtime and protected storage | Generated-ORM persistence, authorization bridge, content protection, fencing, bounded streaming/checkpoints, retention workers. | Schema composition, encryption/key deployment, migration and restore rehearsal, access-policy parity. | Runtime readiness after full restore remains closed until applied reconciliation succeeds. |
| Native/provider adapters | OpenAI, Anthropic, xAI, Ollama, reviewed OpenAI-compatible profiles, trusted JSONL local harness, and strict Codex app-server process/retained-thread interface with a default-off, factory-attested dynamic-tools-only profile for reviewed direct-tool models, registration-bound static bootstrap instructions, and canonical catalog-to-Codex tool-schema projection. | Real credentials, endpoint/network policy, provider account retention/residency settings, OS/container sandbox and process-tree kill implementation, exact executable/model-catalogue review, compile-time static bootstrap policy, experimental app-server compatibility tests, live opt-in tests. | Generic endpoint/model/JSON-RPC authority, arbitrary child-process launch, request-local retained developer instructions, host-authored schema rewriting, Code Mode as a dynamic-tool workaround, and adapter-owned application-tool execution are unsupported. |
| Provider sessions and activity | Private protected provider-session binding; strict Codex create/resume/interrupt/delete transport; absence-proven fenced rebind after cleanup; ordered protected text/visible-summary/hosted-tool/citation activity; exact cleanup and restore audit. | Canonical host transcript/policy fingerprint, managed cleanup/rebind lifecycle, reviewed Codex process actor and sandbox, portable-backup drain. | Warm-process state is never persisted; restored cursors never auto-resume; expiry is not absence proof; multiplexing and portable cursor restore are unsupported. |
| OpenAI background runs | Exact submission, retrieval, webhook receipt, retry/deadline handling, terminal usage/budget/output reconciliation. | Worker scheduling/operations and provider-account configuration. | Ambiguous provider creation or conflicting terminal evidence enters recovery; it is never replayed. |
| Application tools | Explicit catalog, disclosure contracts, read-only loop, sequential one-mutation supervised flow, fresh principal/tool policy/resolver authorization. | Application operation registration, policy, logical target transport, delegated credential issuer, disclosure classification. | Recursive control-plane/introspection, autonomous writes, mixed/partial/parallel/stateless supervised execution are closed; generic parallel consequential execution is unsupported. |
| Provider-persistent files | Exact inline attachment input and deletion of already-known provider artifacts are independent implemented seams. | MIME/egress/storage policy for inline input and cleanup adapter behavior. | Provider upload/index/search and raw vector-store IDs are closed. |
| Configuration | Authenticated provider/profile credential, content-protection, retention, budget, and pricing lifecycles. | Administrative policy, recent-MFA deployment, secret/keyring backend, endpoint policy. | Durable tool-policy management remains closed until its constraints and applied restore are complete. |
| Backup/restore | Schema-module identity, entity/ordering metadata, restore hooks, pure reconciliation plan, and readiness-closed design. | Reviewed compatible backup adapter, object storage, privileged operator workflow, empty-target rehearsal. | Applied backup/row/object restore and automatic runtime opening are closed. |
| Recovery | Expired-lease reclaim, exact completed-result adoption, background reconciliation, cleanup/retention workers, recovery-required preservation. | Privileged operator evidence and audit integration. | Uncertain external effects/budget are never guessed, cleared, or replayed. |

## Release acceptance

For a candidate commit, record:

- exact crate, schema-module, `graphql-orm`, `graphql-orm-macros`,
  `agql-auth`, and storage dependency revisions;
- one resolved package/type universe;
- formatting, complete tests, warnings-denied Clippy, warnings- and
  missing-docs-denied Rustdoc, PascalCase SDL, backend compile profiles,
  release policy, packaging, and SemVer results;
- the owned PostgreSQL and SQL Server results, including any environmental
  skip and the coverage it leaves unverified;
- migration and schema-fingerprint deltas;
- any temporary root `.handoff/` coordination record and its reviewed final
  SHA, if applicable; and
- every capability intentionally kept closed.

Passing the crate matrix does not publish the crate, change `publish = false`,
run a consumer migration, or authorize a provider/application action.

## SQL Server deployment

- Provision the AI tables in `dbo` using the generated SQL Server migration
  plan and external change control. Use `dbo` as the default schema for both
  the schema owner and runtime role. The generated plan gives indexed AI
  string keys a Unicode type with `Latin1_General_100_BIN2` collation even
  when the database default is case-insensitive. Generated AI string filters
  compare exact casing and trailing spaces.
  SQL Server unique indexes still treat otherwise equal values that differ
  only by trailing spaces as duplicates. SQL Server also enforces its index
  key size limits. Such conflicts or size errors reject writes.
- Install the SQL Server restore marker with the schema owner before
  planning or opening runtime connections. Open the runtime pool with
  `MssqlPool::connect_ado_external_writable_with`, a finite query timeout and
  a verified server certificate. Set `SchemaPolicy::ExternalWritable` on the
  ORM `Database` handle. Give the runtime role data access and restore-marker
  reads. Keep permissions to change the schema with the schema owner. SQL
  Server uses `SERIALIZABLE` for state-machine writes.
  Deadlock victims go through the existing bounded transaction retry paths.
- Run the existing AI reconciliation and operator acknowledgement workflow
  after host-managed restore. The SQL Server restore marker blocks ordinary
  reads and writes during the physical restore. The runtime epoch gate controls
  reopening after collection and reconciliation. Automated portable backup
  apply remains closed. SQL Server provides no proof that native row filters
  are inactive, so recovery skips ambiguous missing-row cases.
- Nonempty session activity lists return a configuration error because SQL
  Server does not support the ORM runtime relation batches used to populate
  them. Session point reads and durable subscription replay remain available.
- Session-content retention returns a configuration error before mutation.
  The ORM can enforce append-only checkpoints with triggers but has no SQL
  Server implementation of the privileged, bounded checkpoint purge. Durable
  checkpoint writes and context compaction use their ordinary fenced paths.

### Run candidate indexes

The SQL Server plan declares an `NVARCHAR(450)` BIN2 run state and
state-first indexes on `next_attempt_at` for claims and `lease_expires_at`
for lease recovery. Both indexes carry the maintenance order columns used
by their candidate reads. The generated exact comparison still checks
trailing spaces. The missing-expiry read also supplies a plain state
membership predicate that SQL Server can use as a seek key. The candidate
bounds and the lease, epoch, and row-version checks remain in force.
Existing stores apply the
[run-state migration preflight](../MIGRATION.md#01560-to-01570) before
the external schema plan.

The owned SQL Server 2022 comparison used separate 3 GiB containers with
zero swap and 15-second query deadlines. Each database held 4,096 runs,
including 320 queued runs and 64 expired leases. Three concurrent claim
workers made 96 calls each, and two recovery workers made 32 calls each.
The benchmark restored the legacy schema for the first run and used the
generated schema for the second.

| Measurement | Legacy schema | Indexed schema |
| --- | --- | --- |
| Claim with no next-attempt timestamp | Clustered index scan | State seek on the claim index |
| Claim with a due next-attempt timestamp | Timestamp seek with a state filter | State seek on the claim index |
| Recovery with no lease expiry | Clustered index scan | State seek on the recovery index |
| Recovery with an expired lease | Timestamp seek with a state filter | State seek on the recovery index |
| SQL Server deadlock victims (1205) per 352 public calls | 0 (0%) | 0 (0%) |
| Claim `PersistenceFailed` | 0/288 (0%) | 0/288 (0%) |
| Recovery `PersistenceFailed` | 0/64 (0%) | 0/64 (0%) |

Both runs leased 288 candidates and requeued all 64 expired leases. The
plans came from SQL Server's cache after the workers completed. The deadlock
count is the delta of the isolated server's lock counter and includes
deadlocks that bounded retries could absorb. This workload demonstrated the
access-plan change and observed zero deadlocks on both schemas.

```bash
cargo test -p graphql-orm-ai --locked --no-default-features --features mssql \
  --test durable_parity owned_sql_server_measures_concurrent_run_candidates \
  -- --ignored --nocapture --test-threads=1
```

## Consumer acceptance

Each consumer owner must independently verify its composed GraphQL SDL,
PascalCase/default naming choice, exact registered tool documents and
disclosure projections, authorization and tenant isolation, egress/residency,
budget policy, secrets/delegation/private transport, migration from its actual
deployed version, empty-target backup/restore, and operational rollback. These
tests belong to the consumer repository and must use only consumer-owned
disposable infrastructure.
