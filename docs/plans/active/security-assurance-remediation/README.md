---
title: Security assurance remediation
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-14
review_by: 2026-11-14
supersedes: []
---

# Security assurance remediation

## Outcome

Close the defensive audit findings against current source through independently
reviewable pull requests, with executable regressions, explicit compatibility
changes, and evidence for every supported database and affected companion lane.
The original audit examined ORM commit
`e5023b5c29b097f1c07ebc66057ebb50731a6330` and authentication commit
`674f19bc0d1a2ea3c0ca0c4f96bb7a56b366319b`; findings must be revalidated against
current main before implementation because subsequent changes can already close
them. Preserve the original evidence rather than rewriting its historical result.

Sensitive operations require an authoritative current-principal and revocation
decision at the operation boundary. Unavailable status fails closed. This is a
decision-time guarantee; it does not pretend to atomically reverse an external
operation after a later revocation. Refreshed grants must not retain removed
authority or widen an originally restricted grant. Protected collections may
use bounded scans instead of exact unbounded visible offsets and totals.

## Non-goals

- No application-specific tenant policy, database credentials or domain types.
- No production database or external provider testing; services must be owned
  disposable test infrastructure.
- No automatic tag, package or release publication as a consequence of merging.
- No security certification for code outside the audited or subsequently
  validated scope, and no cosmetic rewrite that delays a security correction.

## Work and dependencies

| Audit reference | Contract to validate or correct | Dependency |
|---|---|---|
| S-001 | Target entity and row authorization on every generated relationship route | Revalidate the already merged relationship-policy work before adding a fix |
| S-002 | Current grants on refresh; no widening of restricted/external grants | Authentication current-authority contract and reviewed external revision |
| S-003 / Q-002 | Unambiguous authorization partitions and lossless typed relation keys | Runtime, macros and all loader/key consumers change together |
| S-004 / Q-006 | Bounded abuse-state lifecycle and credential-verification work | Clock/CAS semantics and explicit host work admission |
| S-005 | Exclusive backup lock lifetime and atomic conditional creation | Provider-independent fail-closed behavior and offline recovery procedure |
| S-006 | Safe public errors across generated resolver paths | Existing stable error mapping and internal diagnostics |
| R-001 | Bounded protected collection scans and honest pagination metadata | Existing authorized scan contract; migration from exact legacy semantics |
| R-002 / R-003 / Q-009 | Current authority, token bindings and consistent request context | Auth change before ORM bridge and sensitive-operation enforcement |
| R-004 | Local filesystem root confinement or explicit trusted-root contract | Local provider path semantics and synthetic link tests |
| U-001 | Defined concurrent one-time-code attempt budget | Deterministic interleaving test before changing store admission |
| H-001 | Reachable dependency advisories resolved or individually justified | Current feature graphs and backend/provider compatibility checks |
| H-002 | Immutable action references and least-privilege CI | Reviewed action revisions and explicit release-job permissions |
| Q-001 / Q-005 | Strict Bearer composition and invalid configuration boundaries | Authentication interface/constructor compatibility |
| Q-003 / Q-007 | Restore partial-failure and buffering guarantees | Adapter capabilities, documented limits and fault-injection tests |
| Q-004 / Q-010 / Q-011 | Accurate compatibility guidance and test ownership | Generated inventories, executable examples and owned database harness |

Refactoring duplicated enforcement (Q-008) supports the security contracts where
needed; broader generator cleanup remains separate from mechanical dependency
and repository work.

## Acceptance gates

- Each finding has current-source evidence and a focused PR or an explicit
  already-fixed disposition with executable regression evidence.
- SQLite, PostgreSQL and MSSQL remain supported. Changed runtime paths execute
  on the affected owned backend lanes; compile success alone is not runtime
  evidence. Affected AI, router, storage and backup provider lanes also pass.
- Relevant tests, formatting, warnings-denied Clippy/Rustdoc, dependency
  direction and release policy checks pass. Alternative backend/provider
  configurations are selected explicitly; never use workspace all-features.
- Every public/runtime change updates the owning package's README, changelog,
  migration guide and SemVer according to release policy. Breaking pre-1.0
  contracts receive a minor bump and concrete prior-version migration steps.
- Run the inventory generator after manifest changes. Governed documentation
  keeps metadata, canonical topic ownership and index links; accepted ADRs are
  immutable. Every PR declares documentation impact and passes the checker.
- An independent challenge of each implementation precedes merge. Review the
  exact final commit and CI result; do not merge merely because a draft exists.
- Final integration uses one reviewed external authentication revision and
  verifies permission removal, immediate sensitive-operation revocation,
  relationships, concurrent contexts, rollback and missing-context denial.

## Current checkpoint

Revalidation and the first implementation wave are in progress on isolated
branches based on current main. The existing disabled-login prerequisite
[authentication PR 11](https://github.com/Dastari/agql-auth/pull/11) is merged.
Relationship-policy changes already present upstream are being challenged;
authorization-key, refresh/current-authority and backup-lock corrections are
being implemented separately. Remaining rows are pending current-source
validation and focused changes. No final production-readiness claim is made.

Temporary coordination and command logs remain outside the authoritative docs
namespace. Replace this checkpoint as work advances; archive the completed plan
with final evidence when all acceptance gates are satisfied.
