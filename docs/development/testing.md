---
title: "Testing and verification"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-15
review_by: 2027-02-01
supersedes: []
---

# Testing and verification

Run the narrowest package and backend lane that covers a change. Database
backends are alternative configurations, so never use workspace
`--all-features`.

Full release validation follows the stable compiler used by CI (Rust 1.97.1
at the current release preparation). Compile-failure snapshots track that
compiler's diagnostics, and the locked S3/Azure SDK lane requires Rust 1.94.1
or newer. Rust 1.90.0 is the separate router minimum-version lane; it is not
a workspace-wide compiler requirement.

## Workflow dependencies and permissions

Pin external GitHub Actions to full commit SHAs and retain the upstream version
in a comment. Review the upstream revision before updating a pin, including
composite action dependencies and downloaded tools; pinning the entry point does
not make those dependencies immutable. Keep workflow token permissions at
`contents: read`, granting write and attestation permissions only to the release
publication job. Changes must preserve every backend and provider validation
lane and the protected release gate. This follows
[GitHub's secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use).

## Baseline ORM lane

```sh
cargo fmt --all -- --check
cargo test -p graphql-orm --no-default-features --features sqlite
cargo clippy -p graphql-orm --no-default-features --features sqlite -- -D warnings
cargo doc -p graphql-orm --no-deps
```

Compile every affected backend lane explicitly:

```sh
cargo check -p graphql-orm --no-default-features --features sqlite
cargo check -p graphql-orm --no-default-features --features postgres
cargo check -p graphql-orm --no-default-features --features mssql
cargo check -p graphql-orm --no-default-features --features "sqlite mssql"
```

When manifests or backend features change, inspect the resolved graph:

```sh
cargo tree -p graphql-orm --locked --no-default-features --features sqlite
cargo tree -p graphql-orm --locked --no-default-features --features postgres
cargo tree -p graphql-orm --locked --no-default-features --features mssql
cargo tree --duplicates --workspace --locked
```

## Required authorization and privacy regressions

CI and release validation select security integration targets explicitly. The
default SQLite package suite does not enable `auth-agql` and therefore cannot
exercise the current-authority integration tests. The `orm-security-sqlite` job
enables `sqlite,auth-agql` and runs `current_authority`, `authorization_mode`,
`authorized_pagination`, `subscription_release_authorization`,
`subscription_provenance_authorization` and `subscription_repository_provenance`.
These cover accepted-request coherence and revocation, strict policy modes,
bounded authorized pagination, event-time authority and private provenance.

The `orm-security-portable` matrix runs `subscription_portable_authorization` and
`bounded_protected_scans` with exactly one backend selected: SQLite, PostgreSQL
or MSSQL. SQLite uses temporary in-memory databases. The other profiles create
their own disposable loopback containers with random ownership identities and
exact-identity cleanup; they do not take an application database URL. These
portable tests are not ignored, so an `--ignored` invocation would skip them.
The matrix also compiles and runs warnings-denied library Clippy with
`auth-agql` on each backend; this is compile/lint coverage for PostgreSQL/MSSQL
current-authority integration, not a claim of runtime parity for SQLite-only
tests.

Both jobs use one Cargo build worker and one test thread; the portable matrix
admits one backend at a time and each job has a finite timeout. The shared
`scripts/run-security-test-targets.sh` executes each selected target separately
and requires a nonzero passing-test count in its output, so a feature-gated
zero-test binary cannot satisfy the security gate. Ordinary ignored benchmarks
remain excluded. Run the same explicit targets locally through the applicable
resource-controlled runner:

```sh
bash scripts/run-security-test-targets.sh sqlite,auth-agql current_authority authorization_mode authorized_pagination subscription_release_authorization subscription_provenance_authorization subscription_repository_provenance
bash scripts/run-security-test-targets.sh postgres subscription_portable_authorization bounded_protected_scans
bash scripts/run-security-test-targets.sh mssql subscription_portable_authorization bounded_protected_scans
```

Nested fixture Cargo commands inherit the caller's `CARGO_BUILD_JOBS` budget.
The cross-crate, relationship-capability, backend-coexistence, federation,
migration compatibility and repository aggregate fixture drivers use locked
dependency resolution; a stale fixture lockfile fails validation instead of
being silently rewritten during a test. Managed Samba storage and backup tests
also require the reviewed workspace lockfile. The independent
`crates/graphql-orm-macros/fixtures/relationship-capability` workspace retains its
own reviewed `Cargo.lock`; generate or update that lock explicitly before
running its parent driver. Include this lock when collecting fixture dependency
evidence.

The existing `sqlserver-security` job separately executes the ignored owned TLS,
writable-transaction and query-notification tests. Release security jobs depend
on the protected `guard`; the aggregate `validate` gate requires them before
publication. Workflow configuration establishes required execution, not a
passing result for an unexecuted candidate.

## Package-specific lanes

Select companion crates explicitly and use their documented provider features.
For example:

```sh
cargo test -p graphql-orm-storage
cargo test -p graphql-orm-backup --features orm-sqlite
cargo check -p graphql-orm-ai --no-default-features --features postgres
cargo test -p graphql-orm-router-protocol
cargo test -p graphql-orm-router
cargo test -p graphql-orm-router --features auth-agql
```

Run package-local tests and their required feature matrices whenever a change
crosses the package boundary.

## Dependency security review

Run `cargo audit --json` against the committed root lockfile and retain the
advisory database revision and output with verification evidence. A nonzero
result requires triage; it must not be silently converted into a successful
security check. Inspect vulnerability, unsoundness, unmaintained and yanked
reports, and check each selected backend/provider graph with `cargo tree
--locked -p <package> --no-default-features --features <lane> -i <dependency>`.
An optional dependency in the lockfile is not proof of runtime reachability.

Prefer compatible patched dependencies and removal of unused legacy features.
For unresolved advisories, record the affected path, exposure conditions,
mitigation, owner and next review date. Recheck those decisions when features
or upstream adapters change. Do not use blanket advisory ignores or substitute
a passing build for security review. Use the
[RustSec advisory database](https://rustsec.org/advisories/) as the source of
affected and patched ranges; keep version inventories generated from Cargo
metadata instead of maintaining a second dependency list here.

Provider implementations are separate feature lanes. Run them locally one at
a time so an accidental dependency on another provider feature cannot make a
lane pass:

```sh
scripts/check-ai-provider-lanes.sh test
scripts/check-ai-provider-lanes.sh clippy
scripts/check-ai-provider-lanes.sh doc
```

The runner covers `provider-openai`, `provider-anthropic`, `provider-xai`,
`provider-ollama`, `provider-openai-compatible`, `local-harness`,
`provider-codex-app-server`, and `provider-grok-acp`, each with only SQLite and
that provider enabled.
Pass one provider feature as the second argument for a focused run, and
`--list` to print the lane set as JSON. The release workflow uses both to give
every provider lane its own runner and its own `target/`. These local commands
are release evidence; a hosted workflow result is not a substitute.

Router changes also require warnings-denied package lanes and the dependency
boundary check:

```sh
cargo clippy -p graphql-orm-router-protocol --all-targets -- -D warnings
cargo clippy -p graphql-orm-router --all-targets -- -D warnings
cargo clippy -p graphql-orm-router --all-targets --features auth-agql -- -D warnings
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc -p graphql-orm-router-protocol --no-deps
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc -p graphql-orm-router --no-deps
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc -p graphql-orm-router --no-deps --features auth-agql
scripts/check-workspace-dependencies.sh
python3 scripts/generate-workspace-inventory.py --check
python3 scripts/check-documentation.py
```

Router integration tests own their loopback subgraphs, listeners, and
timeouts. They include raw HTTP and `graphql-transport-ws` clients plus an
upstream WebSocket subgraph and exercise authentication, expiry, bounded
connections/operations, failure isolation, conditional schema/protocol polling,
in-flight request pinning, invalid-candidate last-known-good behavior, and
authenticated subscription retirement/reconnect after atomic graph-and-policy
replacement. The standalone-binary smoke test performs pre-bind checking,
authenticated HTTP and WebSocket execution, downstream timeout/recovery,
signal drain, and listener-release checks. The bounded hardening campaign also
covers repeated reload, JWKS outage/rotation, WebSocket churn, and subscription
lag. They must not contact a deployed subgraph or application database.

## Required S3 transport regressions

The CI and release `s3-security` jobs execute the reviewed SDK graph's owned
HTTP lost-response/replacement and HTTPS trusted-CA/untrusted-CA regressions.
The HTTP fixture requires ambiguous single-attempt deletion to preserve a
replacement lock. The HTTPS parent creates a loopback Python server and
temporary CA/server certificates, then runs isolated child clients with private
trust configuration. It requires `openssl` and `python3`; no Docker, cloud
account, application endpoint or service credentials are used.

Run the same selected tests locally through the applicable resource-controlled
runner:

```sh
bash scripts/run-owned-s3-security-tests.sh
```

The script uses the S3 feature without provider defaults and selects the exact
`single_attempt_delete_does_not_replay_after_lost_response_and_replacement`
test in `s3_integration`, then the ignored `owned_https_transport_fixture`
parent in `s3_https_transport`. It never runs the environment-configured
real-service round trip or invokes the HTTPS child entry point on its own.
Proxy and real-service test environment settings are cleared within the script.
Each Cargo process must succeed and its final parent summary must report a
nonzero passing-test count; nested HTTPS child summaries cannot satisfy an
empty parent invocation. Tests run with one thread.

Each job has one Cargo build worker and a finite timeout. It also runs storage
all-targets Clippy with explicit `local,s3,azure` features, warnings-denied
Rustdoc with `local,s3`, and backup compilation with the dependency feature
`graphql-orm-storage/s3`. Local examples and Azure placeholder helpers require
that explicit Clippy profile; the backup package has no feature named `s3`.
The release job depends on the protected `guard`, and aggregate `validate`
requires its success before publication. These gates establish required
execution; passing results must still be recorded for the exact candidate.

## External-service tests

SQLite tests may use temporary local databases. PostgreSQL and MSSQL acceptance
tests may run only against infrastructure the test creates, labels,
loopback-publishes, and removes itself—never an application, staging, shared
developer, or manually supplied database. The canonical local runner rejects
ambient database URL variables:

```sh
scripts/run-owned-database-lanes.sh sqlite
scripts/run-owned-database-lanes.sh postgres
scripts/run-owned-database-lanes.sh postgres-legacy
scripts/run-owned-database-lanes.sh mssql
scripts/run-owned-database-lanes.sh ai-postgres
```

Use `all` only when the machine has enough memory for the lanes to run
sequentially. Docker absence or failure is a failed required lane, not a skip.
Follow the [PostgreSQL testing runbook](../operations/runbooks/postgres-testing.md)
and [MSSQL reference](../reference/graphql-orm/mssql.md) for the owned-resource
contract and coverage.

Owned PostgreSQL and SQL Server tests are intentionally ignored in ordinary
unit-test runs because they start containers. Invoke them through the canonical
runner rather than exporting `TEST_DATABASE_URL`, `MSSQL_TEST_DATABASE_URL`, or
`DATABASE_URL`. The `postgres-legacy` lane migrates older generated-schema, relation, mutation,
RLS, transaction, projection, and PostGIS tests to the owned harness. No ORM
integration test accepts a manually supplied database URL. These tests now use
`#[ignore]` for container startup rather than silently passing without a URL.
The lifecycle regression proves cloned pools keep their container alive and
release it when the last pool is dropped; exact identity and ownership labels
remain required for cleanup. PostGIS tests use a separate owned PostGIS image.

## Authentication bridge lane

Changes involving `auth-agql` must retain one external Git selector and its
reviewed locked commit, and test the bridge feature explicitly:

```sh
cargo check -p graphql-orm --no-default-features --features "sqlite auth-agql"
cargo test -p graphql-orm --no-default-features --features "sqlite auth-agql" --test agql_assurance
cargo tree -p graphql-orm --locked --no-default-features --features "sqlite auth-agql"
cargo test -p graphql-orm-router --locked --features auth-agql
cargo tree -p graphql-orm-router --locked --features auth-agql
```

The graph must resolve a single `agql-auth` revision and one
`graphql-orm`/macro universe. The router lane exercises only the one-way
resource-server validator and matcher adapters; it must not instantiate
issuer-side or storage services.
