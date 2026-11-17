---
title: ADR-0014 Workspace minimum supported Rust version
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-11-16
review_by: 2027-02-15
supersedes: [ADR-0008]
---

# ADR-0014: Workspace minimum supported Rust version

## Context

ADR-0008 records the router's former Rust 1.90 minimum and its 1.90.0 lane.
The workspace now validates its independently consumable packages together,
including backend and provider graphs whose requirements exceed that historical
router-only baseline. Existing CI and diagnostic qualification use Rust 1.97.1.
Declaring 1.97 while testing 1.97.1 would promise an untested 1.97.0 minimum.

## Decision

The workspace MSRV is the **Rust 1.97 series, exactly 1.97.1**. All workspace
package `rust-version` fields declare `1.97.1`; the repository toolchain and CI
MSRV lanes select 1.97.1. This remains within the maintainer's ceiling of 1.97.
A common, validated compiler floor keeps package, backend and provider support
claims consistent and makes the tested patch level explicit. The UI lanes also
install `rust-src` for reproducible standard-library diagnostic excerpts.

This ADR supersedes **only the MSRV and minimum-compiler lane statements** in
ADR-0008's Context (the paragraph declaring Rust 1.90 and a 1.90.0 lane). It does
not supersede the Federation runtime choice, composition adapter, dependency
isolation, distribution approval or other decisions in ADR-0008. That accepted
record remains unchanged as historical evidence.

## Consequences

Hosts need Rust 1.97.1 or newer. No stored data or schema migration follows from
this compiler policy. Future floor changes must update manifests, CI, toolchain
configuration and current upgrade guidance together and validate the selected
graphs on the declared compiler. Cargo configuration and the MSRV script own
the current mechanics; this record explains the policy and its partial
supersession of the older router-only baseline.
