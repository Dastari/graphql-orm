---
title: Security property and fuzz testing scope
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-15
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

Maintainer recommendation accepted on 2026-11-15; issues #725 and #726 define
implementation scope. This PR records the plan, not completed fuzz coverage.
