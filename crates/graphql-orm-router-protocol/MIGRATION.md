---
title: graphql-orm-router-protocol migration guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router-protocol migration guide

Version 0.1 introduces protocol major 1. Producers publish one canonical
`SubgraphDescriptor` from their finished schema and operation catalogue.
Framework users may use `SubgraphDescriptorBuilder`; hand-written services may
construct the same serializable declarations directly.

Readers accept later additive minor versions in major 1 and ignore unknown
fields. Producers must list any semantic a reader cannot safely ignore in
`requiredSemantics`; an unknown required semantic or different major fails
admission. Endpoint strings are advertisements, not trusted deployment
overrides or credential containers.

A future incompatible wire change requires a new protocol major, new golden
fixtures, and explicit migration instructions here. No such transition exists
yet.
