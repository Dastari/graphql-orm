---
title: Security property and fuzz testing scope
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-18
review_by: 2027-02-01
supersedes: []
---

# Security property and fuzz testing scope

## Outcome

Implement #513 through [bounded parser/canonicalization properties (#725)](https://github.com/Dastari/graphql-orm/issues/725)
and [JWT/claim fuzz targets (#726)](https://github.com/Dastari/graphql-orm/issues/726).

## Non-goals

No production fuzzing, live services, real keys, unbounded CI jobs, replacement
of authorization regressions or claims that a clean run proves security.

## Dependencies

Owning crates select harnesses and bounded generators. Use synthetic tokens
and keys, temporary storage and test-owned database infrastructure only.

## Acceptance gates

| Surface | Required properties and hostile inputs |
| --- | --- |
| JWT and claims | Malformed encodings, algorithms, expiry/audience/issuer boundaries and size limits; no panic, no authentication without signature and claim checks. |
| Scope matching | Exact/wildcard boundaries, Unicode and separators; malformed input cannot broaden grants. |
| SQL quoting | Quotes, separators and Unicode on each dialect; one identifier remains one identifier and round-trips under the dialect contract. |
| Storage keys | Traversal, reserved namespace, Unicode and separators; accepted keys remain within the addressed namespace. |
| Canonical JSON | Key permutation, nested arrays, numbers and malformed/oversized inputs; idempotence and identical bytes for defined equivalent values. |

Run deterministic bounded property suites and corpus smoke tests in ordinary
CI; longer scheduled fuzz jobs have explicit time/memory limits. Retain seeds,
minimized failures and a regression test for every discovered bug. Promote
security-sensitive findings through the maintainer's private reporting path.

## Current checkpoint

Implemented for #725 and #726 (ORM 0.179.0, storage 0.18.2, router 0.16.5).
Every acceptance-gate surface has a seeded, bounded suite in ordinary CI:

- Scope matching: router scope-template properties under exact and wildcard
  matchers, and the `auth-agql` adapter's syntax check and expiry conversion
  (`crates/graphql-orm-router/src/auth.rs`, `src/agql.rs`).
- SQL quoting: text properties for SQLite, PostgreSQL and SQL Server, and
  executed round trips on in-memory SQLite and the owned PostgreSQL and SQL
  Server containers (`crates/graphql-orm/tests/identifier_quoting_properties.rs`).
- Storage keys: validators against an independent contract model, plus a
  local-provider containment round trip (`crates/graphql-orm-storage/tests/storage_core/key_properties.rs`).
- Canonical JSON: write/read, canonical identity, one spelling, fixed points
  and limits (`crates/graphql-orm/tests/core_contracts/canonical_json_properties.rs`).
- JWT and claims: cargo-fuzz targets with an independent oracle, a retained
  corpus and regressions replayed by `cargo test -p graphql-orm-router`, and
  a weekly 20-minute run per target (`crates/graphql-orm-router/fuzz/`).

Findings fixed with regressions: a bearer `crit` header was ignored (on
both providers, including `"crit": null`, found in review), an
`iss` array containing the issuer was accepted, and a SQLite name starting
with U+FEFF lost the mark. Open, with ignored regressions: #782 (malformed
delimited input passes through `quote_identifier`) and #783 (SQL Server
bare non-ASCII names). Archive this plan when both are resolved.
