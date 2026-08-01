---
title: ADR-0003 Resolver metadata is discovery not authority
kind: decision
status: accepted
owner: graphql-orm-maintainers
last_reviewed: 2026-08-01
review_by: 2027-08-01
supersedes: []
---

# ADR-0003: Resolver metadata is discovery, not authority

## Context

Generated resolver descriptors and schema-root catalogs provide stable
operation identity, exposure details, signatures, and fingerprints. AI tooling
and clients can use them to discover operations and detect drift. Treating that
metadata as authorization would omit the finished schema, custom roots,
document projection, current principal, row policy, application disclosure,
and runtime resolver decisions.

## Decision

Resolver-operation metadata, fingerprints, directives, and client manifests
are descriptive and advisory. They never authorize execution.

The server remains authoritative. A host must independently validate the
finished schema and operation document, apply disclosure/tool-admission policy,
resolve the current principal, enforce resolver and row authorization, apply
operation assurance where required, and execute through the ordinary resolver
path. Drift or an unknown descriptor fails closed wherever an exact binding is
required.

## Consequences

- Generated and AI consumers can bind exact operation identities without
  inheriting authority.
- Custom roots retain explicit reviewed contracts unless an owning metadata
  contract is added later.
- Fingerprint equality proves only the documented metadata surface, not policy
  equivalence or permission.

## Supersession

Any broader security meaning for resolver metadata requires a new ADR and a
complete authorization threat model.
