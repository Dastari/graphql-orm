---
title: Changed stream replay plan
kind: plan
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-19
review_by: 2027-02-15
supersedes: []
---

# Changed stream replay

Design plan for [#668](https://github.com/Dastari/graphql-orm/issues/668):
an opt-in event cursor and bounded replay for generated `<Entity>Changed`
streams. This plan holds the design. Nothing here is implemented yet.

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
- Revealing how many hidden writes happened. Cursors and reset decisions must
  not let a subscriber count or time events it may not read.
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
  (`crates/graphql-orm-macros/src/entity.rs:66-68`,
  `crates/graphql-orm-macros/src/operations.rs:4186-4220`), so the replay
  opt-in requires `changed_started` and the derive refuses it otherwise.
- **#208 (open): external observation adapter.** The multi-replica retention
  source of a later phase; it is not needed for the in-process ring.
- **#548 (open): router loss hook.** The router keeps its own continuity
  epoch and sends "resubscribe and refetch"; the reset-required code and the
  router's wording must agree.
- **Stable code vocabulary.** The reset error needs one new code in the shared
  `extensions.code` vocabulary, proposed as `SUBSCRIPTION_REPLAY_RESET`, in
  the style of `SUBSCRIPTION_CONTINUITY_LOST`. The coordinator reconciles it
  with the stable error-code reference before implementation.
- The durable change journal (`orm/backup.rs`) is **not** a dependency. It
  covers only journaled tables, is compiled only for single-backend SQLite or
  PostgreSQL builds (`crates/graphql-orm/src/graphql/orm/mod.rs:11-15`), and
  has open coverage gaps (#592); it may become a later retention source.

## Design

### Retention

- One ring per opted-in entity, per `Database` process, holding the internal
  event DTO and its release witness, never a serialized payload.
- Hard caps on events and bytes per ring, configured on `Database`, with
  conservative defaults. Eviction is oldest first. A ring that would exceed
  its byte cap drops events rather than grow.
- The ring records a sequence number per event and a process epoch, a random
  value chosen when the ring is created, so a cursor from another process or
  an earlier restart is recognized and reset.
- Retention also has a maximum age (default to be chosen in review, on the
  order of minutes). A retained delete keeps its trusted preimage, the only
  row data a witness carries (`operations.rs:2686-2694` clears
  `event.entity` for every other action), and the age cap bounds how long
  deleted row content stays in memory.

### Sequence and ordering

- A sequence number is assigned in process, when a committed write's event
  is published to the ring. Generated writes queue their events and publish
  them only after the commit returns (`crates/graphql-orm/src/graphql/orm/core.rs:1974-1979`,
  `:2100-2106`). The number therefore means "publication order in this
  process". It means the same thing on SQLite, PostgreSQL and SQL Server.
- That is not database commit order. Two concurrent transactions can commit
  in one order and publish in the other, because each publishes from its own
  task after its commit returns. This matches live delivery today. It is
  safe because a release re-reads the current row by key with the current
  principal (`crates/graphql-orm/src/graphql/subscriptions.rs:343-462`,
  called from `operations.rs:6773`), so a payload always shows current
  state, never the state at the event.
- The ring append and the broadcast `send` happen under one lock, so ring
  order and channel order agree. Each witness carries its sequence number,
  so replay can de-duplicate against live delivery.
- Events come only from writes made through this process's `Database`
  (`crates/graphql-orm/src/db.rs:905-911`), so a process-local ring loses
  nothing that live delivery would have shown.

### Cursors

- A cursor is sealed with XChaCha20-Poly1305, as authorized scan cursors are,
  and binds the entity, the process epoch, the sequence number, the
  publication time, and a digest of the subscription's arguments (filter,
  `Key`/`Keys`), so it cannot be moved to another stream. The key is random
  per ring, not `AuthorizedScanConfig`'s host key: retention is per process,
  so no shared key is needed. A cursor from another process fails to open
  and resets. Replay also does not depend on scan endpoints being installed
  (`crates/graphql-orm/src/graphql/orm/authorized_pagination.rs:1925-1947`).
- Cursors are opaque and per subscriber. Each delivered event's cursor is the
  ring sequence of that event; the subscriber never sees the sequence numbers
  of events it did not receive, so gaps between its cursors are not visible.
- Cursor format changes feed `GRAPHQL_ORM_CURSOR_FORMAT_VERSION`, so the
  interpretation fingerprint (#671) changes with them.

### Replay

- On subscribe with `After`, admission and authorization run exactly as for a
  live subscription, then the receiver is subscribed before the ring is read,
  so no event falls between replay and live delivery. Events seen in both are
  delivered once, by sequence.
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
  (`subscriptions.rs:1208-1219`).
- Deferring every reset would make each expired reconnect wait up to the
  30-second default interval plus jitter (`db.rs:87-88`,
  `subscriptions.rs:1007-1015`) before the client could refetch.

### Schema and catalogue

- Opt-in per entity (attribute name to be chosen in review). The opted-in
  root gains `After: String`, and the event gains a nullable
  `Cursor: String`. Default SDL, catalogues and fingerprints are unchanged.
- The semantic catalogue keeps `BestEffort` for generated streams.
  `ReplayThenLive` already has a different, AI-facing meaning:
  - validation requires a bounded single observation
    (`maximum_duration_seconds` and `maximum_events`,
    `crates/graphql-orm-operation-catalog/src/semantic.rs:1979-1997`);
  - AI tool profiles export every `ReplayThenLive` root as a subscription
    capability (`crates/graphql-orm-ai-tool-profiles/src/query_plans.rs:2809-2827`,
    `:2841`);
  - the AI runtime refuses such a root unless a replay source is registered
    (`crates/graphql-orm-ai/src/subscription_waits.rs:361-366`).

  Reporting it here would silently change AI hosts' capability sets.
- The replay window (scope "process", event, byte and age caps) goes in a
  new optional field of `GraphqlChangedStreamMetadata`. That struct is
  `#[non_exhaustive]` (`crates/graphql-orm-operation-catalog/src/semantic/client.rs:115-132`),
  so the field is additive in Rust. In JSON the field is omitted for
  entities without the opt-in. The struct uses `deny_unknown_fields`, so an
  older reader refuses only metadata from opted-in entities.

### Alternatives rejected

- **Database-assigned sequence.**
  - SQLite would give commit order through a counter row, because it has
    one writer.
  - PostgreSQL sequences are taken before commit and commit out of order
    (`crates/graphql-orm/src/graphql/orm/backup.rs:1341-1356`). Commit order
    there needs a per-entity lock row held to commit, which serializes all
    writes on the entity.
  - SQL Server `rowversion` has the same problem; Change Tracking needs
    database-level configuration.
  - It costs a write per event and still needs re-authorization at release,
    so it buys nothing for an in-process ring.
- **The change journal as the source.** It lacks MSSQL, covers only
  journaled tables, and PostgreSQL export waits for open writers (up to
  30 seconds by default, `backup.rs:146`). It remains a possible later
  durable source.
- **Replaying stored payloads.** This would release rows as they were rather
  than as the subscriber may read them now. Re-reading by key is already how
  live release works.

## Acceptance gates

- A subscriber that reconnects with its last cursor inside retention receives
  exactly the events it missed and may read now, in order, then the "now live"
  frame, then live events; tested on SQLite and test-owned PostgreSQL.
- Outside retention, from another epoch, under another sealing key, or with a
  tampered cursor, it receives exactly one reset-required error and no event.
- Row and field policies are re-evaluated for replayed events: an event the
  subscriber could read when written but not now is not replayed.
- A cursor from one subscription is refused by a subscription with another
  filter or key set.
- Neither cursors nor reset timing let a subscriber count hidden writes; a
  test writes hidden rows between two visible ones and compares what two
  subscribers with different visibility observe.
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

Design written; no code. Revised 2026-11-19 against current source: the
catalogue mode, ordering wording, #666 status, started-frame requirement,
cursor key and reset timing. Next boundary: the breaking batch carrying #666
reaches `main` (or the work targets `next/breaking`). Then the maintainer
reviews this design (retention caps and defaults, the attribute name, the
reset code name) before an implementation PR on the ORM/macros line.
