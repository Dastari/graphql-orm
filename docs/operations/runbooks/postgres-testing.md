---
title: "Postgres Test Coverage"
kind: runbook
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-14
review_by: 2026-11-01
supersedes: []
---

# Postgres Test Coverage

Postgres is the primary compatibility target for generated schema management.
Release acceptance uses only test-owned disposable PostgreSQL containers, not
an application, staging, shared developer, or manually provisioned database.

From the repository root, run the owned PostgreSQL aggregate lane:

```sh
scripts/run-owned-database-lanes.sh postgres
```

Run the AI persistence parity lane separately:

```sh
scripts/run-owned-database-lanes.sh ai-postgres
```

Each selected test creates a unique database and credentials in a labelled
container, publishes its port on IPv4 loopback only, and owns cleanup. Tests
that assert cleanup verify the exact container identity before removal and
then prove the container is absent. The runner rejects `DATABASE_URL`,
`TEST_DATABASE_URL`, and `MSSQL_TEST_DATABASE_URL` so an ambient endpoint cannot
silently replace the owned resource.

Docker must be available to the invoking user. A required owned lane that
cannot start Docker fails; it does not report a skipped parity result. The
containers run sequentially and must never share state.

The PostgreSQL aggregate lane covers typed filters, nullable grouping keys,
multiple metrics, integral/floating/decimal sums, deterministic group order,
and bounded group results. The AI lane covers schema installation, durable
persistence behavior, ownership isolation, and restoration against the
backend-specific implementation.

The migrated legacy lane owns a fresh container for every fixture and refuses
ambient database URLs before creating resources. It includes generated schema,
relations, mutations, transaction-local RLS, typed projections, binary keys, and
PostGIS. Container-backed cases are explicitly ignored by ordinary runs; run:

```sh
scripts/run-owned-database-lanes.sh postgres-legacy
```

The owned pool helper retains its cleanup guard inside pool options, so every
clone shares the same container lifetime. Tests with specialized connection
options retain an explicit owner guard through their entire test scope.
Docker failure is a test failure. Panics and early errors drop the guard; a
process killed without destructors may leave a labelled container for explicit
owner-verified recovery. Never clean unrelated containers by a name prefix.

SQLite remains a separate, database-free lane:

```sh
scripts/run-owned-database-lanes.sh sqlite
```
