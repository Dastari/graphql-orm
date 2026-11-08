---
title: Runtime collection identities and live subscriptions
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-07
review_by: 2027-02-01
supersedes: []
---

# Runtime collection identities and live subscriptions

A runtime-schema host can opt a collection into local global-identity lookup and
bounded live changes. The host supplies current authorization, a committed-event
source and its own transport. No router, journal, replay cursor, cluster fan-out
or server dependency is added. SQLite and PostgreSQL execute these operations.
MSSQL runtime decoding and GraphQL composition remain unsupported and refuse with
`unsupported_backend`; native MSSQL support for derived entities is independent.

## Declarations and storage

Set `RuntimeCollection::node_identity` to
`Some(RuntimeNodeIdentity { version_field })`. The field must already belong to
that collection: non-null, integer, filterable, unformatted, non-generated,
not a key, and without automatic actor/timestamp behavior. String, UUID, integer
and Boolean primary keys, including complete composite keys, use the shared
NodeId encoder. Float identities are refused, as with derived entities.

The existing `id` keeps its name, scalar and meaning. The object additionally
implements `Node` and serves fixed `NodeId: ID!` and `RecordVersion: Int!` fields.
The local Query root has `node(NodeId: ID!): Node`. Hidden, missing, malformed
and mismatched identities produce null, without an existence diagnostic. Field
or operation authorization failures remain ordinary authorization errors,
independent of whether a valid key exists. Aliases and list reads share the
operation's admission and response budgets.

Runtime creates initialize the declared version to 1; every runtime update,
including no-op patches and link writes, increments it in the transaction.
Dependent and attached writes use the same path. Overflow refuses atomically.
Clients cannot supply the managed column in create/update inputs. Existing
`expected: { rowVersion: { eq: "2" } }` conditions compare it atomically; use the
actual version field's API name. Query, mutation and subscription objects all
serve the fixed aliases under the underlying fields' authorization rules.
External writers must advance that same column atomically with their writes,
as with the generated managed-version contract. Global identities must continue
to identify the same logical object; do not recycle a deleted key for a different
object and rely on a previous incarnation's counter for CAS.

Opting in with an existing version column adds no physical column or migration.
If the host needs a new version field, first add/backfill it using the ordinary
owned migration plan. Neither declaration manufactures or silently rewrites
stored data. Both declarations participate in both IR fingerprints and JSON
round-trip. Absent declarations are omitted; legacy canonical bytes, SDL and
catalogs stay unchanged.

For live changes, also set:

```rust,ignore
collection.live_subscriptions = Some(RuntimeLiveSubscriptions {
    subscription_key_limit: 100,
    deleted_images: false,
});
```

Live requires node identity. `subscription_key_limit` is 1–1024. It bounds the
number of entries in the single live root's `keys` list before deduplication;
the effective bound is the smaller of that declaration and
`Database::subscription_key_limit`. A subscription operation selecting any
generated runtime live root must contain exactly one root-field occurrence.
Aliases, repeated response names, named/inline fragments and directive-skipped
fields cannot multiply receivers or key/read budgets. Multiple root occurrences
refuse with `subscription_root_limit_exceeded` before opening any source,
authorizing or reading, independently of key existence. Unused operations are
not counted. An empty list observes nothing. Supply `keys` on each operation;
unkeyed observation and `keys: null` are refused. Single-key lists use the derived
wire scalar (`String`, `Int`, `UUID`, or `Boolean`). Composite lists use
`<Entity>KeyInput`; their event also exposes the complete `<Entity>Key` and the
shared opaque composite `id` representation. Live collections share these typed
key inputs with mutations (`Int`/`UUID` for keys); stored-field inputs and the
existing `id` output keep their runtime scalar types. Initial admission
conservatively budgets the selected delivery shape for every key; visibility
probes themselves load only keys.

## Source and transaction integration

Implement `RuntimeLiveSource::subscribe` with a fresh, independent bounded
`broadcast::Receiver<RuntimeLiveEvent>` for the requested schema and collection.
The adapter is trusted: publish each committed change at most once, only after
confirmed commit, in commit order per collection, without replay. The engine
preserves that order. It rebinds event keys to installed schema handles; an event
for another collection or unregistered key cannot select a different record.

