---
title: "Migration Guide"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-29
review_by: 2027-02-01
supersedes: []
---

# Migration Guide


## 0.80.0 to 0.81.0: exact static signed64 filters

Use `#[filterable(type = "int64")]` on `i64` or `Option<i64>` fields and
`Int64Filter` with native `i64` comparison/list operands. Existing `number` and
`IntFilter` retain their 32-bit input contract. Upgrade ORM and macros together.
No table or stored-value migration is required.

The `Signed64` scalar accepts exact integer literals and canonical decimal
strings in the inclusive signed64 range; output is a decimal string. SDKs must
map it to bigint or decimal strings, never floating point. Fractions, exponents
in strings, leading zeroes and overflow refuse. GraphQL JSON numeric variables
must reach the parser exactly; use decimal strings in JavaScript clients.
This is the filter operand scalar, independent of runtime `RuntimeInt64`.

All nine operators are supported. Empty in/notIn lists add no constraint (the
existing static SQL convention); comparisons and nonempty lists yield SQL UNKNOWN for NULL, including under
NOT/OR/AND, and only the final match boundary excludes UNKNOWN.
`isNull` composes with other operators using AND. Each list has a 256-member
bound, checked on borrowed raw objects at actual configured operator names
before cloning/decoding list elements, and recursively for native Rust callers.
GraphQL singleton-list coercion remains supported. Existing query-level
policy, depth, work and database bind budgets remain required. Generated
metadata advertises `Signed64` operands rather than GraphQL `Int`.

Generated list, keyset and authorized-scan GraphQL resolvers now check read
policies for every field referenced by a static filter, including nested
NOT/OR/AND, before list/count/scan SQL. This applies to all static filter
families. A predicate cannot bypass a field policy by selecting only another
field in the response. Providers receive the original public field identity
and policy key with no row (`record = None`), and must authorize that field's
use in a predicate; row and selected-field authorization remain independent.
Previously accepted filtered reads may now return forbidden or a missing-policy
error. No additional client scope or read authority is implied.
