---
title: graphql-orm-router-protocol agent guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router-protocol Agent Guide

## Boundary

- Keep this crate project-neutral, serializable declarations and deterministic
  utilities only.
- Do not add Hive, Axum, a GraphQL server, database backend, `graphql-orm`,
  `agql-auth`, product-specific code, application types, network I/O, URL
  parsing, credentials, or deployment overrides.
- Endpoint strings are inert advertisements. Router code owns SSRF policy,
  DNS and network validation, credentials, and override selection.
- Unknown additive fields remain compatible. New semantics that a reader must
  understand must be advertised through `requiredSemantics` and rejected by
  older readers.
- Router authorization is advisory defence in depth. `SubgraphOnly` must remain
  available for dynamic, custom, and unrepresentable authoritative policy.

## Change rules

- Preserve JSON camelCase names and stable `ProtocolErrorKind` codes.
- Any wire incompatibility requires a protocol-major decision, migration notes,
  golden fixtures, and a later protocol version.
- Keep canonical ordering and fingerprint tests for every new unordered field.
- Do not expose federation-engine types through this crate.

## Verification

Run the standalone manifest checks in the package README. Do not use workspace
`--all-features` when this crate joins the workspace.
