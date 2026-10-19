---
title: ADR-0013 Explicit source-only release verification profiles
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-10-19
review_by: 2027-02-01
supersedes: [ADR-0010]
---

# ADR-0013: Explicit source-only release verification profiles

## Context

The optional owned-planning replay API has source-qualified positive evidence
on two exact compilers. Its owner review restricts security, refusal, fault and
broad mixed suites to code inspection. The default release matrix executes
those suites. Publishing under that restriction requires an explicit acceptance
policy, rather than interpreting excluded jobs as successful validation.

## Decision

This record replaces ADR-0010 while retaining its identity and distribution
rules. Packages version independently; ORM/macros remain aligned. Qualified
package tags and calendar workspace tags are annotated and immutable. The
deterministic manifest binds the full source SHA, root lock hash, package source
trees/versions/tags, external Git revisions and independent wire/persistence
contracts. Consumers retain the release's exact recorded source identity.
Registry publishing stays disabled; compiled router distribution still needs
separate artifact-specific approval. Tags, assets and published identities
never move.

The **full** release profile remains the default, with every existing matrix
command and mandatory validation dependency preserved. It is not authorized by
an inspection-only scope. A separately selected, source-only
`replay-scoped-positive-v1` profile is permitted for the prepared ORM/macros
0.52.0 source qualified at `1e49f1c63728865b294795684e38a27e48258b5a`.
Only reviewed release-policy/documentation descendants with identical crate
trees, root manifests/lock, compiler configuration and qualification helper may
reuse that evidence. A code, fixture, dependency or profile change needs a new
reviewed qualification/policy; this profile is not an evergreen exception.

The scoped profile has its own mandatory integrity, positive compiler-matrix
and validation jobs, followed by its own gated invocation of the normal
immutable publisher. Every mandatory scoped gate must succeed. Excluded full
jobs and inspection-only obligations remain explicitly **unexecuted**, not
passed. There is no skipped-job/`always()` shortcut between validators. Literal
commands and source-qualified reuse rules live in the
[release runbook](../operations/release/process.md#replay-scoped-positive-v1).

Both profiles retain protected human entry approval, workflow/source/current-main
identity checks, unused tag and package-tree checks, checksums, asset
attestations, atomic annotated-tag publication and immutable GitHub assets.
The scoped profile additionally requires an explicit approval reference binding
source, profile, release identity and retained limitations, and rejects router
binary selection. Policy implementation/merge is distinct from publication
authorization and protected-environment approval.

The scoped release attaches verification metadata and checksummed archives
containing original qualification/audit evidence and fresh positive results.
The evidence labels original source commits, actual counts/skips, the retained
MSSQL integration-target compile failure, audit exit 1/findings, conditional
applicability, advisory age and coverage gaps, and runtime-unverified obligations.
The scoped profile permits reuse of the retained unsuppressed audit snapshot
without a new scan only after the locked dependency records, features,
companion/vendor trees and disposition condition sources are reconciled.
An unchanged graph does not establish current advisory coverage or safety.

The complete replay recipe, wire format, feature defaults, custody consent,
ordinary planning/apply behavior and authority separation are unaffected.
Historical and Rust 1.98.1 recipes stay separately exact-compiler/platform
admitted. A release profile neither upgrades historical receipts nor authorizes
consumer adoption, data validity or deployment.

## Consequences

- Full-matrix releases retain their existing default acceptance contract.
- Scoped releases disclose a smaller, explicitly approved verification scope;
  they make no whole-workspace runtime or current-audit acceptance claim.
- Source reuse is checked mechanically and preserves the original provenance;
  old results are never presented as fresh execution at a later policy commit.
- The attached verification record is release metadata, not replay format 1,
  a new recipe, a consumer receipt or authorization to apply a migration.

## Supersession

Changing eligibility, the literal positive allowlist, evidence-reuse conditions,
retained limitations or distribution/identity rules requires a later reviewed
policy change. Accepted ADRs remain immutable; replace this record with a later
superseding ADR when its decision changes.