Wire the source into the existing runtime mutation hook. Capture an effect with
`RuntimeLiveEvent::from_effect(schema, effect)`, then publish it through
`MutationContext::defer`:

```rust,ignore
let event = RuntimeLiveEvent::from_effect(schema, effect)?;
tx.defer(move |_| async move {
    let _ = committed_sender.send(event);
    Ok::<(), std::convert::Infallible>(())
});
```

The effect is pending, not commit evidence. Process dependent effects in their
execution order as well. `defer` supplies the existing after-commit/rollback
boundary; the source adapter must also maintain ordering across concurrent
transactions and external writers. Supplying a source does not automatically
publish writes or install a mutation hook. There is intentionally no built-in
producer with a second set of transaction mechanics.

```rust,ignore
let options = RuntimeGraphqlOptions::default().with_live_source(
    source,
    RuntimeLiveProfile::NativeRls,
);
```

Install cursor protection as for other runtime GraphQL compositions, or choose
`RuntimeCursorProfile::Unprotected` explicitly. Point reads have no outer page
cursor; selected nested relation connections retain their configured encryption,
trusted cursor partition and budgets. The host owns WebSocket mounting, socket
limits, identity rehydration and source lifecycle.

The generated root is lower-camel `<Entity>Changed`, with an
`<Entity>ChangedEvent`: `action`, `changeKind`, `id`, nullable `sourceEntity` and
`sourceId`, empty direct `path`, and the nullable lower-camel entity field. New
runtime root/payload names follow explicit runtime naming, independently of derive
case flags; `NodeId` and `RecordVersion` are always fixed. No cross-collection
provenance is disclosed. An opaque host `subscription_root` cannot currently be
combined with generated runtime live roots; composition refuses with
`unsupported_subscription_composition` rather than overwriting fields or risking
a duplicate-field panic in the locked dynamic framework. Existing host-only
subscription roots are unchanged.

## Current authorization and release

`RuntimeReadCheck::purpose` distinguishes `Select`, `NodeLookup`,
`SubscriptionRegistration` and `SubscriptionDelivery`.
`RuntimeReadCheck::operation_purpose` retains that origin even on a
`RelationFilter` target, so indirect filter reads can enforce the same observation
rules. A list read permission need not grant observation permission.
Registration validates all requested fields and every key member. Initial
visibility and every event's reload and final release require BOTH current
`SubscriptionRegistration` and `SubscriptionDelivery` grants. The engine
intersects their row restrictions at every selected layer, including each
purpose's authorized relation-filter predicates; neither purpose can broaden
the other. A key hidden by either rule is
silently admitted and produces no per-row frame, just like a missing key.
Initial probes load keys only, without projectors or related records. Both
purposes can be checked repeatedly without a frame. Registration is not an
enduring allow grant: revoking it also withholds the next queued event.

Deliveries start from fresh grants, reload through the existing authorized
anchored executor, prepare the entire selected payload, and authorize both
purposes again before release. No registration grant or event-time record
becomes a delivery snapshot. For a prepared row or an approved removal, a row
restriction changing between preparation and its final release loses continuity
with `SUBSCRIPTION_CONTINUITY_LOST`, followed immediately by `complete`. Empty
reloads without approved image evidence remain quiet. Other authority errors
and reload failures use that
same terminal, irrespective of whether a key was previously visible. Changes
between events simply apply both fresh restrictions to the next read.
Field or operation denial, including revocation of either purpose, quietly
withholds the event and keeps the stream open. Runtime-IR's host grant/denial API
does not distinguish subscriber-level loss from row-level denial; there is no
key-dependent completion or diagnostic. Reauthorization can permit a later
runtime-IR event. Authorizers must return current decisions, reject stale
database-auth/cursor partitions and verify current principals rather than
retaining an allow decision from registration.

