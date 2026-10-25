---
title: "Owned runtime schema IR"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-25
review_by: 2027-02-01
supersedes: []
---

# Owned runtime schema IR

The `runtime_schema` module provides an owned, backend-neutral schema representation for hosts
that load collection definitions from a durable catalog at runtime instead of (or in addition to)
compile-time derives. All strings are owned — nothing requires `&'static str` — and schema objects
reference each other through distinct stable ID newtypes: `CollectionId`, `FieldId`, `RelationId`,
and `IndexId`.

## Types

- `RuntimeSchema` → `RuntimeCollection` → `RuntimeField` / `RuntimeRelation` / `RuntimeIndex`.
- `RuntimeValueKind`: `boolean`, `integer`, `float`, `string`, `uuid`, `json`, `bytes`,
  `datetime` and `decimal(DecimalDef)`. Date-time is first-class here; backends choose the
  storage representation. See [exact decimals](#exact-decimals).
- `RuntimeDefault`: a `Literal` in the canonical text grammar of the field's value kind, or
  the database-evaluated `CurrentTimestamp`. See [typed literal defaults](#typed-literal-defaults).
- `RuntimeField::format`: an optional owned semantic descriptor. See
  [semantic field formats](#semantic-field-formats).
- `RuntimeField::auto_timestamp`: an optional server-managed creation or modification
  timestamp. See [automatic timestamps](#automatic-timestamps).
- `description`, `classification` and `export` on collections, fields and relations. See
  [descriptions, classification and export](#descriptions-classification-and-export).
- `RuntimeCollection::label_fields`: ordered fields forming one human-readable record label.
  See [label fields](#label-fields).
- Relations hold ordered `RelationKeyPair` lists, so source/target key arity mismatches are
  unrepresentable. Delete behavior (`restrict`/`cascade`/`set_null`) belongs only on the
  foreign-key-enforcing side.
- Collections carry ordered primary keys (composite supported), `composite_unique` groups,
  `append_only`, the opt-in `retention_purge` capability, and a deterministic `default_order`.
  Retention is valid only on append-only collections. Missing `retention_purge` properties in
  format-v1 serialized catalogs default to `false`, so loading an older catalog cannot grant the
  maintenance capability.

## Validation

`RuntimeSchema::validate` consumes the schema and returns either a `ValidatedRuntimeSchema` or
`RuntimeSchemaDiagnostics` containing **every** diagnostic found: duplicate stable IDs (globally,
across all collections), case-folded name collisions, GraphQL grammar and `__` reservation
violations on type, field, and relation names, non-portable physical identifiers (lowercase,
63-byte bound), dangling references, nullable primary keys, duplicate members inside primary
keys/indexes/unique groups/relation key pairs, key-pair type mismatches, misplaced delete
behavior, set-null over non-nullable keys, foreign keys whose referenced target fields are not
provably unique (primary key, unique field, unique index, or composite-unique group), and
defaults that do not fit their field's value kind. Invalid runtime definitions never panic.

Stable IDs are validated at construction, revalidated during `validate`, and enforced during
deserialization (`TryFrom`-backed Serde), so serialized catalog data cannot bypass them. All IR
structs reject unknown serialized properties (`deny_unknown_fields`); a durable catalog format
fails closed on typos. Canonical rendering escapes free-form literal defaults, so hostile values
containing delimiters or newlines cannot make distinct schemas produce identical canonical
bytes.

## Canonical form and fingerprints

Only a `ValidatedRuntimeSchema` produces canonical output:

- `canonical_bytes()` / `fingerprint()` — deterministic rendering including stable IDs, sorted so
  declaration/insertion order never changes the bytes. Two catalogs with identical shapes but
  different stable IDs are different schemas here.
- `structural_fingerprint()` — the ID-free structural schema. Use this to compare a
  static-derived schema with a catalog-loaded one. It is structural-only by design: it covers
  tables, fields, keys, relations, indexes, defaults, append-only/retention semantics, and
  ordering. It does not cover
  authorization policy hooks, backup enablement/ordering/redaction, or relation change
  propagation — conversion fails closed when static metadata carries those semantics, so a
  successful conversion plus equal structural fingerprints cannot silently equate schemas with
  different security or persistence behavior.

- `format_fingerprint()` — only the semantic field format descriptors, keyed by stable
  collection and field IDs. A schema without descriptors has one constant value.

Fingerprints use FNV-1a like the existing schema hashes. They detect structural drift; they are
not cryptographic signatures. The existing `stable_schema_hash`/backup hashing behavior is
unchanged.

## Typed literal defaults

`RuntimeDefault::Literal` keeps one owned text encoding; the field's value kind selects its
grammar. Build literals with `RuntimeDefault::{boolean, integer, float, string, uuid, json,
bytes, datetime}` and read the typed value a backend returns for an omitted create input with
`RuntimeDefault::typed(kind)`.

| Kind | Accepted literal text | Constructor output |
| --- | --- | --- |
| boolean, integer, float, string | unchanged from earlier releases (`i64`/finite `f64` text, e.g. `+5`, `007`, `.5`, `1e5`, `-0.0`) | minimal decimal; floats keep `.0` or an exponent, negative zero becomes `0.0` |
| uuid | unchanged: any `Uuid::parse_str` form, rendered exactly as stored | lowercase hyphenated |
| json | canonical JSON v1 only | canonical JSON v1 |
| bytes | padded standard base64 whose re-encoding is identical, at most 8 KiB decoded | same |
| datetime | `YYYY-MM-DDTHH:MM:SS.ffffffZ`, years 0001–9999 | from `RuntimeDateTime`; parse authored text with `RuntimeDateTime::parse_exact`, which normalizes offsets and rejects sub-microsecond precision |

Canonical JSON v1 is compact text with unique object keys sorted by UTF-8 bytes, minimal string
escapes (`\"`, `\\`, `\b`, `\f`, `\n`, `\r`, `\t`, other controls as lowercase `\u00xx`),
integers in the `i64`/`u64` range in minimal form, and decimals with a fraction but no exponent,
no trailing fractional zero except `X.0`, no negative zero and at most 17 significant digits.
Bounds are 16 KiB, depth 32 and 1024 values. PostgreSQL `jsonb` preserves these spellings, so
SQLite and PostgreSQL return the same typed JSON value.

Every literal accepted by earlier releases is still accepted with identical serialized form,
canonical bytes and physical rendering. A rejected default reports
`RuntimeSchemaDiagnosticCode::InvalidDefault` with `detail:
RuntimeSchemaDiagnosticDetail::Default(RuntimeDefaultProblem)`; neither the message nor the
detail contains the submitted literal. `CurrentTimestamp` remains a database-evaluated
expression and is valid only for datetime and epoch-second integer fields.

## Semantic field formats

A `RuntimeFieldFormat` describes what a stored scalar means — for example an external
resource reference stored as a UUID — separately from its value kind, physical type and
nullability. It is plain owned data:

- `namespace`: `RuntimeFormatNamespace`, two or more `[a-z][a-z0-9_]*` segments separated by
  `.`, at most 64 bytes. The first segment `graphql_orm` is reserved.
- `version`: a positive `u16`.
- `payload`: `RuntimeFormatPayload`, a JSON object of null, boolean, `i64`, string, list and
  object values. Floats are rejected so identity never depends on float formatting. Keys are
  `[a-z][a-z0-9_]*`; bounds are 4 KiB canonical bytes, depth 8, 256 values and 1 KiB strings.
  An empty payload is omitted from serialization.

The ORM validates descriptors, preserves them through serde, validation and field handles
(`RuntimeFieldHandle::format`), and never interprets them. An invalid descriptor reports
`RuntimeSchemaDiagnosticCode::InvalidFormat` with a value-free `RuntimeFormatProblem` detail;
deserialization rejects invalid namespaces, zero versions, float payloads and unknown properties.

Identity rules:

| Identity | Schema without descriptors | Descriptor added, changed or removed |
| --- | --- | --- |
| serialized JSON | byte-identical; `format` is omitted when absent | gains `"format": {…}` |
| `canonical_bytes()` / `fingerprint()` | byte-identical | changes; this is the stale-write binding for handles, records, cursors, GraphQL `expectedSchema` and mutation environments |
| `structural_fingerprint()` | byte-identical | unchanged; descriptors are semantic, not structural |
| physical target hash, plan hash, migration history | byte-identical | unchanged; there is no DDL |

The descriptor joins the ID-bearing canonical line of its field as
`|format="<namespace>@<version>:<canonical payload>"`. Rebuild the runtime mutation environment
for a descriptor-only revision; certification introspects and needs no DDL. Readers older than
0.55.0 reject serialized documents that contain `format`. The serialized format version stays 1.

Runtime GraphQL serves a formatted field only through an explicit plain-scalar allowance or a
reviewed projector; see [formatted fields](runtime-graphql.md#formatted-fields-and-host-projections).

## Descriptions, classification and export

Collections, fields and relations accept one optional public `description`. It is the only
description: runtime GraphQL emits it in SDL and introspection and the runtime semantic
catalogue publishes the same text to AI capability indexes. There is no AI-only description.
Collections also accept a default `classification`; fields and relations accept a
`classification` and an `export` disposition. The types are the catalogue's
`GraphqlSemanticClassification` (`public` < `internal` < `confidential` < `restricted` <
`secret`) and `GraphqlSemanticExport` (`exportable`, `never_export`).

Defaults match the static derives, through `effective_description`,
`effective_classification` and `effective_export`:

| Member | When absent |
| --- | --- |
| description | humanized API name (`createdAt` → `Created At`, `Invoice` → `Invoice`) |
| collection classification | `internal` |
| field/relation classification | the collection's |
| export | `exportable`, or `never_export` when the effective classification is `secret` |

Validation uses the catalogue rule (`validate_semantic_description`): trimmed, non-empty, at
most 1024 bytes and no control or directional-formatting characters (`InvalidDescription`).
A field or relation may only strengthen its collection's classification, and a `secret` member
cannot be `exportable` (`InvalidClassification`). Diagnostics never echo the text.

Identity follows the format precedent. Present members join the ID-bearing canonical lines as
`|description="…"|classification=…|export=…`, so `canonical_bytes()` and `fingerprint()`
change (a classification change must rebind `expectedSchema`, cursors and catalogues). They are
semantic, not structural: `structural_fingerprint()`, the physical target, plan hashes,
migration history and DDL are unaffected. Absent members are omitted from serialization.
Static conversion leaves them absent.

## Label fields

`RuntimeCollection::label_fields: Vec<FieldId>` names, in order, the fields whose values form
one human-readable label per record, for relation pickers, relation cells and model context.
It is a collection member rather than a per-field flag, so one list states both membership and
composition order in a single place. Empty means no declared label, and consumers choose
their own fallback. At most `RUNTIME_MAX_LABEL_FIELDS` (4) distinct fields of the same
collection are allowed. Each must be a scalar other than `json` or `bytes`, with an effective
classification other than `secret` and an effective export of `exportable`. Unknown fields
fail with `UnknownFieldReference`, so removing a labelled field fails validation until the
host removes or replaces the label explicitly. Other violations fail with `InvalidLabelField`.

The member serializes as `label_fields` with `serde(default)` and is omitted when empty, so
earlier documents read unchanged. A declared label joins the ID-bearing collection line as
`|label=<JSON array of field ids in order>`, which changes `canonical_bytes()` and `fingerprint()`. It is
presentation metadata: `structural_fingerprint()`, `format_fingerprint()`, the physical
target, migration plans and DDL are unaffected, so a label-only revision is a metadata-only
change. Runtime GraphQL publishes the labels by API name as
`GraphqlEntitySemanticMetadata::label_fields` in the semantic catalogue. A label with a
projector-served member is not published, because dropping only that member would change
the label's meaning. Readers older than 0.67.0 reject documents or catalogues that declare
a label (`deny_unknown_fields`); format versions are unchanged. Static conversion
leaves the member empty.
## Automatic timestamps

`RuntimeField::auto_timestamp: Option<RuntimeAutoTimestamp>` makes the database clock the
only writer of a `DateTime` field, like Rails/Django `created_at`/`updated_at` or a
PocketBase autodate field. Field names are the host's choice.

| Kind | Insert | Runtime update of the row |
| --- | --- | --- |
| `OnCreate` | set from the database clock | never changed |
| `OnCreateAndUpdate` | set from the database clock | refreshed in the same `UPDATE` statement |

- **Input.** The field is never writable: `runtime_create_request` and
  `runtime_update_request` refuse it with `field_denied` (no value is echoed), explicit
  `Null` included. Runtime GraphQL omits it from `Create<T>Input` and `Update<T>Input`. A
  required (non-null) auto field is never `missing_required`. `RuntimeField::is_client_writable`
  and `RuntimeFieldHandle::auto_timestamp` expose the rule to hosts.
- **Clock.** Writes use exactly the `CurrentTimestamp` expression: on SQLite
  `strftime('%Y-%m-%dT%H:%M:%f000Z', 'now')`, canonical runtime text with millisecond
  resolution; on PostgreSQL `CURRENT_TIMESTAMP` into `timestamptz`. PostgreSQL returns the
  transaction start time, so every write in one transaction stores the same value. SQLite
  reads the clock per statement. Both timestamps of one insert are always equal.
- **Updates.** Every runtime update request writes at least one field, so it always issues
  an `UPDATE` and refreshes `OnCreateAndUpdate` fields, even when no stored value changes.
  The dependent `SetNull` updates of an opted-in delete are row updates too and refresh the
  nulled rows. A rolled-back update leaves the value unchanged. Mutation results and staged
  effects are reread after the write, so they return the stored values.
- **Link-only changes do not count.** A standalone many-to-many link mutation writes rows of
  the link table, not the source row, so it does not touch the source row's timestamps.
  The rule is "the row's own columns were written". Link changes attached to a create or
  update (`RuntimeMutationRequest::with_link_change`) ride on a row write, because an update
  request always writes at least one field, so that write sets or refreshes the timestamps.
- **Physical shape.** The column's effective default is `CurrentTimestamp`
  (`RuntimeField::effective_default`), whether `default` is `None` or `CurrentTimestamp`.
  Writes do not depend on it, because the engine names the column explicitly, but it
  backfills existing rows when the column is added and keeps rows that other writers
  insert consistent. See [runtime migrations](runtime-migrations.md#automatic-timestamps-0750).
- **Validation.** `InvalidAutoTimestamp` with the value-free detail
  `RuntimeSchemaDiagnosticDetail::AutoTimestamp(RuntimeAutoTimestampProblem)` rejects a field
  whose kind is not `datetime` (`not_date_time`), a `generated` field (`generated`), a
  literal default (`literal_default`), and a primary-key member or a relation key on
  either side (`key_field`). Nullable auto fields are valid. Their column accepts NULL,
  but every runtime insert stores a value.

Identity: the member is omitted from serialization when absent, and the field's canonical
line gains `|auto_timestamp=on_create` or `|auto_timestamp=on_create_and_update` only when
present. Schemas without it keep their exact serialized documents, `canonical_bytes()`,
`fingerprint()` and `structural_fingerprint()`. The goldens in
`tests/runtime_compat_goldens.rs` pin this. The flag describes write semantics, so it is
part of both the ID-bearing and the structural forms; `format_fingerprint()` is
unaffected. Readers older than 0.75.0 reject documents that declare it
(`deny_unknown_fields`); the format version stays 1. Static conversion leaves it absent:
derived `created_at`/`updated_at` behaviour is unchanged.

## Many-to-many relations

A relation with `join: Some(RuntimeJoinTable)` links records through an ORM-owned link
table instead of a key column. It has `Many` cardinality, no key pairs, `enforce_foreign_key:
false` and no `on_delete`. `RuntimeJoinTable { physical_table, source, target }` names the
table and maps each primary-key field of the source and target collections, in key order, to a
link column (`RuntimeJoinColumn { field, column }`). Validation (`InvalidJoinTable`) requires:

- the shape above, with the columns covering the complete source and target primary keys;
- distinct, portable table and column names. The table, its generated primary-key constraint
  name (`<table>_pkey`) and its generated target-column index name must not collide with a
  collection table or index name, and must not use the reserved `__graphql_orm_` prefix;
- at most two relations per link table, and two only as exact inverses (target and source
  swapped), so `posts.tags` and `tags.posts` share one table. A self-relation (source =
  target) uses two different columns.

The member is structural: present join relations render `|join=…|join_source=…|join_target=…`
in both the ID-bearing and the structural canonical forms. It is omitted when absent, so other
schemas keep their bytes. `physical_schema` emits one table per link table. Its primary key is
the complete pair, so a link exists at most once. It has two foreign keys with `ON DELETE
CASCADE` and an index on the target columns. Hosts include link tables in their
`ManagedTableSet`; `ValidatedRuntimeSchema::owned_table_names()` lists every owned table.
Adding a join relation plans an additive `CreateTable`; removing one, or removing a linked
collection, plans a destructive `DropTable`. The planner has no table rename. Changing a link
table's `physical_table` is a destructive drop and re-create, so its links are lost; copy them
first if they must survive. Changing a linked collection's `physical_table` re-creates that
table empty and repoints the link table's foreign key, which SQLite applies as a controlled
table rebuild. If links to the old rows exist, the foreign-key check fails and the migration
rolls back. Link writes and reads are described in
[runtime mutations](runtime-mutations.md#many-to-many-links) and
[runtime relations](runtime-relations.md#many-to-many-layers).

## Exact decimals

`RuntimeValueKind::Decimal(DecimalDef)` exposes the static portable decimal: precision
1..=18 and scale ≤ precision, validated by `DecimalDef::new` and on deserialization. It
serializes as `{"decimal": {"precision": 12, "scale": 2}}` (unit kinds stay plain strings)
and renders canonically as `|kind=decimal(12,2)|`; the definition is structural.

| | SQLite | PostgreSQL |
| --- | --- | --- |
| declared type | `INTEGER_DECIMAL(p,s)` (INTEGER affinity) | `NUMERIC(p,s)` |
| stored value | scaled `i64` mantissa (`12.50` → `1250`), as static decimals | native numeric |
| literal default DDL | scaled mantissa (`DEFAULT 1250`) | canonical literal (`DEFAULT 12.50`) |

Values are `RuntimeValue::Decimal(DecimalValue)` and are never rounded: input needing more
scale or precision is rejected, and a stored value outside the definition fails decoding.
Read them with `RuntimeRecord::decimal` (`rust_decimal::Decimal`) or `decimal_value`; there is
no `f64` path.

Text uses one strict grammar everywhere (`parse_runtime_decimal`): optional `-`, digits
without redundant leading zeros, optional `.` and digits; no `+`, exponent, whitespace or
separators. Lossless trailing zeros are accepted (`"1.500"` at scale 2). Output — GraphQL,
JSON records and values, defaults — is `canonical_runtime_decimal`: exactly `scale`
fractional digits, unsigned zero (`"12.50"`, `"-0.05"`, `"0.00"`). JSON carries
`{"decimal": {"value": "12.50", "definition": {…}}}` with the value always a string; parsing
accepts the same strict text or an integer.

Defaults: `RuntimeDefault::decimal(DecimalValue)` or `RuntimeDefault::decimal_text(def,
"12.5")` (canonicalized to `"12.50"`, without a direct `rust_decimal` dependency). A literal
must be the canonical spelling. `CurrentTimestamp` is invalid for decimals. Decimal fields
cannot be primary keys, relation keys or generated (`UnsupportedCapability`).

The SQL Server MONEY domain (`DecimalDef::mssql_money()`, precision 19, scale 4, signed
64-bit mantissa) is static-only and is not a runtime decimal definition. `validate` reports a
runtime field declaring it as `UnsupportedCapability`; `RuntimeSchema::from_static_entities`
reports a static `#[graphql_orm(mssql_money)]` column as `UnsupportedCapability` instead of
converting it; and a JSON `RuntimeValue::Decimal` carrying it is rejected. Runtime decimals
therefore never emit `DECIMAL(19,4)`, `NUMERIC(19,4)` or `INTEGER_DECIMAL(19,4)` for a MONEY
column.

Queries support `eq`, `ne`, `in`, `not_in`, `lt`, `lte`, `gt`, `gte`, `between`, null checks,
ordering and keyset cursors. A changed definition is refused by owned planning; see
[runtime migrations](runtime-migrations.md#exact-decimals-0560).

## Static conversion

`RuntimeSchema::from_static_entities(&[&EntityMetadata])` converts derive-generated metadata into
the owned IR so both worlds converge on one semantic representation. Stable IDs are synthesized
deterministically from physical names (`<table>`, `<table>.<column>`, `<table>.<relation>`);
compare converted schemas by `structural_fingerprint()`, not `fingerprint()`.

To make conversion faithful, static metadata now records three additional facts, emitted by the
derives and available on `ColumnDef`/`FieldMetadata`:

- `api_name` — the public GraphQL field name after rename attributes and case features;
- `is_sortable` — the derive generated an order-input term;
- `is_date_time` — the column is a `#[date_field]`; its `logical_type` remains `String` so backup
  and existing schema hashes are unaffected.

Static decimal columns convert to `RuntimeValueKind::Decimal` with their definition, and their
backend default SQL to the canonical literal.

Static append-only entities declared with `retention_purge = "..."` convert with
`RuntimeCollection::retention_purge = true`; that bit participates in canonical and structural
fingerprints. The policy key itself is deliberately not part of structural IR because policy
providers are host runtime behavior, not database shape.

Declarations using capabilities the IR does not represent yet are reported as
`UnsupportedCapability` diagnostics instead of being silently dropped: spatial columns, full-text
search, partial/GiST indexes, and check constraints. Backup ordering, policy hook names, and
relation change propagation are also deliberately deferred; they are host/back-end concerns that
later slices may add.

Conversion additionally **fails closed** on semantics the IR does not represent: entity
read/write policy hooks, disabled backup or explicit backup export/restore orders, column backup
exclusion/redaction, non-managed schema ownership policies, and upward relation change
propagation all produce `UnsupportedCapability` diagnostics rather than a lossy conversion.

Conversion notes:

- Single `uuid` primary keys are auto-marked `generated` by the derive; composite keys are not.
- To-many relations require the explicit `multiple` relation flag; cardinality is not inferred
  from `Vec<T>`.
- `default_sort` strings must be `column [ASC|DESC]` terms to convert; anything else is an
  `InvalidDefaultOrder` diagnostic.
- Legacy identifier shapes (schema-qualified MSSQL tables like `dbo.Jobs`, PascalCase columns)
  convert into the IR but fail its portable-identifier validation. The IR targets managed
  lowercase schemas; external/legacy read-only schemas are out of its scope and are reported,
  never silently normalized.

## Deferred

Owned values, records, fingerprint-bound handles, projections, and exact SQLite/PostgreSQL row
decoding are available through the [runtime record boundary](runtime-records.md). Runtime select,
filter, deterministic order, bounded keyset execution, and optional exact count are available
through the [runtime query boundary](runtime-queries.md). Relation loading, migration planning from
the IR, transactional outbox integration, writes, and dynamic GraphQL registration remain later
slices.
