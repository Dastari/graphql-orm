---
title: "graphql-orm-macros"
kind: reference
status: active
owner: graphql-orm-macros-maintainers
last_reviewed: 2026-10-29
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm-macros`

Version 0.81.0 remains paired with the runtime actor-stamp contract.

Version 0.81.0 adds opt-in `int64` static filters with native `i64` operands,
`Signed64` wire coercion and exact typed metadata. Existing `number` filters
remain 32-bit. See the [signed64 migration](../../MIGRATION.md#0800-to-0810-exact-static-signed64-filters).
