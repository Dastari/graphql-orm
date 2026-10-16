---
title: Security assurance remediation
kind: plan
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-10-16
review_by: 2026-11-14
supersedes: []
---

# Security assurance remediation

## Outcome

Complete the independently reviewed source remediations and bounded acceptance
for the defensive audit findings, retaining executable regressions, explicit
compatibility changes and the actual scope of each backend/companion result.
The original audit examined ORM commit
`e5023b5c29b097f1c07ebc66057ebb50731a6330` and authentication commit
`674f19bc0d1a2ea3c0ca0c4f96bb7a56b366319b`; findings were revalidated against
the implementation baseline before changes; later relationship-policy work
already addressed part of the original scope. Original reports remain historical
evidence and are not rewritten as post-remediation results.

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
| S-007 | Current target-row, selected-field and bounded provenance authorization before each subscription release | Fresh policy/current-authority checks, explicit deleted-preimage opt-in and fail-closed unsupported selections |
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
- An independent challenge of each implementation precedes merge. Review exact
  source and applicable retained local/CI evidence; held CI execution and release
  actions require their separate authorization. A draft PR is not acceptance.
- Final integration uses one reviewed external authentication revision and
  verifies permission removal, immediate sensitive-operation revocation,
  relationships, concurrent contexts, rollback and missing-context denial.

## Acceptance checkpoint

The reviewed implementation set at `836be12f69be8a66d66860881dbf6eb7ec05d917` closes the
revalidated source-remediation work, including the already-fixed relationship
route and the subsequently confirmed subscription and one-time-code findings.
The confirmed scope remains eight security findings (three High, four Medium,
one Low); engineering correctness/capability follow-ups add no vulnerability
count. Approved complete-key reads, checked original filter intent/matching and
Q014–Q020 follow-ups retain their own source, migration and acceptance evidence.
The independently reviewed local acceptance record is
the owner-retained packet `IMPLEMENTATION_ACCEPTANCE.json` (SHA256 `c290c18a6eb834b1fbbb551dccd726e768d073dc272c41cd0a4ab3effcc97e3a`), including the completed bounded AI phase ledger
`FINAL_AI_ACCEPTANCE_33.json` (SHA256 `6a5001ca1b972fc71bcd6e1e8352859f095caa8045107a06158687311601ea11`). Original historical reports and native failure
artifacts remain preserved; later applicability never changes their actual
execution revisions, features, compilers or counts.

Acceptance is scoped to those recorded implementations and explicit selected
configurations, not a whole-application security or production-readiness claim.
Dependency review remains complete **with findings** under its frozen advisory
boundary; see the [canonical disposition](../../../reference/dependency-security-disposition.md).
The [security upgrade guide](../../../how-to/upgrade-security-remediation.md) and
owning migration guides govern consumer rollout. Full-principal PG/MSSQL,
Windows/macOS, real-provider, deep AI/router audit and performance/fuzz coverage
limits remain as recorded. Custom resolver/raw/runtime host boundaries, tenant
policy, authoritative store availability, immutable snapshot provenance and
transactional fencing against concurrent revocation remain host obligations.
Offline matching cannot prove a removed row's native RLS visibility.

This completed implementation plan grants no Actions, merge, tag or publication
authority. Held remote/release steps remain separately governed by the
[release process](../../../operations/release/process.md). The eventual final
source/tag/manifest must bind the post-archive commit; no unpublished preview or
local source assembly is called a published release.
