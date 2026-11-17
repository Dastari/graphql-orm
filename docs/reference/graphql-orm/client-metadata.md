---
title: Client-facing schema metadata
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-17
review_by: 2027-02-15
supersedes: []
---

# Client-facing schema metadata

A GraphQL client generator can read most of what it needs from the SDL. This
page covers the facts the SDL does not carry, and how the ORM publishes them.
Every item is opt-in. A host that declares none of them serves the same SDL,
operation catalogue and semantic catalogue, with the same fingerprints, as
before ORM/macros 0.172.0.

| Fact | Issue | Delivered in | Opt-in |
| --- | --- | --- | --- |
| Fields a generated `Changed` stream can carry | #340 | client metadata export | `schema_roots!(client_metadata: true)` |
| Expiry field and its time encoding | #670 | client metadata export | `graphql_entity(lifetime(...))` and `client_metadata: true` |
| Relationship source-to-target key mapping | #672 | client metadata export | `client_metadata: true` |
| A `Changed` stream has started | #341 | served schema | `graphql_entity(changed_started = true)`, then `Started: true` per subscription |
| A generated aggregate's source entity | #372 | served schema | `graphql_entity(aggregate_source = true)` |
| Interpretation fingerprint of a release | #671 | host-served value | none; `schema_roots!` always emits the function |

