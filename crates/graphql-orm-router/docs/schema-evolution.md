---
title: graphql-orm-router schema evolution guide
kind: runbook
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router schema evolution guide

Use expand–migrate–contract across independently deployed subgraphs.

1. Add nullable fields/types and compatible resolver behavior first.
2. Publish the finished SDL and the matching protocol authorization catalogue
   from the same application release.
3. Wait until router status shows the new active fingerprint on every intended
   router instance.
4. Migrate clients and data.
5. Remove old fields only after usage has ceased and a composed candidate has
   been checked in the deployment environment.

SDL and authorization policy are one candidate. Publishing only one side may
produce a stale or incomplete descriptor, which is rejected without changing
the active graph. A router allow remains advisory; deploy the authoritative
subgraph guard with the field.

An incompatible update records a rejected candidate while every request keeps
using the exact previous executable graph. Temporary disappearance never means
removal. Topology removal is an explicit authenticated operation and is itself
composed before activation.

Ordinary HTTP requests retain the graph selected at request start. Existing
subscriptions do not migrate silently: retirement emits
`SUBSCRIPTION_SCHEMA_RELOAD`, completes the operation, and requires a new
connection/subscription. Rollouts must retain old resolver behavior long enough
for bounded HTTP work to drain and clients to reconnect.

Rollback by restoring a previously compatible SDL and matching descriptor, then
triggering or awaiting refresh. Graph versions are process-local and monotonic;
compare fingerprints across restarts and instances rather than version numbers.