Generated static/derived subscriptions instead use an independent jittered
authority timer: definite subscriber denial ends with sanitized `FORBIDDEN`,
and host failures with `SUBSCRIPTION_CONTINUITY_LOST`. Their per-event subscriber
and row/key/field/provenance denials remain silent; event activity does not
trigger or reset that timer. See [subscription release authorization](entities-and-relations.md#subscription-release-authorization)
for configuration and polling/latency limits.

Current rows hidden from this subscription produce no per-row frame. A reload
that is invisible or missing can be reported as removed (`DELETED`, null entity)
only with an approved captured pre-image, without saying why. Remembered initial
visibility alone never authorizes a removal. An approved image also establishes
prior eligibility when a committed deletion races the initial key read, so that
authorized deletion is not lost. Image-less empty reloads remain quiet.
Unproven hidden/missing rows, other keys, denied fields and revoked authorities do
not yield identifiers, counts or private error details. Repeated field aliases
are included in the final response bound. Already released frames cannot be
recalled; the final authorization/continuity fence is the release boundary.

## Deleted images and native RLS

`deleted_images: true` is a separate opt-in for every removal, including an
update that reloads after a later deletion. Event action alone cannot establish
that the row was not deleted in the meantime. It requires
`RuntimeLiveProfile::ApplicationPolicy { match_profile }` with the existing
`SqliteBinaryCanonical` or `PostgresCUtf8` storage assertion for that backend.
This assertion concerns real storage/collation, not merely the decoder's type.
The engine verifies the captured image's key, loaded fields and current row
predicate through the conservative checked matcher. A loaded SQL NULL in a
nullable selected field is present evidence; an unloaded field is missing
evidence and denies. This applies to both deletes and visibility-loss removals.
Unsupported predicates
(including relation dependencies) and missing evidence refuse the whole event.
`RuntimeReadAuthority::authorize_deleted_image` independently proves all current
application-policy dependencies, including those not expressed in the row
predicate. Its default is denial, and it is checked again before release.

Only transaction-captured authoritative pre-images qualify: use the effect's
pre-image and project policy dependencies in the write grant. No image means no
evidence. The image is never serialized. After approval, a current reload still
wins if the key now identifies a visible record; an old image cannot replace it.

Native PostgreSQL RLS cannot establish deleted-row visibility after deletion.
The default `NativeRls` profile refuses a declaration with `deleted_images: true`
using `unsupported_deleted_image_profile`. With deleted images disabled, it
supports visible current-row events, but no removals or captured-image delivery,
even when a callback would allow it. This is an explicit limitation, not an RLS proof.

## Continuity and generated clients

On overflow, auxiliary preparation-buffer exhaustion, an inconsistent reload,
or a schema-revision mismatch, the next frame contains only a sanitized error
with uppercase `SUBSCRIPTION_CONTINUITY_LOST`, then `complete`. Intentional source
closure uses `SUBSCRIPTION_SOURCE_CLOSED` in the same framing. The host can send
`RuntimeLiveEvent::ContinuityLost` or `SourceClosed`, including before a graceful
restart, a missed batch, restore/reconciliation or a schema revision it alone
can observe. Send the signal even when no row changes. Hard process/network
failure necessarily appears as a transport disconnect.

Continuity is checked during registration, before filtering and after
asynchronous preparation; a queued terminal/revision signal takes precedence
over a coincident policy or reload failure. A
queued terminal signal or detected lag prevents release of the prepared payload.
These operational signals contain no sequence, hidden key or dropped-event count;
wall-clock execution time is not promised to be constant. Neither signal is a
replay cursor. Subscribe again and refetch current authorized state.

The fixed NodeId encoding, `node(NodeId:)`, `RecordVersion`, typed keys and common
Changed fields match the derived client contract. Regenerate schema artifacts and
saved operation catalogs for opted-in collections. Existing runtime scalars and
`id` are unchanged. Use `finish_with_semantic_catalog` for the full composition;
the composer adds shared node/live roots and event/key metadata. NodeId metadata
inherits the strictest key classification/export restriction, and RecordVersion
inherits the version column's restriction. The AI compiler can still compile the
typed list capabilities with both fields. Its current closed planner cannot model
polymorphic Node selection, so the fully described `node` root is explicitly
`NeverExport` for automatic provider capabilities; direct GraphQL lookup remains
available. Runtime entity label/filter semantics are unchanged.