None of this metadata grants or changes authority. The server never enforces
an expiry time, and a batched relationship read is authorized as the list read
it is (see [relationship key mappings](#relationship-key-mappings)).

## Client metadata export

`schema_roots!(client_metadata: true, ...)` emits
`graphql_orm_client_metadata() -> &'static GraphqlClientMetadata`, a companion
document of `graphql_orm_semantic_catalog()`. It is defined in
`graphql-orm-operation-catalog` 0.8.1 as a separate, versioned document
(`GRAPHQL_CLIENT_METADATA_VERSION` 1) with its own fingerprint
(`graphql-orm-client-metadata-canonical-json-sha256-v1`). It is bound to the
semantic catalogue by `semantic_catalog_fingerprint`, and it names only public
GraphQL identities that the bound catalogue publishes: no table, column, Rust
path or condition. The semantic catalogue, its wire version 3 and its
fingerprint are unchanged.

Export it at build time beside the semantic catalogue, for example from a
small binary or example in the service crate:

```rust,ignore
fn main() {
    let semantic = my_service::graphql_orm_semantic_catalog();
    let client = my_service::graphql_orm_client_metadata();
    std::fs::write("semantic-catalog.json", serde_json::to_vec(semantic).unwrap()).unwrap();
    std::fs::write("client-metadata.json", client.export_json().unwrap()).unwrap();
}
```

`export_json` writes canonical JSON (sorted keys), so the bytes are stable for
the same schema. A generator merges one export per subgraph.
`GraphqlClientMetadata::from_export_json` decodes and validates one, and
`validate_against(&catalog)` checks it against the catalogue it claims to
describe. Router-served delivery, as a descriptor extension composed by the
router, is a later change.

The document holds one entry per entity with at least one fact, ordered by
entity name:

```json
{
  "entity_name": "Child",
  "changed_stream": {
    "root_field": "childChanged",
    "carried_fields": ["id", "parentId", "validUntil"],
    "identity_carried": true,
    "started_signal": false
  },
  "lifetime": { "field": "validUntil", "encoding": "sqlite_utc_text" },
  "relationships": [
    {
      "field_name": "parent",
      "target": "Parent",
      "key_mapping": {
        "mode": "batchable",
        "source_fields": ["parentId"],
        "target_fields": ["id"],
        "target_filterable": true
      }
    }
  ]
}
```

### Changed stream carried fields

`carried_fields` lists, in the entity's field order, exactly the fields the
subscribe-time selection check admits under the event's entity field: the
persisted scalar fields that are public, readable and not `subscribe = false`,
and generated identity fields such as `NodeId` where the semantic catalogue
lists them. The list is
computed by the same rules as that check, so the two cannot differ. Selecting
any other field (a relation, a computed field, a `subscribe = false` field)
refuses the whole stream, with `INVALID_INPUT`, or with
`SUBSCRIPTION_SELECTION_UNSUPPORTED` when the host opts into the dedicated
code (ORM/macros 0.171.0, see [stable error codes](error-codes.md)). `identity_carried` says whether the event's `Id`
(and `Key`, for a composite key) may be selected.

A carried field with a field policy is listed. An event whose row the
subscriber may not read in that field is not delivered to that subscriber, so
a client that wants every event should select only fields without a policy;
`has_field_policy` in the semantic catalogue says which ones have one.

### Lifetime fields

```rust,ignore
#[graphql_entity(table = "sessions", plural = "Sessions",
    lifetime(field = "expires_at", format = "unix_seconds"))]
struct Session { /* ... */ expires_at: Option<i64> }
```

`field` names the Rust field. It must be a public, readable, persisted,
non-list scalar field with no `read_policy` and not classified `secret`; the
derive refuses any other field. A null value means the record does not expire.
Every encoding is UTC:

| Field type | `format` | Published `encoding` |
| --- | --- | --- |
| `DateTime<Utc>` | omitted or `rfc3339_utc` | `rfc3339_utc` |
| `i32`, `i64`, `u32` | `unix_seconds` (required) | `unix_seconds` |
| `i64` | `unix_milliseconds` | `unix_milliseconds` |
| `String` | `rfc3339_utc` or `sqlite_utc_text` (required) | as declared |

`sqlite_utc_text` is `YYYY-MM-DD HH:MM:SS` with optional fractional seconds
and no offset. `Entity::graphql_lifetime()` returns the declaration.

Non-goal: the server does not enforce expiry. An expired row stays readable,
listable and subscribable until it is written or deleted, and `Changed` does
not fire when a clock passes the value.

### Relationship key mappings

Each public relationship field of an entity reports one `key_mapping`:

- `batchable`: equal values of `source_fields` (on the owning entity) and
  `target_fields` (on `target`), position by position, reproduce the
  relationship. Composite keys list every component in key order.
  `target_filterable` is true when every target field accepts both `Equal` and
  `In` in the target's generated `WhereInput`; a composite batch combines one
  `And` of equalities per parent inside an `Or`. The target list root's page
  maximum is published in the semantic catalogue (`schema_roots!(max_page_limit)`),
  and a batch must stay within it.
- `not_batchable` with `reason: "conditional"`: the relationship has a source
  or target condition, so a key read alone would return other rows. The
  condition is never published.
- `not_batchable` with `reason: "field_policy"`: the relationship field has a
  field policy, which a read of the target's list root does not apply.
- `no_exposed_key`: a key field on either side is not a public field, has a
  field policy, is classified `secret` (for example `#[graphql_orm(sensitive)]`),
  or the target entity is not in this schema. No key name is published, and a
  client is never told to batch with secret values.

A batch read through the target's list root is authorized as a list read of
the target: the list root's entity, row and field policies apply, not the
relationship traversal's. The results are equivalent only when both surfaces
apply the same policy. Since 0.129.0 a relationship traversal also enforces the
target's operation scopes; a host whose list root and relationship differ in
policy should not batch.

## Started signal on `Changed` streams

`#[graphql_entity(changed_started = true)]` adds a `Started: Boolean` argument
to the entity's `<Entity>Changed` root. A subscriber that passes `true`
receives one frame with `Started: true` before any event. The frame is sent
after the subscription is admitted and authorized and after its event
receiver is subscribed, so every change committed after the frame is
delivered to that subscriber, subject to the usual per-event checks. It is not
a replay: changes committed before it are not sent. A subscriber that omits
the argument, or passes `false`, receives exactly the frames it received
before.

The started frame carries no change: `Started` is true, `Path` is empty and
every other field is null. To allow that, the opted-in entity's
`<Entity>ChangedEvent` gains `Started: Boolean!` (false on every event), and
its `Action`, `ChangeKind`, `Id` and (composite keys) `Key` fields become
nullable. Events still fill them. Entities without the option keep their
existing types. The frame is an ordinary subscription payload, so the router
passes it through unchanged. Lag and authority ends are unchanged
(`SUBSCRIPTION_CONTINUITY_LOST`, `SUBSCRIPTION_SOURCE_CLOSED`, `FORBIDDEN`);
the signal adds no error code.

Two shapes that keep the event's fields non-null were considered and
rejected. A union of the event and a started type would change the root's
result type, so every existing document selecting the event's fields
directly would stop validating. A separate opt-in root that streams frames
would add a second subscription root per entity, with its own catalogue,
authorization and root-collision entries, and duplicate the generated stream
body. Loosening nullability on opted-in entities keeps every existing
document valid, and subscribers that never pass `Started: true` never receive
a null.

## Aggregate source entity

`#[graphql_entity(aggregate = true, aggregate_source = true)]` makes the
entity's aggregate root return `[<Entity>AggregateResultRow!]!` in place of
the shared `[GraphqlAggregateResultRow!]!`. The row has the shared row's
`Groups` and `Metrics` fields and a nullable `SourceEntity: <Entity>` field.
`SourceEntity` always resolves to null. Its type is the contract: from a
pulled schema, introspection, or a composed supergraph, a client reads the
entity an aggregate root reads from the type of that field, without matching
input type names. Two aggregated entities in different subgraphs each name
their own type. The semantic catalogue already names the entity as
`generated_entity_name` of the aggregate operation.

`aggregate_source` requires `aggregate = true`. The row type is specific to
its entity and is not `@shareable`; the shared row type keeps its
`@shareable` declaration.

## Interpretation fingerprint

Every `schema_roots!` emits:

```rust,ignore
pub fn graphql_orm_interpretation_fingerprint(salt: Option<&str>) -> String
```

It returns lowercase hex SHA-256 over canonical JSON of exactly these inputs:

1. `GRAPHQL_ORM_INTERPRETATION_ALGORITHM`
   (`graphql-orm-interpretation-sha256-v1`) and
   `GRAPHQL_ORM_CURSOR_FORMAT_VERSION` (1). The cursor-format version covers
   offset cursors, keyset cursor plaintext and sealing, composite key inputs
   and global `NodeId` values. A release that changes how any existing one is
   read increments it.
2. The backend dialect name (`sqlite`, `postgres` or `mssql`).
3. For each entity, sorted by entity name, so reordering `entities:` does not
   change the value:
   - the entity name. `NodeId` values encode it with the key values
     (`encode_node_id`), and keyset cursors are bound to it, so it is the
     identity both use. `stable_type_id` is not an input, because neither
     encoding uses it;
   - whether it serves `NodeId`;
   - the keyset order fingerprint its keyset cursors are bound to
     (`AuthorizedKeysetEntity::KEYSET_FINGERPRINT`): every order column, with
     its direction and null placement, primary-key suffix included. The
     derive computes both from one function, so they cannot differ;
   - its effective default sort (`DatabaseEntity::DEFAULT_SORT`, the declared
     `default_sort` or the primary-key default), which orders offset pages;
   - each primary-key field, in key order: its public GraphQL name, logical
     value kind and column type.
4. The SHA-256 of the host salt, when one is given.

The value is the same across restarts and replicas of one build and
configuration, and changes when any input changes. It contains no secret,
host name, path or version string: names and sort text enter only through the
hash, and only a digest of the salt does.

It does not see a change that keeps these inputs, such as a row policy that
now hides rows a cursor pointed at. The sealing key of `AuthorizedScanConfig`
is not an input; rotating it invalidates sealed scan cursors, so pass a salt
that changes with it (for example a key version, never the key). A host that
wants a value per deployment can pass its own build identifier as the salt,
at the cost of discarding client state on every deploy.

The ORM does not serve the value itself. A host serves it as the HTTP response
header `GRAPHQL_ORM_INTERPRETATION_HEADER` (`graphql-orm-interpretation`) and
the `connection_ack` payload member `GRAPHQL_ORM_INTERPRETATION_ACK_FIELD`
(`graphqlOrmInterpretation`). Composing the subgraphs' values in the router
and serving them from it is a later router change. Serve the value under the
same gate as introspection if a deploy-change signal should not be public.

## Consistent multi-page reads

The ORM has no snapshot token for multi-page reads (#673). Each page is an
independent read; see
[consistency and backend boundaries](pagination-migration.md#consistency-and-backend-boundaries).
