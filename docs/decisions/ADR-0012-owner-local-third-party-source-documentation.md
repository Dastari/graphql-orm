---
title: ADR-0012 Owner-local third-party source documentation
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-10-15
review_by: 2027-04-15
supersedes: [ADR-0001]
---

# ADR-0012: Owner-local third-party source documentation

## Context

A narrowly backported third-party dependency must retain its original documentation
and license attribution. Rewriting upstream Markdown to add first-party metadata
would obscure exact source provenance. A blanket vendor-directory exclusion would
also let unrelated first-party documentation evade governance.

## Decision

Extend ADR-0001 only for the reviewed `smb` 0.10.2 source under
`crates/graphql-orm-storage/vendor/smb`. Preserve the original six Markdown files
verbatim. The upstream MIT text is retained separately as `LICENSE`, with its
original source path recorded. The checker exempts only the six Markdown
explicit paths and fixed original SHA256 values, with matching owner-local
`PROVENANCE.json` identifying the archive checksum and full upstream backport
revision. Missing/altered provenance, changed bytes, symlinked paths, extra files
or another vendor root do not receive the exemption. No other directory or
third-party package is implicitly exempt.

First-party rationale, integration limits, change history and migration guidance
remain governed documents outside that subtree. Normal public documentation
verification, immutable source-tree ownership and dependency review still apply.
The exclusion is not verification of the backport runtime code; a separate exact
archive/patch provenance check owns that contract.

## Consequences

The owning storage package tree and tag cover source, license and provenance.
This private dependency is excluded from workspace membership; the workspace
retains nine independently versioned first-party packages. Its archived upstream
Cargo.lock is not an additional workspace lock. Future dependency replacement
requires explicit provenance/hash and checker review; there is no generic vendor
allowlist or automatic exemption.

## Supersession

This record narrowly supersedes ADR-0001's enumerated Markdown exceptions. All
other documentation authority, lifecycle and immutability rules remain unchanged.
The original accepted ADR is not edited.
