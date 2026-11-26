---
title: graphql-orm-router-protocol changelog
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-07
supersedes: []
---

# Changelog

## Unreleased - 0.4.0

- Inherit the workspace lint baseline, which forbids unsafe Rust in crate
  targets. The library still requires documentation for its public API.

## Unreleased - 0.3.1

- Install from a reviewed Git revision using the package inventory and release manifest.

## Unreleased - 0.3.0

Declare Rust 1.97.1 as the minimum supported compiler; verify explicit feature graphs in CI.

## 0.2.1 - 2026-08-13

- Published the maintained protocol configuration and example navigation from
  the coordinated documentation overhaul. Protocol wire 1.0 and all public
  data contracts are unchanged; no migration is required.

## 0.2.0 - 2026-08-11

- Added optional project-neutral `DescriptorExtension` values. Each extension
  has a lower-case identity, positive extension-owned version, bounded
  canonical JSON payload, and SHA-256 fingerprint.
- Extensions are canonically ordered and participate in the descriptor
  combined fingerprint but not its authorization fingerprint. An empty
  extension list retains the established protocol-v1 combined fingerprint.
- Protocol wire major 1 remains current. Old JSON without `extensions`
  decodes unchanged; consumers of a named extension are responsible for
  rejecting unsupported or incomplete extension versions.

## 0.1.0 - 2026-08-07

- Added a framework-neutral descriptor builder that validates identity and
  advertisements, canonicalizes metadata, and computes protocol fingerprints
  for host-owned `/.well-known/graphql-router` routes.
- Added a maintained hand-written descriptor example and protocol v1 migration
  guidance.
- Added protocol v1 data declarations for subgraph identity, inert endpoint
  advertisements, capabilities, operations, authorization, scope templates,
  explicit subgraph-only policy, and canonical fingerprints.
- Added compatible-minor decoding and stable incompatible-major and required-
  semantic error categories.
