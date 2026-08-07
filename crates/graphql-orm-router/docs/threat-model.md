---
title: graphql-orm-router threat model
kind: architecture
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router threat model

## Protected assets and trust boundaries

Assets are bearer credentials, JWKS trust configuration, internal schema
credentials, subgraph topology/SDL, authorization metadata, the active
executable graph, administrative authority, and service availability.

Public clients are untrusted. A validated user bearer establishes only its
declared subject/scopes until expiry. Administrative service identities are
trusted only for their exact scope. Dynamic subgraphs are trusted only for the
preconfigured subject, ID/name, and destinations. Static endpoints and network
routing are deployment-owned trust. Subgraph resolver guards, database policy,
and RLS remain authoritative after router preflight.

## Implemented mitigations

- Authentication is fail closed and resource-server-only RS256/JWKS
  validation. The crate accepts no private key and performs no signing,
  issuance, session refresh, or private-key decryption.
- Router authorization binds canonical metadata to the exact active graph and
  may deny before downstream work. It cannot grant around a subgraph guard.
- Bearers propagate only after validation and only to configured GraphQL
  destinations. Schema credentials are source-specific and separate.
- Dynamic registration binds authenticated identity to exact advertised
  origins. Metadata/SDL clients bypass ambient proxies, reject redirects, pin
  validated DNS, verify peers, bound resolution and responses, and deny special
  address ranges unless explicitly allowed. Execution origins are IP literals
  to close the private engine's second-resolution rebinding window.
- Complete immutable graph and policy candidates are constructed before one
  atomic publication. Failure retains the exact last-known-good runtime.
- Public/admin bodies, headers, parsing, graph complexity, downstream time,
  connection pools, WebSockets, operations, messages, and subscription queues
  are bounded.
- Default logs exclude variables/tokens and use structured info events. Safe
  admin errors omit endpoints, SDL, keys, headers, tokens, and downstream
  bodies.

## Residual deployment responsibilities

- Terminate public and administrative TLS with a trusted proxy or service mesh;
  restrict the admin and optional Prometheus listeners by network policy.
- Protect static endpoint DNS/routing and schema environment variables. The
  dynamic SSRF policy does not turn a malicious statically configured endpoint
  into a trusted service.
- Keep direct subgraph authentication/authorization enabled. Do not expose an
  unguarded subgraph because the router normally fronts it.
- Rotate public keys and keep cache/refresh bounds below the desired revocation
  response. The router cannot revoke an otherwise valid JWT in place.
- Treat debug logs as sensitive operational data because GraphQL document
  literals may appear even though variables and bearer values are excluded by
  default.
- Account for ephemeral, process-local event delivery and uncoordinated dynamic
  state. Use the authoritative store for recovery; do not infer delivery or
  durability guarantees.
- Review the exact-pinned Hive dependency, advisories, licenses, SBOM, and
  distribution channel under ADR-0008 before shipping an artifact. Hive JWT,
  S3, and object-storage configuration remain deliberately unreachable.

## Out of scope

The router does not own identity proofing, token issuance, sessions, durable
events, replay, workflow orchestration, cross-instance registry consensus,
business authorization, database RLS, or storage. A future durable integration
belongs above this crate and uses `graphql-orm-storage` without adding that
dependency to the project-neutral router.
