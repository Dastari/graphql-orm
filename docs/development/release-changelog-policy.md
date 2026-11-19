---
title: Release cadence and changelog policy
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Release cadence and changelog policy

Use one reviewed release candidate per coherent batch. The release manifest
and tags determine what shipped; an old `Unreleased` heading is not evidence
that a tagged release is unpublished. Never rewrite an already released
package's changelog to reconstruct missing history. The 0.90–0.107 gaps and
older unreleased headings are historical evidence, not templates to repeat.

Choose patch bumps for compatible fixes and minor bumps for new or incompatible
pre-1.0 contracts. A test/fixture-only or lockfile-only change needs no package
bump unless it changes Cargo-published files; use the release-policy checker
against the PR base. For published-source changes bump above the current base,
keep ORM/macros aligned and update README, changelog and migration obligations.
Do not make empty minor releases for test or lock refreshes. The current batch's
minimum-compiler declarations change published manifests and are released as
one coordinated compatibility batch.

Before tagging, reconcile every package changed since the previous release
against its current changelog entry and the generated release manifest. Date
the candidate entry when publishing; keep at most one new unreleased entry per
package. Describe observable behavior and reference fixes; avoid session logs.
Mark cumulative migration entries **Action needed** or **No action** and state
whether data migration is needed. Run the package release policy, documentation,
inventory and release-manifest checks before publication. Reviewers verify
coverage of the diff; automation does not prove semantic completeness.

This policy implements #505 prospectively. Historical gaps stay intact; release
tags, manifests and archived chronology remain the sources for old releases.

Every release-bearing PR also indexes its host-facing changes under Unreleased
in the [cumulative upgrade guide](../reference/upgrade-guide.md), naming package
versions, marking **Action needed** or **No action**, and linking detailed notes.
