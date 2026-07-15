# Owned runtime schema IR

The `runtime_schema` module provides an owned, backend-neutral schema representation for hosts
that load collection definitions from a durable catalog at runtime instead of (or in addition to)
compile-time derives. All strings are owned — nothing requires `&'static str` — and schema objects
reference each other through distinct stable ID newtypes: `CollectionId`, `FieldId`, `RelationId`,
and `IndexId`.

## Types

- `RuntimeSchema` → `RuntimeCollection` → `RuntimeField` / `RuntimeRelation` / `RuntimeIndex`.
- `RuntimeValueKind`: `boolean`, `integer`, `float`, `string`, `uuid`, `json`, `bytes`,
  `datetime`. Date-time is first-class here; backends choose the storage representation.
- `RuntimeDefault`: portable `Literal` values or `CurrentTimestamp`.
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

Fingerprints use FNV-1a like the existing schema hashes. They detect structural drift; they are
not cryptographic signatures. The existing `stable_schema_hash`/backup hashing behavior is
unchanged.

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
decoding are available through the [runtime record boundary](runtime-records.md). Runtime query
rendering/execution, filters, ordering, pagination, relation batching, migration planning from the
IR, transactional outbox integration, and dynamic GraphQL registration remain later slices; the
IR carries the metadata they will need (`filterable`, `sortable`, `generated`, defaults, ordering)
without implementing those operations.
