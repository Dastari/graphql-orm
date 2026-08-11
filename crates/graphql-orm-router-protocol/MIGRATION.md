---
title: graphql-orm-router-protocol migration guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router-protocol migration guide

## Crate 0.1.0 to 0.2.0 (protocol major remains 1)

`SubgraphDescriptor` adds an optional `extensions` vector. Constructor users
can call `SubgraphDescriptorBuilder::extension`; direct Rust struct literals
must supply `extensions: Vec::new()`. Existing JSON payloads omit the field and
remain compatible. Empty extensions preserve the prior combined fingerprint.

An extension producer constructs `DescriptorExtension::new(name, version,
payload)`. The protocol owns canonical ordering, size/identity validation, and
fingerprinting but does not interpret payloads. A consumer that understands a
named extension must decode its inner version itself and reject unsupported or
incomplete payloads. The router treats unknown optional extensions as inert,
fingerprint-bound metadata.

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
