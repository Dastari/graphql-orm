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
