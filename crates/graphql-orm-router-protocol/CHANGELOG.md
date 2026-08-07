---
title: graphql-orm-router-protocol changelog
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-07
supersedes: []
---

# Changelog

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
