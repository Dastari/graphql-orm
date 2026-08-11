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
