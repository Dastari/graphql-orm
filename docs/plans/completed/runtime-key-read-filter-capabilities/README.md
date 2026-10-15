---
title: Runtime key reads and filter matching interface review
kind: plan
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-10-15
review_by: 2026-11-15
supersedes: []
---

# Runtime key reads and filter matching interface review

Requirement A received explicit implementation approval on 2026-10-15. Its
implementation and scoped validation are complete; final stack integration and
release acceptance remain pending. Requirement B also received explicit
implementation approval, using the validated structural Value prerequisite and
excluding the optional raw JSON parser. Its separate implementation and scoped
validation are complete; coherent release acceptance remains separate. This plan addresses only A and B in the [immutable consumer
request](https://github.com/Dastari/digibase/blob/0d26ab1af68b80985cafa4a621fa850eda5afee4/docs/coordination/prompts/0020-graphql-orm-runtime-key-reads-and-record-filter-matching.md).
The reviewed ORM baseline is `4059d10026b9b8d1e9a2be0f4f556df807f77794`,
version 0.48.0. Remote main was independently resolved to that commit.

## Outcome and current boundary

The completed first checkpoint established whether public APIs satisfy each requirement, using
compiled external consumer controls and narrowly scoped expected compile
failures. Separate approvals now authorize A point reads and B checked intent/
matching under the bounded profiles below. B is a distinct additive implementation
on exact A4c; no raw JSON parser or wire-duplicate guarantee is approved.

At the reviewed baseline, owned APIs provided checked complete mutation keys,
projection/row codecs, bounded SQL reads, relation anchor capture, backend auth
context and GraphQL request authority. They do not automatically grant principal authority.

- `ValidatedRuntimeSchema::runtime_key` validates complete key members and
  preserves declared order independently of filterability. `RuntimeKey` retains
  its schema and exposes checked members, but baseline public consumers were
  mutation requests. `runtime_read_request` accepts a `RuntimePredicate`, not a
  key; `runtime_compare` rejects a nonfilterable key field.
- `runtime_read_request_with_relation_keys` captures relation anchors for a
  normal predicate-based read. It does not turn a complete identity key into a
  point-read predicate. Inventing a relation or changing metadata flags is not
  an acceptable substitute.
- Static `authorize_subscription_row<T, B>` and
  `authorize_subscription_keys<T, B>` already perform keyed current SQL reads
  with policy checks. They require compiled `Entity + FromSqlRow<B>` and an
  original GraphQL context; an arbitrary owned `RuntimeRecord` is not that
  compiled entity. Retain these helpers unchanged as prior art.
- Runtime GraphQL's canonical `plan::filter` and `FilterIntent` are private.
  The current public `RuntimePredicate::referenced_fields` reports surviving
  expression dependencies. Empty membership becomes a constant, so it does not
  retain original field/operator authorization intent. The existing GraphQL
  path separately retains that intent; this is not a demonstrated bypass in
  that path.
- No public owned-record predicate evaluator was found. The existing record
  projection/versioned codec should be reused, not replaced.

The public-API probe sources are retained as [library controls and gap probes](probes/lib.rs)
and [positive construction tests](probes/current_boundary.rs). The external
consumer uses only the ORM public dependency, with `runtime-graphql` plus one
of `sqlite` or `postgres`. They test a
valid nonfilterable key versus rejected public equality, empty-membership
intent loss in the dependency accessor, and expected failures for putting a key
in the predicate slot, accessing private canonical lowering, calling a missing
record evaluator and treating `RuntimeRecord` as a static subscription entity.
Expected failures establish specific boundaries, not proof that every possible
API composition is absent. Actual compiler diagnostics and controls are a
required checkpoint before requesting interface approval.

## A: approved and implemented checked point read

Public constructor and asynchronous executor, owned by core `graphql::orm` and
independently usable without `runtime-graphql` or a direct async-graphql dependency:

```text
ValidatedRuntimeSchema::runtime_key_read_request(
    &self,
    key: &RuntimeKey,
    projection: &RuntimeProjection,
    required_row: &RuntimePredicate,
    additional_filter: Option<&RuntimePredicate>,
    limits: RuntimeQueryLimits,
) -> Result<RuntimeKeyReadRequest, RuntimeKeyReadError>

Database<B: OrmBackend + RuntimeRowDecoder>::execute_runtime_key_read(
    &self,
    request: &RuntimeKeyReadRequest,
    auth: Option<&DbAuthContext>,
) -> Result<Option<RuntimeRecord>, RuntimeKeyReadError>
```

The constructor reuses complete-key validation and existing fingerprint,
collection, literal, projection and budget checks. Identity comparisons are
internal checked key identity, not a public Filter grant. Required and optional
predicates retain ordinary filterability restrictions; no raw identifiers or SQL
are accepted. An explicit required predicate is mandatory even when the trusted
host deliberately chooses true. This is structural restriction, not proof that
the host performed principal/field authorization.

The request owns checked handles, key values, predicates and schema reference;
no GraphQL context or caller borrow crosses its lifetime. It must be Send+Sync,
immutable and non-forgeable. Reuse RuntimeKey's retained schema ownership rather
than duplicating key parsing. Any internal refactor must preserve existing
mutation signatures and behavior.

SQL combines every key member, required restriction and optional filter before
I/O under the existing auth-context/native RLS path. Fetch at most two rows to
detect invalid multiplicity, return at most one projected record and never a
hidden identifier. Missing and policy-hidden rows both return None. Projection
is exact; key columns needed internally are not silently disclosed. No paging
scan, mutation-as-read, retry, write, journal or migration is introduced.

Use a new bounded error wrapper rather than changing an existing exhaustive
public enum without a separate compatibility decision. Safe categories:
invalid_handle, invalid_key, invalid_predicate, invalid_projection, limit_exceeded,
unsupported_backend, unexpected_multiplicity, backend_failure. Backend detail and
values remain redacted; preserve current safe sources for server diagnostics.
All schema/shape/capability failures occur before I/O. Cancellation inherits the
existing read future/connection cleanup contract; no automatic retry or stronger
transactional consistency is promised.

The host example must first authenticate and authorize requested fields/key-read
intent, then supply an explicit required row restriction and coherent auth
context. A failed/missing authority must deny before calling the executor.
The low-level API cannot infer tenant or entity policy from an owned schema.
Static current-authority wrappers remain separate, unchanged host integration.
SQLite and PostgreSQL use existing runtime row decoding. MSSQL must fail its
existing unsupported decoding capability before I/O; A adds no live
MSSQL runtime decoding support.

## B: approved checked intent and record semantics

Review as smaller pieces: canonical lowering plus preserved intent; independently
reviewed structural matching. Exposing lowering alone does not solve matching.

```text
runtime-graphql:
prepare_runtime_filter(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    input: &graphql_orm::async_graphql::Value,
    limits: RuntimeGraphqlLimits,
) -> Result<CheckedRuntimeFilter, RuntimeFilterError>

core graphql::orm:
CheckedRuntimeFilter::predicate(&self) -> &RuntimePredicate
CheckedRuntimeFilter::visit_intent(&self, visitor: &mut dyn RuntimeFilterIntentVisitor)
CheckedRuntimeFilter::prepare_match(
    &self, profile: RuntimeMatchProfile,
) -> Result<RuntimeRecordMatcher, RuntimeFilterError>
RuntimeRecordMatcher::matches(
    &self, record: &RuntimeRecord,
) -> Result<bool, RuntimeFilterError>
```

Use the current canonical GraphQL lowering internally; do not copy a second
parser. The immutable artifact retains schema/collection, bounded complete
original Boolean shape and field/operator intent before constant folding,
including duplicates and empty logical/membership nodes. Intent visitor events
must include enter/leave Boolean node and each original field/operator
occurrence, so hosts cannot authorize only surviving dependencies. It need not
expose private predicate AST or values. The host must authorize all intent before
protected SQL or record disclosure. No public deserialize/unchecked constructor
or wrapper around an already-folded predicate may fabricate missing intent.

Core construction (no GraphQL feature required):

```text
ValidatedRuntimeSchema::runtime_filter_builder(
    &self, collection: &RuntimeCollectionHandle, limits: RuntimeQueryLimits,
) -> Result<RuntimeFilterBuilder, RuntimeFilterError>

RuntimeFilterBuilder::compare(
    &self, field: &RuntimeFieldHandle, op: RuntimeScalarOperator, value: RuntimeValue,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::membership(
    &self, field: &RuntimeFieldHandle, op: RuntimeListOperator, values: Vec<RuntimeValue>,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::is_null(
    &self, field: &RuntimeFieldHandle, is_null: bool,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::between(
    &self, field: &RuntimeFieldHandle, low: RuntimeValue, high: RuntimeValue,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::and(
    &self, children: Vec<RuntimeFilterNode>,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::or(
    &self, children: Vec<RuntimeFilterNode>,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::not(
    &self, child: RuntimeFilterNode,
) -> Result<RuntimeFilterNode, RuntimeFilterError>
RuntimeFilterBuilder::finish(
    self, root: RuntimeFilterNode,
) -> Result<CheckedRuntimeFilter, RuntimeFilterError>
```

The builder owns its validated schema reference, collection and limits. Nodes
have private fields and no public constructor/deserializer; only checked builder
methods create them. Each node retains the schema fingerprint and collection;
logical composition rejects foreign nodes. Leaf construction validates field
handles, filterability, kinds/operators and individual literal limits. Logical
construction and finalization count the complete original tree, depth, values
and binds before folding; reusing a node in several children counts every
occurrence. Finalization retains immutable original intent and separately
constructs the optimized structural predicate. Builder, nodes and artifact are
owned Send+Sync values, without context or external borrows.

Canonical GraphQL lowering must use these same constructors/accumulator, not a
second interpretation engine. Duplicate field/operator keys in one input object
and repeated occurrences in distinct Boolean children are different: the latter
are valid and must remain in intent.

**Approved B input boundary:** a prebuilt async-graphql Value has unique map
keys and cannot reveal duplicates erased by an earlier parser. The proposed
Value entry therefore requires an explicitly validated GraphQL coercion
prerequisite: the host/integration supplies a structural value whose original
wire duplicate checks have already succeeded. The library guarantees checked
handles/operators, bounds and preservation of all occurrences present in that
value; it cannot guarantee recovery or rejection of erased wire occurrences.
This prerequisite must be proven in the canonical GraphQL integration, not
assumed to be provided by the current parser. No such proof is claimed here.

If direct untrusted wire input is required, review a separate bounded,
duplicate-aware parser entry before converting to Value, for example:

```text
prepare_runtime_filter_json(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    input: &str,
    max_input_bytes: usize,
    limits: RuntimeGraphqlLimits,
) -> Result<CheckedRuntimeFilter, RuntimeFilterError>
```

That optional entry must reject duplicate object keys before erasure and feed
the same canonical lowering, not a second SQL evaluator. It is not included
implicitly in the Value signature. The owner selected the validated-coercion
prerequisite; the raw-input entry remains excluded and requires a separate future
contract review. Existing wire duplicate erasure remains an integration limitation.
 Existing RuntimePredicate APIs stay compatible
and cannot fabricate original intent they never retained.

`prepare_match` validates the entire artifact against an explicit semantic
profile and rejects unsupported operators/types/collations before evaluation.
`matches` revalidates fingerprint, collection, loaded dependencies, explicit NULL
and value kinds; unloaded is a structured error, never NULL. Use SQL
true/false/unknown internally; only true selects a row. NOT Unknown remains
Unknown. Preserve existing empty membership/logical/range behavior.

Initial profile should be a conservative proven SQLite BINARY/PostgreSQL C
subset. Profile selection is a trusted host assertion about actual database
configuration/storage, not inferred from a backend name. Literal string
prefix/suffix/contains require explicit empty/long operand, Unicode, wildcard and
control/NUL rules matching current rendered SQL. Exact i64, finite floats, UUID,
bytes and datetime representations need independent conformance proof; reject
unsupported combinations rather than approximate with Rust comparisons. JSON
comparisons remain unsupported. MSSQL/profile expansion requires separate proof.

Use a new safe RuntimeFilterError with invalid_input, invalid_handle,
limit_exceeded, missing_loaded_field, wrong_value_kind, unsupported_semantics
and schema_mismatch categories. Debug/Display must not include filter literals
or record values. Existing predicates, query wire forms, hashes, names and codecs
remain unchanged. The implementation uses paired ORM/macros 0.50.0.

A trusted authoritative immutable record is still only host-supplied evidence.
Neither matching nor a record codec establishes provenance, principal authority,
current policy or native RLS. In particular, offline matching cannot prove a
current SELECT policy for a deleted row. Existing default-deny deleted-preimage
policy is retained; no generic matcher overrides it.

## Dependencies, non-goals and acceptance

Dependencies are existing runtime schema/key/record/read code, the canonical
runtime GraphQL filter path and test-owned SQLite/PostgreSQL infrastructure.
No sibling changes, event/journal/replay product, transport, auth store, raw SQL
escape hatch or static API rewrite is included. No Actions dispatch, merge or
publication is authorized here; proposal commits use `[skip ci]`.

The completed pre-approval checkpoint compiled the existing public consumer controls and
confirm each expected diagnostic; review the actual nearest alternatives and
publish exact source/features/compiler/lock provenance. Documentation checks and
formatting run only in an assigned constrained lane. Then stop for owner contract
review; no new API implementation proceeds automatically.

Following separate approval, A tests cover complete single/composite nonfilterable
keys, exact projection, host denial before I/O, required restrictions, malformed
and stale/cross-schema inputs, hidden/missing equivalence, multiplicity, owned
non-superuser PostgreSQL RLS/auth isolation, cancellation and concurrent changes.
B tests compare actual selected SQL IDs and matcher results across nullable
nested logic, constant-lowered denied intent, empty lists/strings, Unicode,
pattern literals, numeric boundaries, supported types and unsupported profiles.
Unloaded/wrong-schema/kind/limits and redacted errors are explicit negatives.
Regress existing reads/mutations/static subscriptions, feature boundaries and
legacy consumers. Any implemented release follows package versions, changelog,
migration, SemVer and warnings-denied lint/docs rules; expected data migration
is none.

## Compiled current-API evidence

The external consumer lock was seeded from the reviewed root991-package lock
and reconciled offline to342 packages. The sole added identity is the local
probe package; no external source/version/checksum was introduced. Locked full
metadata passed separately for SQLite and PostgreSQL. The compiler was
Rust1.97.1, full `8bab26f4f68e0e26f0bb7960be334d5b520ea452`, target
x86_64-unknown-linux-gnu. Source and external lock hashes stayed unchanged during
each backend probe lane; the actual underlying source is the stated4059d100
baseline, not an implemented proposal.

For **each** backend, two construction controls passed and four separately
selected negative checks failed with only the intended diagnostics:

| Selection | Result | Boundary established |
| --- | --- | --- |
| `current_boundary` | 2 passed | Checked nonfilterable identity is valid, public equality rejects it; empty membership dependencies do not preserve original field intent. |
| `gap-key-read` | E0308 naming RuntimeKey | Existing predicate-based read constructor does not accept RuntimeKey in its predicate slot. |
| `gap-record-match` | E0599 naming matches_record | No such public matcher method exists on RuntimePredicate; broader source review found no alternate owned-record evaluator. |
| `gap-lowering` | E0603 private module | Canonical runtime GraphQL plan/lowering is not a public consumer entry. |
| `gap-static-record` | E0277 naming RuntimeRecord | The static subscription helper requires compiled Entity/FromSqlRow; the backend-correct owned RuntimeRecord call does not satisfy those bounds. |

The external runner rejects unrelated diagnostics, including generic-arity/type
errors in the static-helper setup. These are compiled construction/type-boundary
proofs, **not** SQL execution, native RLS, snapshot matching or proposed API
conformance. No database, external service or sibling repository was used.
One early cold compile was intentionally stopped before tests, retained as
incomplete, then resumed with an authorized independent idle-cache copy.

The [example manifest](probes/Cargo.toml.example) makes the retained Rust sources
reproducible without adding another active workspace/lock. In a scratch copy,
adjust its path dependency to the exact reviewed ORM checkout, use it as
Cargo.toml and seed/reconcile a reviewed lock. The positive command is
`cargo test --offline --locked --no-default-features --features BACKEND --test current_boundary`.
For each negative feature, run
`cargo check --offline --locked --no-default-features --features BACKEND,GAP --lib --message-format=json`
and verify the table's intended diagnostic, not merely a nonzero status. Select
only one backend and one GAP per negative check. The example manifest changes
only source layout/path from the actual external probe; the Rust files are the
same reviewed public consumer sources.

The A-only [consumer manifest](probes/Cargo.key-read.example) and
[construction/exhaustive-enum controls](probes/key_read.rs) are separate
sources for the new implementation. They use only a core ORM dependency with
one explicit backend and no `runtime-graphql` or direct GraphQL dependency. Copy
to a scratch consumer, adjust the path to the reviewed source, then reconcile a
reviewed lock in an assigned lane before running its two tests. Both controls
passed on SQLite, PostgreSQL and MSSQL without adding GraphQL dependencies;
the historical gap probes remain unchanged.

## Current checkpoint

The historical SQLite/PostgreSQL current-boundary controls and expected
diagnostics passed at 4059d1; documentation validation (197 governed files),
formatting, inventory and compiled-probe source comparisons belong to that
proposal checkpoint. They are not validation of the new implementation.

A-only source work is based on approved PR143 full head
`a52b6224750298d3b9d7ad5f03edc4f02aacec51` (production source 4059d1).
The A-only implementation reuses the key's retained schema, complete-key
validation, predicate rendering, exact projection decoder and native read/auth
path; a new bounded error wrapper leaves existing exhaustive enums unchanged.
SQLite and owned PostgreSQL key-read tests passed, including real non-superuser
RLS isolation, cancellation and subsequent safe pool reuse. Host-boundary,
current-authority, existing read/mutation/subscription, public consumer and
selected backend feature regressions passed. SQLite/PostgreSQL all-target
quality and focused supported MSSQL quality passed; MSSQL key reads reject
before pool I/O. A package build without any backend remains unsupported by the
unchanged macro guard. The explicit package-selected native API comparison
passed under a minor-release override to check additive compatibility.

ORM/macros 0.49.0, all seven existing locks and the generated inventory are
reconciled on this proposal-based stack. Before release, integrate the reviewed
0.48.x correctness fixes and validate the coherent final stack. No merge,
Actions or publication follows from these implementation gates.

B was separately approved with the host-validated coercion/duplicate
prerequisite for structural Value input; the optional raw JSON parser remains
excluded. The implementation is based on A4c0502e with paired ORM/macros 0.50.0.
Scoped SQLite and owned PostgreSQL matching tests passed at 2bcc5628, including
83 SQL cases across eight records on each backend. Its test-initializer-only
descendant 008764bd passed four selected backend quality profiles and two
explicit native minor-release API comparisons (196 checks each). External
public consumer tests passed at 008764bd: core 1, SQLite 2, PostgreSQL 2 and
MSSQL 2, without database services. The exact reviewed consumer lock was adopted
with locked metadata; the initial strict optional-edge rejection remains a
retained failed preparation, not a test pass.

The separate unpublished coherent preview 01b73b00 (paired 0.50.1, including
Q019 and the reviewed 0.48.x corrections) passed generator/policy gates and nine
finite canonical/parity phases: 54 tests, zero failed or ignored. Both SQLite
and owned PostgreSQL completed 83 SQL matching cases across eight records;
owned service cleanup was verified. Those are actual preview results, not
execution of the B-only commit or whole-workspace release acceptance.

This completes the scoped capability plan. Final release-wide integration,
feature/provider gates and publication remain under the separate release plan.
Offline matching establishes neither principal authority nor current native
RLS or deleted-record provenance. Duplicate wire keys must be checked before
Value construction; existing parser erasure is not repaired by this API.
No Actions, merge or publication follows from this plan's completion.
