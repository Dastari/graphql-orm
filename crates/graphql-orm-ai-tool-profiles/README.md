---
title: "graphql-orm-ai-tool-profiles"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-11
supersedes: []
---

# graphql-orm-ai-tool-profiles

This package compiles reviewed, bounded GraphQL tool profiles and versioned
subgraph manifests without selecting a `graphql-orm-ai` persistence backend.
It owns the canonical serialized tool, disclosure, operation-binding, profile,
and manifest types re-exported by `graphql-orm-ai`.

Profiles are discovery and static policy only. They do not enable a resolver,
mint authority, perform introspection, execute GraphQL, or grant provider
egress.

Version 0.3.0 uses manifest wire version 2 and recursively canonicalizes JSON
object keys before hashing manifests and tool descriptors. This makes an exact
manifest invariant under router-extension JSON normalization while preserving
array order and every version, schema, descriptor, projection, disclosure,
target, and fingerprint check.

The optional, fingerprinted `AiBrowserResultPreviewPolicy` remains closed by
default. Opt-in policy supplies only classification, byte, record, and depth
ceilings; the backend-enabled runtime must still reauthorize the current owner,
scope, tool, row, and fields before returning a protected least-disclosure
projection.

See the [changelog](CHANGELOG.md) and [migration guide](MIGRATION.md) for the
wire-version and policy-fingerprint transition.
