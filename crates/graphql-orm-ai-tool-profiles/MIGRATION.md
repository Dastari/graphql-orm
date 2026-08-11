---
title: "graphql-orm-ai-tool-profiles migration guide"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-11
supersedes: []
---

# Migration Guide

## 0.2.0 to 0.3.0: canonical JSON fingerprints

Update every manifest producer and consumer to the same reviewed monorepo
revision. Manifest wire version 2 recursively sorts JSON object keys before
hashing while retaining array order, scalar representation, schema binding,
entry order, and all nested security contracts. Version 1 payloads remain
unsupported rather than being guessed or silently upgraded.

Tool descriptors containing JSON Schema objects may receive new fingerprints.
Hosts with exact tool-fingerprint allowlists must review and replace those
values when they adopt the new manifest. Do not copy a version 1 fingerprint
onto a version 2 descriptor.

No database or GraphQL schema migration, table change, backfill, or row rewrite
is required. This is a wire/fingerprint and host-policy configuration
migration only.
