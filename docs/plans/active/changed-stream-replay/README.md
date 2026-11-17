---
title: Changed stream replay plan
kind: plan
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-17
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
with a cursor still inside retention first receives, in commit order, exactly
the retained events after that cursor that it may read now, then live events.
A cursor outside retention, from another process, or that the server cannot
interpret gets one stable reset-required error and nothing else, after which
the client re-reads as it does today. Entities without the opt-in, and
subscribers that send no cursor, are unchanged. The generated stream is
described in the semantic catalogue as `ReplayThenLive`, the contract custom
subscriptions already use, and the router passes cursors and the reset error
through unchanged.

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

- **#666 (open): key-only deletes.** Replay must define what a replayed
  delete carries when no preimage grant exists. It edits the same generated
  `on_changed` bodies and event types, so it lands first.
- **#387 (closed): privacy of lag caused by hidden writes.** 0.164.0 holds a
  lag error until the next independent authority tick. Replay must keep that
  property: whether a cursor resets must not depend on hidden events in a way
  the subscriber can observe more precisely than the tick.
- **#341: the started signal** (opt-in `Started: true` frame, ORM/macros
  0.172.0). After a replay the server sends one "now live" marker; the plan is
  to reuse that frame shape rather than add a second one.
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
  covers only journaled tables and has open coverage gaps (#592); it may
  become a later retention source.

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

### Cursors

- A cursor is sealed with an XChaCha20-Poly1305 key, as authorized scan
  cursors are (`AuthorizedScanConfig`), and binds the entity, the process
  epoch, the sequence number, and a digest of the subscription's arguments
  (filter, `Key`/`Keys`), so it cannot be moved to another stream.
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
  authority. A row that has since changed or been deleted is re-read; what a
  replayed delete carries follows #666.
- Replay is bounded by the ring caps and by a per-subscription work limit, so
  one reconnect cannot cost more than one ring scan.
- After the replayed events, one "now live" frame (the #341 started frame).

### Reset

- A cursor older than the oldest retained event, from another epoch, sealed
  with another key, or malformed ends the stream with one error carrying the
  reset-required code and nothing else.
- To keep the #387 property, the decision is made from the cursor and the
  ring bounds only, never from how many hidden events lie after the cursor,
  and the reset error is released no earlier than the next authority tick,
  as the lag error is.

### Schema and catalogue

- Opt-in per entity (attribute name to be chosen in review). The opted-in
  root gains `After: String`, the event gains a nullable `Cursor: String`, and
  the semantic catalogue reports `ReplayThenLive` with the ring caps as its
  bounds. Default SDL, catalogues and fingerprints are unchanged.

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
- The router passes cursors, the "now live" frame and the reset error through
  unchanged, tested on the router's WebSocket gateway.
- Reference documentation states "same process, bounded" and the reset
  contract, and the new code is in the stable error-code reference.

## Current checkpoint

Design written; no code. Next boundary: #666 lands, then the maintainer
reviews this design (retention caps and defaults, the attribute name, the
reset code name) before an implementation PR on the ORM/macros line.
