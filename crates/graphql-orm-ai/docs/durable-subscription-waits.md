---
title: "Durable bounded subscription waits"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-01
supersedes: []
---

# Durable bounded subscription waits

Durable waits are one-shot observations over a canonical semantic GraphQL
subscription. They are not background agents. The provider supplies only the
closed typed plan compiled by `AiGraphqlSubscriptionCapabilityCatalog`:
arguments, an explicit event projection, an optional admitted top-level
condition, a positive timeout and a positive event ceiling. The server owns
the subscription document, variables, target and all fingerprints.

Only semantic operations advertising `ReplayThenLive` are eligible. A host
registers an `AiReplayableSubscriptionSource` under the exact target and
semantic-operation fingerprint. Source registration is routing metadata, not
authority. Best-effort generated broadcast subscriptions remain ineligible
because process or network loss could silently miss an event.

`OrmAiSubscriptionWaitService::register_wait` captures the replay position,
protects the exact plan/cursor/continuation and parks the run at a chained
`subscription_wait_parked` checkpoint. A waiter worker processes at most one
source item per short claim. It rehydrates the credential-free
`PrincipalReference`, checks current session/scope, target and rule policy,
and asks the source to authorize the exact projected event. Nonmatching cursor
progress, or a matching/event-limit outcome plus the existing run-queue
continuation, commits atomically.

Wrap the ordinary checkpoint adopter with `AiSubscriptionCheckpointAdopter`.
Before the next provider call it reopens the protected outcome, recompiles the
capability, rechecks current principal/rules/target/source event and egress,
then consumes the adoption once. Stop/cancellation closes the waiter and its
tool step atomically. Timeout and event-limit are ordinary typed tool outcomes;
retention reset becomes `RecoveryRequired`.

No browser API exposes cursors or protected event payloads. Portable backup
redacts the plan, cursor and outcome; portable restore must quarantine such a
wait as `RecoveryRequired`. Same-database restart may reclaim an exact valid
waiter. Recurring monitors, arbitrary predicates, model-authored GraphQL and
indefinite autonomous loops are outside this contract.
