---
title: Authentic 8e1 owned planning fixtures
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-18
review_by: 2027-02-01
supersedes: []
---

# Authentic 8e1 owned planning fixtures

These twelve positive fixtures were produced by the original successful
`plan_owned_migration` at exact source
`8e1e5698437d9ea5c0d6ff2c0f29cf7af58d45a0` (ORM 0.50.2), with Rust 1.97.1
on x86_64 Linux. They were not reconstructed from the new verifier. The
[provenance record](provenance.txt) retains compiler/source/lock/backend/image
identities and SHA-256 values for the complete byte-sensitive fixtures and
[instrumentation patch](instrumentation.patch).

The patch adds a test-only observation hook immediately after the original live
plan validation succeeds. It serializes the actual `current`, target, ownership,
options and complete original output. It adds no introspection, changes no
algorithm, and retains unowned metadata rather than deriving a source from a
target. Private serialization adapters contain no planning logic. The helper's
crate alias permits the existing disposable PostgreSQL harness to compile inside
lib tests. Additional unit-test setup is independent of the engine algorithms.

Each backend has no-op and column-addition cases with legitimate unowned default
and index metadata, and a compound ordered One/Restrict FK plan. Both strict and
managed-only options are covered. Identity strings contain quotes, Unicode,
backslashes and a newline; the complete typed fields, strings, vectors, SQL,
risks/reasons and all hashes are retained. No application rows are used.

Reproduce in an isolated checkout of the exact source, using the qualified
compiler and its root lockfile:

```sh
git apply /path/to/instrumentation.patch
cargo test -p graphql-orm --offline --locked --no-default-features \
  --features sqlite --lib replay_fixture_positive_sqlite -- --nocapture
cargo test -p graphql-orm --offline --locked --no-default-features \
  --features postgres --lib replay_fixture_positive_postgres -- --nocapture
```

The recorded patch writes fixtures to `/tmp/graphql-orm-replay-authentic`; compare
all bytes and checksums without replacing expected data. SQLite is process-owned
in memory. PostgreSQL uses the existing labelled disposable Docker harness;
`current_database()` proves the generated database identity, pools close and the
harness verifies exact container ID/owner label before removal. The recorded runs
passed one selected positive test per backend and released their resources.

Current conformance runs select only `--lib replay_positive`, alongside the
[documented scoped checks](../../../../../docs/reference/graphql-orm/owned-planning-replay.md).
They compare live capture against the complete historical bytes and the ordinary
planner, compare original introspection call counts, encode/decode and verify all
twelve fixtures, and repeat offline verification after the owned database closes.
These fixtures establish their listed cases. They do not prove arbitrary historic
artifact adoption, unsupported compiler recipes, refusal/security/failure paths
or any current application authority. Reproduction differences require review;
there is no current-only or resealing fallback.
