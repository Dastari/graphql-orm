---
title: Changed stream replay plan
kind: plan
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-23
review_by: 2027-02-15
supersedes: []
---

# Changed stream replay

Design plan for [#668](https://github.com/Dastari/graphql-orm/issues/668):
an opt-in event cursor and bounded replay for generated `<Entity>Changed`
streams. This plan records the design and remaining review gates.

## Outcome

An entity that opts in gives each delivered `Changed` event an opaque cursor,
and its `Changed` root accepts an `After` cursor. A subscriber that reconnects
with a cursor still inside retention first receives, in the order this process
published them, exactly the retained events after that cursor that it may read
now, then live events.
A cursor outside retention, from another process, or that the server cannot
interpret gets one stable reset-required error and nothing else, after which
the client re-reads as it does today. Entities without the opt-in, and
subscribers that send no cursor, are unchanged. The replay window is published
in the entity's client metadata. The semantic catalogue keeps describing the
stream as `BestEffort` (see "Schema and catalogue" for why), and the router
passes cursors and the reset error through unchanged.

## Non-goals

- Durable or cross-replica replay in the first release. Retention is an
  in-process, bounded ring per entity: a restart, or a reconnect that lands on
  another replica, always resets. This is documented as "same process,
  bounded".
- Replaying with the authority the subscriber had when the event was written.
  Every replayed event is authorized again with current authority.
- Exposing per-event counts or timing for events hidden from the subscriber.
  The capacity reset discloses the aggregate retention gap described below.
- Exactly-once delivery across a reset. After a reset the client re-reads.
- Replay for runtime collections or for custom subscriptions, which already
  have an application-supplied `ReplayThenLive` source.
- A snapshot of the data at the cursor position. Replay delivers retained
  change events, not historical row states (see #673 for multi-page reads).

## Dependencies

- **#666: key-only deletes.** Settled as option A and merged into
  `next/breaking` by #741: the `key_only_deletes` entity opt-in sends a
  key-only `DELETED` only to `Key`/`Keys` subscribers of that exact key. The
  issue stays open until the breaking batch reaches `main`. Replay edits the
  same generated `on_changed` bodies, so it is implemented on top of that
  code: after the batch reaches `main`, or on `next/breaking`. A replayed
  delete follows exactly the live rule (see "Replay").
- **#387 (closed): privacy of lag caused by hidden writes.** 0.164.0 holds a
  lag error until the next independent authority tick. Replay must keep that
  property: whether a cursor resets must not depend on hidden events in a way
  the subscriber can observe more precisely than the tick.
- **#341: the started signal** (opt-in `Started: true` frame, ORM/macros
  0.172.0). After a replay the server sends one "now live" marker; the plan is
  to reuse that frame shape rather than add a second one. The started frame
  type exists only on entities with `changed_started = true`
  ([Entity attribute parsing](../../../../crates/graphql-orm-macros/src/entity.rs),
  [Generated Changed code](../../../../crates/graphql-orm-macros/src/operations.rs)), so the replay
  opt-in requires `changed_started` and the derive refuses it otherwise.
- **#208 (open): external observation adapter.** The multi-replica retention
  source of a later phase; it is not needed for the in-process ring.
- **#548 (open): router loss hook.** The router keeps its own continuity
  epoch and sends "resubscribe and refetch"; the reset-required code and the
  router's wording must agree.
- **Stable code vocabulary.** `SUBSCRIPTION_REPLAY_RESET` is in the
  [stable error-code reference](../../../reference/graphql-orm/error-codes.md#one-vocabulary-with-the-router).
- Replay is independent of the durable change journal, which covers only
  journaled tables and may become a later retention source.
  [Journal implementation](../../../../crates/graphql-orm/src/graphql/orm/backup.rs)

## Design

### Retention

- One ring per opted-in entity and `Database` handle family holds internal
  change witnesses. A ring retains up to 128 events and 2 MiB of accounted
  witness storage. It evicts the oldest entries first.
- The ring records a sequence number per event and a random ring identity.
  The first ring attached to a new handle family starts a fresh cursor scope.
- Events and cursors age out after two minutes. Generated witnesses retain
  row data only for deletes. The Changed reference describes when idle rings
  prune retained delete preimages.
  [Bounded replay](../../../reference/graphql-orm/macros-and-attributes.md#bounded-replay)

### Sequence and ordering

- The ORM assigns sequence numbers in publication order. Generated writes
  queue witnesses and publish them after commit on SQLite, PostgreSQL, and
  SQL Server.
  [ORM write pipeline](../../../../crates/graphql-orm/src/graphql/orm/core.rs)
- Concurrent transactions can publish in a different order from their commits.
  At delivery, non-delete events re-read the current row by key under the
  current principal.
  [Changed delivery](../../../../crates/graphql-orm/src/graphql/subscriptions/changed.rs)
- The ring appends the event and broadcasts its witness under one lock.
  Subscription attachment uses that same lock to capture the last replay
  position and attach the live receiver. Live delivery checks the sequence and
  ring identity before subscriber filters run.
  [Replay retention](../../../../crates/graphql-orm/src/graphql/subscriptions/replay.rs)

### Cursors

- A cursor is sealed with XChaCha20-Poly1305, as authorized scan cursors are,
  and binds the entity, the process epoch, the sequence number, the
  publication time, the subscriber identity, and the subscription's
  arguments (filter, `Key`/`Keys`). The key is random
  per ring, not `AuthorizedScanConfig`'s host key: retention is per process,
  so no shared key is needed. A cursor from another process fails to open
  and resets. Replay also does not depend on scan endpoints being installed
  ([Authorized scan cursors](../../../../crates/graphql-orm/src/graphql/orm/authorized_pagination.rs)).
- Cursors are opaque and per subscriber. Each delivered event's cursor is the
  ring sequence of that event; the subscriber never sees the sequence numbers
  of events it did not receive, so gaps between its cursors are not visible.
- The interpretation fingerprint (#671) includes
  `CHANGED_REPLAY_CURSOR_FORMAT` for opted-in entities.
  [Interpretation fingerprint](../../../../crates/graphql-orm/src/graphql/orm/client_metadata.rs)

### Replay

- On subscribe with `After`, admission and authorization run exactly as for a
  live subscription. The resolver rechecks authority at the next independent
  tick before attaching the live receiver and starting replay.
- Each replayed event goes through the same release path as a live event:
  provenance, row policy, field policies and key checks with current
  authority. A row that has since changed or been deleted is re-read. A
  replayed delete is released as the live path releases it. With its
  retained preimage it needs a current `can_read_deleted_preimage` grant.
  Otherwise, only when `key_only_deletes` is on and the subscriber names that
  exact key, it is released key-only after the same key-field checks.
  Otherwise nothing is sent.
- Replay is bounded by the ring caps and by a per-subscription work limit, so
  one reconnect cannot cost more than one ring scan.
- After the replayed events, one "now live" frame (the #341 started frame).

### Reset

- A cursor older than the oldest retained event, from another epoch, sealed
  with another key, or malformed ends the stream with one error carrying the
  reset-required code and nothing else.
- To keep the #387 property, the decision is made from the cursor and the
  ring bounds only, never from how many hidden events lie after the cursor.
- A reset decided by age alone (the cursor's sealed publication time is past
  the maximum age), or by epoch, key or format, depends only on the cursor
  and the clock. Hidden writes cannot influence it, so it is sent at once.
- A reset caused by count or byte eviction depends on total write volume,
  hidden writes included. That reveals the same bit as lag, so it is
  released no earlier than the next authority tick, as the lag error is
  ([Subscription runtime](../../../../crates/graphql-orm/src/graphql/subscriptions.rs)).
- Accepted residual disclosure. Several reusable cursors can be probed in
  parallel to estimate at tick granularity when total writes after a position
  cross the 128-event bound, including hidden and other-tenant writes. A
  hidden oversized delete preimage or a value whose storage cannot be measured
  clears the ring and can also cause a capacity reset.
- Deferring every reset would make each expired reconnect wait up to the
  30-second default interval plus jitter ([Database handle](../../../../crates/graphql-orm/src/db.rs),
  [Subscription runtime](../../../../crates/graphql-orm/src/graphql/subscriptions.rs)) before the client could refetch.

### Schema and catalogue

- Entities opt in with `changed_replay = true`. The generated root gains `After: String`, and the event gains a nullable
  `Cursor: String`. Default SDL, catalogues and fingerprints are unchanged.
- The semantic catalogue keeps `BestEffort` for generated streams.
  `ReplayThenLive` already has a different, AI-facing meaning:
  - validation requires a bounded single observation
    (`maximum_duration_seconds` and `maximum_events`,
    [Semantic catalogue validation](../../../../crates/graphql-orm-operation-catalog/src/semantic.rs));
  - AI tool profiles export every `ReplayThenLive` root as a subscription
    capability ([AI subscription export](../../../../crates/graphql-orm-ai-tool-profiles/src/query_plans.rs));
  - the AI runtime refuses such a root unless a replay source is registered
    ([AI subscription waits](../../../../crates/graphql-orm-ai/src/subscription_waits.rs)).

  Reporting it here would silently change AI hosts' capability sets.
- The replay window (scope "process", event, byte and age caps) goes in a
  new optional field of `GraphqlChangedStreamMetadata`. That struct is
  `#[non_exhaustive]` ([Client metadata types](../../../../crates/graphql-orm-operation-catalog/src/semantic/client.rs)),
  so the field is additive in Rust. In JSON the field is omitted for
  entities without the opt-in. The struct uses `deny_unknown_fields`, so an
  older reader refuses only metadata from opted-in entities.

### Alternatives rejected

- **Database-assigned sequence.**
  - SQLite would give commit order through a counter row, because it has
    one writer.
  - PostgreSQL sequences are taken before commit and commit out of order
    ([Journal implementation](../../../../crates/graphql-orm/src/graphql/orm/backup.rs)). Commit order
    there needs a per-entity lock row held to commit, which serializes all
    writes on the entity.
  - SQL Server `rowversion` has the same problem; Change Tracking needs
    database-level configuration.
  - It costs a write per event and still needs re-authorization at release,
    so it buys nothing for an in-process ring.
- **The change journal as the source.** It lacks MSSQL, covers only
  journaled tables, and PostgreSQL export waits for open writers (up to
  30 seconds by default, [Journal implementation](../../../../crates/graphql-orm/src/graphql/orm/backup.rs)). It remains a possible later
  durable source.
- **Replaying stored payloads.** This would release rows as they were rather
  than as the subscriber may read them now. Re-reading by key is already how
  live release works.

## Acceptance gates

- A subscriber that reconnects with its last cursor inside retention receives
  exactly the events it missed and may read now, in order, then the "now live"
  frame, then live events. The test runs on SQLite and test-owned PostgreSQL
  and SQL Server.
- Outside retention, from another epoch, under another sealing key, or with a
  tampered cursor, it receives exactly one reset-required error and no event.
- Row and field policies are re-evaluated for replayed events: an event the
  subscriber could read when written but not now is not replayed.
- A cursor from one subscription is refused by a subscription with another
  filter or key set.
- Cursors carry no readable sequence. A test interleaves writes for two
  subscribers with different visibility, checks their replay and live events,
  and verifies that both capacity resets wait for the authority tick.
- Ring memory stays within its caps under a write burst, and replay work per
  reconnect is bounded.
- Entities without the opt-in serve byte-identical SDL and catalogues
  (`scripts/compare-generated-schema.sh`).
- An opted-in entity's semantic catalogue entry stays `BestEffort`, and AI
  tool-profile subscription capability sets are unchanged by the opt-in.
- An expired-by-age cursor resets at once; a cursor evicted by count or bytes
  resets no earlier than the next authority tick.
- The router passes cursors, the "now live" frame and the reset error through
  unchanged, tested on the router's WebSocket gateway.
- Reference documentation states "same process, bounded" and the reset
  contract, and the new code is in the stable error-code reference.

## Current checkpoint

The independent authorization review confirmed that replay uses the shared
Changed release path. The follow-up checks live witness identity and sequence
before filtering and covers replay visibility, cursor binding, authority
revocation, age, eviction, propagation, and concurrent publication across
the backend scenarios. The
[Changed reference](../../../reference/graphql-orm/macros-and-attributes.md#bounded-replay)
describes the wire contract. The focused live witness continuity checks exercise publications from foreign
rings and repeated witnesses. The Changed reference also documents
[the behavior of a sender shared across handles](../../../reference/graphql-orm/macros-and-attributes.md#bounded-replay).
