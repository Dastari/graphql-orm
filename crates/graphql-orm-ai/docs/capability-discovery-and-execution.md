---
title: "Capability discovery and execution"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-25
review_by: 2027-02-01
supersedes: []
---

# Capability discovery and execution

This is the canonical contract for schema-derived AI capability discovery,
delivery, query planning, provider projection, durable execution and browser
bootstrap. It applies equally to reviewed static reads and generated GraphQL
queries. Mutation and subscription capabilities retain their separate policy
and lifecycle contracts and are never enabled by discovery.

## Canonical index

`AiCapabilityIndex::compile` runs only after the public GraphQL schema and
`GraphqlSemanticCatalog` are finished. It consumes the exact target ID and
schema fingerprint, generated query/mutation/subscription catalogues, reviewed
static descriptors and exact target-policy fingerprint. Compilation is
deterministic and all-or-nothing under independent maximum entry count,
per-entry bytes and total bytes.

Entries contain stable public IDs, kind, short name/description, logical
namespace, public entity/root, operation shape, scalar and relationship
summaries, count/sum/group-by metadata, result/risk/approval classifications
and exact source fingerprints. Entity, field, relationship, argument and root
descriptions come directly from semantic metadata; changing one changes the
semantic, capability, entry and index fingerprints.

### Entity capabilities

Index version 3 (`AI_CAPABILITY_INDEX_VERSION`) holds one entry per entity,
not one per root. Every generated query root of one entity becomes a member of
one `AiCapabilityKind::GeneratedEntity` entry (shape `Entity`), listed in its
`operations`. The entity entry carries the entity's scalar, relationship and
aggregate summaries once; its capability fingerprint covers every member
root's ID and fingerprint. A root without an entity stays a standalone
`GeneratedQuery` entry. The entry count and byte limits, per index and per
set, count discoverable entries only, so they grow with entities.

Each grouped root keeps everything it had: its own `GeneratedQuery` entry,
capability ID, capability fingerprint, entry fingerprint and plan compiler.
`entry(root_id)` resolves it, `operation_entry(entity_id, operation)` finds it
by its root field name, and `entity_entry_of(root_id)` goes back.
`entries()` lists discoverable entries only. Mutation and subscription roots
are not grouped and stay outside the broker.

The index contains no argument schema, GraphQL document or SDL, database
coordinate, resolver URL, auth mechanism, policy expression, token, delegated
authority, hidden field, secret classification or protected value. It is a
complete catalogue, not a prompt payload and not an authorization cache.

A federated host compiles one index per owning logical target, then combines
them with `AiCapabilityIndexSet::compile`. The set sorts targets
deterministically, rejects every cross-target capability-ID collision, and
fingerprints the exact target-to-index membership. It does not invent a
combined SDL, semantic catalogue, or policy fingerprint. Search ranks entries
globally, while load and execute recover the sole owning index and revalidate
that target's schema, semantic catalogue, target policy, capability, and
current resolver authority. A host-authored combined fingerprint is neither
needed nor accepted.

### Roots a catalogue leaves out

A generated capability catalogue compiles one capability per admitted root.
Two things can leave a root out while the rest of its target compiles:

- **An oversized schema.** A root whose compact plan schema is larger than the
  fixed provider-contract ceiling for one capability (1 MiB) is excluded. It
  used to fail the whole catalogue. `excluded_capabilities()` on the query,
  mutation and subscription catalogues lists each excluded root with its
  schema size and the ceiling, and `AiToolCatalog` logs one warning per
  excluded root when the catalogue is registered. An excluded root is absent
  from the catalogue fingerprint, the index and discovery. Check the list at
  startup: nothing else tells an operator that a root the schema exposes is
  unavailable to the model.
- **Strict query roots.** By default every exportable public Query root
  becomes a capability once the target policy calls `allow_queries()`.
  `AiGraphqlQueryCapabilityOptions::with_strict_query_roots` switches a query
  catalogue to opt-in: only the listed root field names are compiled, so any
  other root is never registered, indexed, discovered or executable as a
  generated capability. A listed name that is not an exportable public Query
  root fails compilation. The list selects roots only. Fields and
  relationship paths under an admitted root are still governed by the ORM's
  export and classification metadata and by resolver authorization.

```rust,no_run
# fn options() -> Result<graphql_orm_ai::AiGraphqlQueryCapabilityOptions, graphql_orm_ai::AiError> {
graphql_orm_ai::AiGraphqlQueryCapabilityOptions::default()
    .with_strict_query_roots(["Records", "RecordById"])
# }
```

### Startup cost

`cargo run --release -p graphql-orm-ai-tool-profiles --example
capability_startup_benchmark -- 250 500 1000` compiles a synthetic target at
each size and prints catalogue and index compile time, search latency and
peak resident memory, with the per-entity slope. CI runs it and publishes the
result as the `capability-startup-benchmark` artefact. Capabilities of one
catalogue share one copy of the finished schema and semantic catalogue, so
catalogue memory grows linearly with the number of roots. Catalogue compile
time is still more than linear in the number of roots; read the artefact
before sizing a very large target.

```rust,no_run
# use std::sync::Arc;
# use graphql_orm_ai::{AiCapabilityIndex, AiCapabilityIndexLimits, AiCapabilityIndexSet};
# fn build(target: graphql_orm_ai::GraphqlExecutionTargetId, schema_fingerprint: String, semantics: &graphql_orm_ai::GraphqlSemanticCatalog, queries: &graphql_orm_ai::AiGraphqlQueryCapabilityCatalog, static_tools: Vec<graphql_orm_ai::AiToolDescriptor>) -> Result<AiCapabilityIndexSet, graphql_orm_ai::AiError> {
let index = AiCapabilityIndex::compile(
    target,
    schema_fingerprint,
    semantics,
    Some(queries),
    None,
    None,
    static_tools,
    "exact-target-policy-v7",
    AiCapabilityIndexLimits::default(),
)?;
AiCapabilityIndexSet::compile([Arc::new(index)])
# }
```

### Model-context benchmark

`cargo run --release -p graphql-orm-ai --example capability_context_benchmark
-- 1 10 100` measures what the model receives as the catalogue grows. It
stamps out two copies of a generated family per target. Each family has
one wide entity with forty public scalar fields and five to-many
relationships, plus the five related entities. Each entity has five
generated query roots, including list, keyset and aggregate. That is 12
entities and 60 query roots per target. Each target gets its own query catalogue and index, and the
catalogues are registered in one `AiToolCatalog`. The example then drives
the version 2 broker on a host that caches its index set, with the
coordinator's own describe steps. For each described operation it builds and
validates the exact model definition, checks its fingerprint and renders the
signature. Describe results are merged under
`AiCapabilityDeliveryLimits::maximum_describe_bytes`, and a `describe_type`
page above that bound is refused and counted, as the coordinator refuses it.
Each point runs in its own process. Byte sizes are deterministic; times are
for one machine and one build. Approximate tokens are bytes / 4. The *eager
reference* is the size of one exact definition per root: what eager or
provider-deferred delivery would send for the same roots.

Release build, 2026-11-23 (AI 0.156.0, tool profiles 0.28.0):

| Targets | Query roots | Index entries | First-turn definitions | Eager reference | Discover result p95 | Describe result p95 | `describe_type` page p95 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 60 | 12 | 5,548 B (≈1.4k tokens) | 1.49 MB | 5,302 B | 6,037 B | 4,653 B |
| 10 | 600 | 120 | 5,548 B | 14.9 MB | 5,372 B | 6,080 B | 4,660 B |
| 100 | 6,000 | 1,200 | 5,548 B | 149 MB | 5,394 B | 6,123 B | 4,664 B |

From 1 to 100 targets, the first-turn definitions are byte-identical (the
four frozen broker definitions). Discover, describe and `describe_type`
results grow by under 2%, while the eager reference grows 100 times. A
describe of the widest entity, with all five of its operations, stays below
8 KB. At every point:

- A caller whose policy admits one target still receives a full page of that
  target's candidates for a query that matches every target.
- Changing one entity description in one target changes the index-set
  fingerprint and an eager binding, but not the fixed-broker session binding.
- A count question's top candidate offers the entity's `aggregate`
  operation.
- No `describe_type` page reaches the describe byte bound, so none is
  refused.

All points fit the default index and index-set limits.

| Targets | Discover p50 / p95 | Describe p95, first | Describe p95, repeat | `describe_type` p95 | Startup, one target | Peak memory |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.05 / 0.07 ms | 0.87 s | 15 ms (was 0.92 s) | 3.9 ms (was 244 ms) | 23 s | 174 MiB |
| 10 | 0.27 / 0.39 ms | 0.91 s | 19 ms (was 1.00 s) | 4.3 ms (was 228 ms) | 22 s | 1.3 GiB |
| 100 | 2.7 / 4.7 ms | 0.89 s | 22 ms (was 0.89 s) | 3.9 ms (was 223 ms) | 21 s | 7.8 GiB |

A *first* describe is the process's first describe of a capability: it
recovers the capability's signature type names. A *repeat* describes the
same entity again. "Was" is the same benchmark against AI tool profiles
0.27.1, which recovered the names on every call.

Discover time follows the number of matches, not the catalogue as such. The
benchmark's questions share words such as "records" with every entity, and
host policy is applied to every match before the result ceiling (see
[Discovery is not authority](#discovery-is-not-authority)). A host with an
expensive policy should override `authorize_discovery`. In those runs,
peak memory grew by about 1.3 MiB per query root with the capabilities
registered in the `AiToolCatalog`. For current startup measurements, see
[Catalogue compilation benchmark](#catalogue-compilation-benchmark).

A single-target host can supply its index through `AiCurrentCapabilityIndex`,
and the crate then forms a one-member set on every broker call. Since AI tool
profiles 0.28.0, that set reuses the index's measurements, so each call costs
what it costs a host that caches its set. Describe and `describe_type`
render a signature on every call. Since 0.28.0, a capability recovers the
type names behind its signature once, so only the first describe of a
capability pays for that. Measured on one target with 60 entities (300
roots). A first pass on the single-index host meets cold per-capability
caches. Both hosts are then measured on equally warm caches, the
single-index host first:

| | Before (tool profiles 0.27.1) | After (0.28.0) |
| --- | ---: | ---: |
| Discover p50, single-index host, warm pass | 1.56 ms | 0.153 ms |
| Discover p50, host-cached set, warm pass | 0.137 ms | 0.161 ms |
| Describe p95, cold pass (each capability's first describe) | 4.0 s | 4.6 s |
| Describe p95, warm pass, single-index host | 4.1 s | 0.025 s |
| Describe p95, warm pass, host-cached set | 4.0 s | 0.019 s |
| `describe_type` p95, warm pass, single-index host | 1.09 s | 4.4 ms |

The figures above are for broker version 2. Version 3 adds the browse
definition, so the first-turn definitions grow once, by one fixed
definition, and stay identical at every catalogue size. The benchmark now
also reports `browse` result sizes (the namespace list and each sampled
entity's capability and operation pages, each at most 32 items) and whether
a keyset operation offers a continuation, which it does from version 3 (see
[Broker version 3](#broker-version-3-browsing-and-continuation-handles)).

### Catalogue compilation benchmark

In tool profiles 0.29.0, a release profile of the 60-root target recorded
11,402 filter-role validations in 22.46 s of query catalogue compilation.
Those validations accounted for 21.65 s. At 300 roots, validation ran
57,002 times and accounted for 527.06 of 532.95 s. Input schema rendering
and admission both repeated the validation, which visits the target's
filterable fields. Tool profiles 0.30.0 validates those roles at catalogue
binding and reuses the result. The compiler also indexes the generated input
owners it has to check.

Use `capability_context_benchmark --compile-target 2`, `10`, or `20` on the
release binary for 60, 300, or 600 roots in one target. The timer includes
building the target fixture and compiling the query catalogue and index. Run
`--compile-federated 1`, `10`, or `100` for the existing 60-root targets.
The federated time is the first-target time plus the remaining targets'
compile and registration time, with up to six workers. The harness records
index-set assembly separately.

Release build on an AMD Ryzen 9 9955HX with Rust 1.97.1. Before is tool
profiles 0.29.0 at `2f2b6c9d7`. After is tool profiles 0.30.0. Each row is
one wall-clock sample from the same harness and deployment limits.

| Shape | Query roots | Before (s) | After (s) |
| --- | ---: | ---: | ---: |
| One target | 60 | 22.43 | 0.44 |
| One target | 300 | 533.31 | 2.09 |
| One target, time to rejection | 600 | 2,148.48 | 4.17 |
| Federated, 1 target | 60 | 22.63 | 0.48 |
| Federated, 10 targets | 600 | 69.56 | 1.68 |
| Federated, 100 targets | 6,000 | 384.33 | 12.01 |

The 600-root single-target baseline produced 17,060,832 bytes of execution schemas
before hitting the existing 16 MiB total schema limit. Both versions
compiled the root schemas and rejected the target at the same limit.
The row records the time to that rejection.

Run the same harness with `--contract 2` on each revision and compare stdout
with `cmp`. The snapshot covers the 60 capability fingerprints and
execution schemas, the catalogue fingerprint and serialized index, all
provider definitions, and the sampled broker describe and type results. It
uses a fixed, equal-length value for the opaque loaded reference, which
depends on the broker session.

## Discovery is not authority

The built-in search uses bounded normalized lexical terms, optional exact
namespace/kind/entity-or-operation filters, deterministic scores and stable ID
tie-breaking. Narrow, explicit list, details, search, keyset-page, or aggregate
intent ranks the matching compiler-owned operation shape before entity,
target/namespace and lexical-description relevance. Every result must retain a
positive lexical match, and relevant non-matching shapes are not discarded. It
needs no embeddings or external database.

Compiling an index builds an inverted index: each term maps to the posting
list of entries that hold it, with that entry's fixed field weights. A search
tokenises only its query and scores only the entries in its terms' posting
lists, so its cost follows the matches, not the catalogue. Scores are fixed
per-entry field weights. No inverse document frequency, BM25 or other
catalogue-wide statistic is computed, so an entry's score never depends on
how many other entries hold a term, and the rank of an entry a principal may
see carries no information about entries it may not. An entity entry is also
found by its roots' operation names and descriptions, and matches a shape
intent when any of its roots has that shape. Each root's contribution is
recorded separately, so after host policy the broker scores an entity on its
own metadata and its admitted roots only (`rank_admitted`).
`AiCapabilityIndexSet::ranked_operation_matches` ranks each executable root
on its own metadata, for delivery that installs one definition per root. A host may later
implement the closed current-index-set and authority traits with a reviewed
search service, but returned IDs and fingerprints must still bind to the
canonical set and the candidate's exact owning index.

`AiCapabilityDiscoveryBroker` rehydrates the current principal and reapplies
host scope, target, kind, classification, provider and session policy while
filtering search results, again while loading, and again immediately before
execution. Search applies policy to every ranked match through
`AiCapabilityAuthorityPolicy::authorize_discovery` and only then applies the
result ceiling, so a restricted principal receives the highest-ranked
candidates it may use. The policy work depends on the query and catalogue, not
on the decisions, and the response carries no score, count or continuation.
Each batch decision names the capability it decides, and the broker refuses
the batch unless decision `i` names ranked match `i`. Matches are decided in
rank order, in batches of at most `AI_CAPABILITY_BROWSE_POLICY_BATCH` (256)
decisions, or the host's `with_maximum_discovery_authorizations` when it is
lower; an entity with more grouped roots is decided alone. The default
`authorize_discovery` calls `authorize` per match with up to 16 calls in
flight; hosts with an expensive check override it to decide each batch from
one current lookup. A broad query can send up to the index set's
`maximum_entries` matches to policy.

`with_maximum_discovery_authorizations` also sets a ceiling on the
**authorized view**: the admitted matches with their admitted grouped roots
(and, per root, each admitted entity). A query whose authorized view exceeds
it is refused with the stable `AI_CAPABILITY_DISCOVERY_TOO_BROAD` input
error. Denied matches never count towards the ceiling, so whether a query is
refused depends only on what the principal may see. As for browse, the
guarantee covers response content, not timing: policy work grows with the
hidden matches decided before the view is complete, so discover does not take
constant time against an observer who can time it in process. A host with an
expensive policy should override `authorize_discovery`. Report a denial as
`allowed: false`: a policy `Err` fails the whole search and would reveal that a
hidden entry matched. `AiCapabilityAuthorityPolicy` receives the exact owning index on
each check; hosts apply target policy from that proof rather than parsing an
ID. Its `AiLoadedCapabilityBinding` is crate-created, private-field,
short-lived and fenced to owner reference, session, run, attempt, lease,
provider session, aggregate index set, owning target policy, schema, semantic
catalogue, index, entry, kind and capability. Revocation or drift between any
two stages fails closed.
The final call must still pass through the ordinary durable application-tool
broker and authenticated GraphQL resolver.

## Delivery modes

The coordinator calls `select_capability_delivery_mode`; prompts do not select
delivery. `ProviderCapabilities::capability_delivery_modes` is reviewed
negotiation metadata, never authority.

| Mode | Initial surface | Use |
| --- | --- | --- |
| `EagerExact` | All already-filtered exact definitions | Small sets within exact count and byte limits |
| `ClientDeferred` | Exact static bootstrap plus discovery; freshly loaded generated-query definitions on the next continuation | Stateless/local or client-executed search |
| `ProviderDeferred` | Already-filtered definitions marked for reviewed native deferred loading | Native provider tool search, including large non-frozen surfaces unless the broker preference is enabled |
| `FixedBroker` | Exact static bootstrap plus frozen discover/browse/describe/describe_type/execute definitions | Retained sessions, and large surfaces by default |

Above the eager count or byte limit, a provider that supports both
`ProviderDeferred` and `FixedBroker` gets `FixedBroker` by default
(`AiCapabilityDeliveryLimits::prefer_fixed_broker_for_large_surfaces` is
`true`), so no selected mode's request size grows with the catalogue. To keep
native deferred loading for large surfaces, set the field to `false` or call
`with_provider_deferred_for_large_surfaces()`. The mode is part of the
retained binding, so changing this setting rebinds retained sessions. A
provider that supports only `ProviderDeferred` keeps it.

Provider-session bindings admit at most 128 exact static bootstrap fingerprints.
Grok SDK and Codex dynamic tools independently enforce 128-definition ceilings
on the **complete** surface, including fixed broker definitions. Fingerprint
admission does not relax provider byte/frame limits or application authority.

`prepare_client_deferred_continuation` accepts only definitions matching the
crate-owned loaded bindings and the configured selection count. Fixed broker
arguments use a closed scalar argument list and public selection paths; the
selected capability's authoritative compiler validates every name and type.
No broker accepts arbitrary GraphQL, target, alias, fragment, introspection,
SQL, URL or callback.

### Reviewed static tools behind the fixed broker

A reviewed static read tool can be reached through the fixed broker instead
of being offered as a bootstrap definition. Such a tool is not a provider
definition: it does not count against the 128 static bootstrap fingerprints
or a provider's definition ceiling, and adding or removing one never changes
the session binding or the five broker definitions.

```rust,ignore
let session = AiCapabilityBrokerSession::new(delivery_limits)?
    .with_brokered_static_tools(brokered_descriptor_fingerprints)?;
```

List each tool either in the session binding's bootstrap fingerprints or
here, never both; `AiCapabilityDeliveryTurn::select` refuses an overlap and
refuses the list in any mode other than `FixedBroker`. The static descriptors
must be in the compiled index, as before.

The model finds a brokered tool with discover (an unfiltered search returns
generated queries and brokered static tools together), loads it with
describe and runs it with execute:

- describe returns the tool's risk, approval and classification metadata and
  one operation whose signature renders the tool's reviewed argument schema
  and ends in `-> fixed`;
- execute takes the arguments as the same flattened name and value entries a
  generated query uses, and an empty `selections` list. The tool's document
  and result are fixed, so a selection, relationship argument or bound is
  refused as invalid input.

The exact-definition review contract is unchanged. The fingerprint in the
list must equal the indexed entry's and the registered descriptor's
fingerprint at discover, describe and execute, or the call fails closed. Each
call rehydrates the principal and applies host capability policy to the exact
entry. Execute then validates the arguments against the registered schema,
applies current tool authorization, runs the ordinary resolver path and
applies the tool's own static disclosure schema. The coordinator refuses a
turn whose hierarchical rules do not admit every listed fingerprint as an
approval-free read tool, so a rule that lists allowed tool fingerprints
applies to a brokered tool exactly as to an offered one.

Only an approval-free, read-only, idempotent application query whose maximum
classification is below `Restricted` and which declares no result media can
be brokered. Anything else in the list is never discoverable and never
executable through the broker; keep it a bootstrap definition. A brokered
call carries no `AiToolExecutionProvenance`, so a host executor that requires
provenance refuses it. Reviewed static mutations stay bootstrap definitions.
Generated mutations can enter the broker under a classified coordinator (see
[Generated mutations through the fixed broker](#generated-mutations-through-the-fixed-broker));
subscriptions keep their wait contract and cannot.

### Coordinator construction and dispatch

One `AiCapabilityDeliveryTurn` is the run-owned bridge between mode selection,
the exact provider surface and durable broker execution:

```rust,ignore
let delivery = AiCapabilityDeliveryTurn::select(
    provider_capabilities,
    index_set.fingerprint(),
    currently_eligible_read_definitions,
    retained_definitions_frozen,
    provider_capability_session_binding,
    capability_broker,
    AiCapabilityBrokerSession::new(delivery_limits)?,
)?;

let surface = delivery.current_surface();
request.tools = surface.tools().to_vec();
let provider_plan = AiProviderCallPlan::new_with_capability_surface(
    provider_kind,
    request,
    budget,
    transfers,
    correlation_id,
    &surface,
    runtime.tool_catalog(),
    static_policy,
    generated_target_policy,
)?;
let turn = AiReadOnlyAgentTurnPlan::new(
    provider_plan,
    result_egress_route,
    rules,
    uses_byok,
)?
.with_capability_delivery(delivery.clone())?;
```

The host composes `OrmAiApplicationToolCallService` as the coordinator's
ordinary tool executor. The coordinator recognizes only the five frozen IDs
from the exact offered surface and dispatches them through that service; a
host executor retains a fail-closed default and does not gain broker execution
implicitly.

For a continuation, override
`AiReadOnlyAgentTurnPlanner::continuation_plan_with_capability_delivery`. Use
the supplied delivery turn's current surface with
`AiProviderCallPlan::new_continuation_with_capability_surface`, and attach a
clone of that same turn. Client-deferred discovery installs freshly loaded
generated-query definitions only after its durable tool result and checkpoint
commit. An empty current-authority discovery clears earlier generated
definitions rather than retaining a stale provider surface. Recreating blank
broker state under an equal public fingerprint, retaining a pre-discovery
surface, or substituting definitions fails closed.

Hierarchical rule policy must admit the exact broker fingerprints present in
the crate-owned surface as approval-free read tools. That is a loop constraint,
not application authority: the execute operation still rehydrates the current
principal and passes the selected generated query through the ordinary policy,
delegation, resolver and disclosure boundary.

### Broker version 2: signatures, `describe_type` and entity operations

Version 2 introduced discover, describe, `graphql.capabilities.describe_type`
and execute as described here. Version 3 keeps them and adds browsing and
continuation handles (next section).

- **Discover** returns, for an entity, one `generated_entity` candidate with
  its `operations` (each root's name and shape). A root without an entity and
  a brokered static tool are candidates of their own. `kind` filters:
  `generated_query` (every generated read), `generated_entity` or
  `reviewed_static`. Each candidate carries an opaque `candidateReference`
  (see [Candidate references](#candidate-references)).
- **Describe** takes the candidate's `capabilityId` and `candidateReference`,
  loads the candidate and returns one entry per operation under
  `operations`: the root's shape, description, result-record cost and a
  compact typed signature, for example
  `Records(where?: RecordWhereInput|null, page?: PageInput) -> select`, with
  `select` (the top level of the selection tree, relationships as
  `name{…}`), `maximumItems` and `inputTypes`. Every GraphQL input type is
  named, never inlined. There is no `planSchema` unless the run sets
  `AiCapabilityBrokerSession::with_inline_describe_schema`, which adds each
  operation's inline schema when it fits the bound.
- **`describe_type`** takes the `loadedReference`, the `operation` and a type
  name: one from `inputTypes`, `select`, or `select.<path>` for a
  relationship path. It returns the type's fields (name, type, whether
  required, description), an enum's values, or a selection level (scalars,
  and relationships with their arguments and `maximumItems`; the root level
  also carries each field's index description), at most 128 per page with
  `offset` and `nextOffset`.
- **Execute** takes `operation` and the same flattened arguments and
  selection paths as before. `operation` may be omitted when the description
  admitted exactly one operation.

Host policy decides the entity and each root. Discover sends each matching
entity entry followed by its grouped roots in one `authorize_discovery`
batch. An entity is returned only with the roots admitted, its `operations`
list only those, and it is ranked again on its own metadata and the admitted
roots alone (`AiCapabilityIndexSet::rank_admitted`), so a denied root's name,
description or shape decides neither whether nor where the entity appears.
An entity with no admitted root is not listed. Describe asks about the entity
and then each root entry, and lists only the roots admitted (none admitted
refuses the load); its entity-level `resultDescription` and
`resultRecordCost` count only those roots. `describe_type` and execute
re-check the entity and the chosen root, including their fingerprints and the
policy fingerprints recorded at describe, before anything runs (see
[Stale selections](#stale-selections) for how a change the principal cannot
see is handled).
Client-deferred discovery ranks each root on its own metadata
(`ranked_operation_matches`) and sends a grouped root's entity before the
root; a root is returned, loaded and executed only while both are admitted. The
chosen root's own capability, fingerprint and plan compiler execute the plan,
then the ordinary resolver path and disclosure schema apply. An `operation`
the description did not return is `invalid_arguments` with a fixed
correction (`AI_CAPABILITY_OPERATION_INVALID`), as is an undefined type name
(`AI_CAPABILITY_TYPE_UNKNOWN`).

The signature is model-facing only. `AiCapabilitySignature` renders it from
`AiSharedTypePlanSchema::compile_named`, which expands back to the compact
schema exactly. Plan validation, the capability fingerprint and the eager and
client-deferred definitions are unchanged. Hosts can render the same
signature with `AiGraphqlQueryCapability::capability_signature`.

Describe also reports conservative compiler-owned maximum root and total
result-record costs plus whether the plan must choose a positive root bound;
these values are planning metadata, not execution authority or observed row
counts. The complete description, including its signatures, is bounded by
`AiCapabilityDeliveryLimits::maximum_describe_bytes` (512 KiB by default and
4 MiB at the compiled ceiling), and so is each `describe_type` page. Nothing
is truncated or partially exposed: an inline `planSchema` that does not fit is
dropped first, then the signatures, and the response sets
`signaturesAvailable` to `false`; an oversized type page is refused so the
model asks for a smaller `limit`.

### Broker version 3: browsing and continuation handles

`AI_CAPABILITY_BROKER_VERSION` is 3. Its five definitions are discover,
`graphql.capabilities.browse`, describe, `describe_type` and execute.

**Browse** lists the capabilities discover covers (generated reads and the
static tools this run brokers) by hierarchy, for a model whose search is
ambiguous:

| `level` | Needs | Returns |
| --- | --- | --- |
| `namespaces` | nothing | each logical namespace with at least one admitted capability |
| `capabilities` | `namespace`, optional `entity` | discover-shaped candidates with a `candidateReference` |
| `operations` | `namespace` and `entity` | the entity's candidate header (no operation list) and one page of its operations: name, shape, description and result description |

Pages hold at most `AI_CAPABILITY_BROWSE_PAGE_MAXIMUM` (32) items, with
`offset` and `nextOffset`; `offset` goes up to
`AI_CAPABILITY_BROWSE_MAXIMUM_OFFSET` (1,048,576, the most entries an index
set can hold), so every returned `nextOffset` is accepted. Browsing applies
host policy exactly as discover does, before anything is counted or paged:
every listed entry and each entity's grouped roots go to
`authorize_discovery` in one batch, an entity is kept only with its admitted
roots, and a namespace appears only with an admitted capability.

Nothing in a browse result depends on what the principal may not see:

- Policy decides the listed entries in order, in batches of at most
  `AI_CAPABILITY_BROWSE_POLICY_BATCH` (256) decisions, or the host's
  `with_maximum_discovery_authorizations` when it is lower (an entity with
  more grouped roots is decided alone). Browse stops as soon as the
  requested page is decided: the first `offset + limit + 1` admitted
  results, or at `namespaces` the first namespaces with an admitted
  capability (so namespaces carry no count). The response is exactly that
  admitted prefix. Hidden entries change how much policy work is done,
  never what is returned, so browse never reports
  `AI_CAPABILITY_DISCOVERY_TOO_BROAD`. An unknown and a denied filter
  return the same empty page.
- The guarantee is on response content, not timing: browse does not take
  constant time against an observer who can time it in process, since
  policy work grows with the entries before the page. A host with an
  expensive policy should override `authorize_discovery`.
- Results carry no index or entry fingerprint. Each listed capability has an
  opaque `candidateReference`, fresh for every browse, that describe takes,
  exactly like a discover candidate's (see
  [Candidate references](#candidate-references)). Browse results are not
  shared through the discovery cache.
- The `operations` header has no operation list, so `limit` bounds every
  list the page returns.

Browsing grants no load or execution authority.

#### Candidate references

Discover and browse return no index-set, index or entry fingerprint, and
describe echoes none. Those fingerprints hash the whole index set or entity
entry, including operations host policy denies the principal, so returning
them would let the model, or anyone who can read the transcript, see that
hidden operations exist and detect when they change. The broker keeps them
server-side and checks them at describe, `describe_type` and execute as
before, but reports a change as `capability_stale` only when the principal can
see it (see [Stale selections](#stale-selections)).

Every discover or browse candidate instead carries an opaque
`candidateReference`: 64 hex characters of a random nonce and the capability
ID, fresh for every call, hashing no fingerprint or policy decision.
Describe takes `capabilityId` and `candidateReference`, both required. A
reference resolves only in the broker session that issued it, or the
discovery cache, and only for the same principal reference, AI session, run,
attempt and provider session, so a reused or cloned session cannot resolve it
under another run. Each reference is stored with the exact search snapshot
it was issued with, and describe resolves it against that snapshot only, never
against a later search that returned the same capability. A reference lives
as long as its issuing search is retained: when the session evicts that
search (`maximum_retained_searches`) and no cache holds it, the reference is
`selection_stale`.
Apart from their fresh references, two principals with different policy
therefore receive identical discover and describe results for the operations
they share, and no result changes when only a hidden operation changes. A
describe call that sends an earlier contract's `candidateFingerprint` is
`selection_stale`, whatever its value, `null` included, and whether or not it
also sends a `candidateReference`.

`AiCapabilityDiscoveryBroker::with_discovery_cache` retains discovery results
beyond the session's own `maximum_retained_searches`, together with the
references they issued and the run they were issued to. A cached entry
answers only that run, so a later run, or another attempt of the same run,
discovers again before it describes.

**Continuation handles.** A generated keyset root
(`AiGraphqlQueryCapability::supports_continuation`) returns its rows a page
at a time. Describe marks such an operation `"continuation": true`. Its
broker result is the usual `data` and `errorCodes` plus `continuation`:
`{"handle", "expiresInSeconds"}` while the resolver reports another page,
`null` otherwise. To read the next page the model calls execute with the
`loadedReference`, optionally the `operation`, and `continuation` only. Any
other plan field with a handle is invalid input.

The handle is an opaque random reference to state the run's
`AiCapabilityBrokerSession` keeps server-side: the exact compact plan and root
of the first page and the resolver's end cursor. The cursor never reaches the
model. The compiler selects the connection's `pageInfo { hasNextPage
endCursor }` beside its edges and `AiCompiledGraphqlQuery::take_page_info`
removes it from the resolver response before the disclosure schema is
applied; for a later page the compiler supplies the cursor as the
server-owned keyset `after`, and the model still cannot address the page
argument. A continued page is not authorized by its handle:

- it must be presented with the `loadedReference` that produced it, in the
  same broker session, so it carries that binding's principal, session, run,
  attempt, provider session and fingerprints;
- execute repeats the full loaded-binding check, the current index and
  schema fingerprints and current host policy for the entity and the root,
  exactly as for a first page, and refuses a change the principal can see as
  `capability_stale`; the root's model-facing contract fingerprint must still
  be the first page's, and the page runs against the exact current
  capability;
- the page then runs through ordinary resolver authorization, row policy and
  the disclosure schema, so a cursor only ever moves the position.

An unknown, altered, evicted or expired handle, or one presented with another
`loadedReference` or session, is `selection_stale`. Handles expire with
their loaded binding (the broker's loaded TTL, at most five minutes) and are
expired from the expiry instant on. A redemption first reserves one execution
of its chain's budget, then rechecks retention and expiry after the policy
checks and once more at executor admission
(`AiCapabilityDiscoveryBroker::continuation_admission`, called after host
tool policy and the request context are built, immediately before the
resolver); a refusal at either point returns the reservation and answers
`selection_stale`. A session retains at most
`AI_CAPABILITY_MAXIMUM_CONTINUATIONS` (32) handles, dropping the oldest. The
chain's budget is created with the first page's execution and shared by
every handle issued from it, however often that page is recorded: at most
`AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES` (256) executions, the first page
included, so reuse and concurrent redemption spend the same budget and 256
bounds the chain's total work, not only its depth. A spent
chain issues no new handle and its handles are `selection_stale`. An end
cursor longer than `AI_GRAPHQL_QUERY_MAXIMUM_CURSOR_BYTES` (1,024) ends the
chain.

The cursor is also kept out of diagnostics: the `Debug` output of
`AiCapabilityBrokerSession`, `AiCapabilityExecution`,
`AiGraphqlQueryPageInfo`, a compiled page (`AiCompiledGraphqlQuery`),
`ToolGraphqlRequest` (variable names only) and `ToolGraphqlResponse` (whose
`data` still holds `pageInfo` until the runtime removes it) redacts it. The host's
`GraphqlToolExecutor` necessarily receives it as the `after` variable of the
continued request; a host transport must not log or forward request
variables.
Every page is bounded by `maximumItems` and the result limits as before, so
results stay O(page). Offset list roots take the model's own `page.offset`
and are not continuable.

A host driving the broker itself uses
`AiCapabilityDiscoveryBroker::dispatch_browse`,
`AiRuntime::execute_query_capability_page` with
`AiCapabilityDiscoveryBroker::continuation_admission` as its final admission (or
`AiGraphqlQueryCapability::compile_compact_page` and `take_page_info`) and
`AiCapabilityDiscoveryBroker::record_continuation`; the coordinator does this
for broker calls.

**Server-side aggregates** are the entity's generated aggregate and count
operations: a count question's top candidate offers the `aggregate`
operation, and its result is the aggregate, not the rows.

### Generated mutations through the fixed broker

A session can also let its fixed broker reach generated mutations:

```rust,ignore
let session = AiCapabilityBrokerSession::new(delivery_limits)?
    .with_brokered_mutations();
```

Opting in grants nothing by itself. Only the classified native supervised
coordinator routes consequential calls, and only it binds its turn's
hierarchical rules when the delivery is attached
(`AiSupervisedAgentTurnPlan::with_capability_delivery`). The binding is part
of that turn's own delivery value: the delivery the host passed in, and any
clone of it (for example one attached to a read-only plan), stays closed to
mutations. A generated mutation is then discoverable, browsable, describable
and executable only while those rules admit its exact capability fingerprint
at its reviewed classification:

| Reviewed classification | Admitted as | Describe `effect` |
| --- | --- | --- |
| `ApprovalRequired` | one-shot supervised write | `approval_required` |
| `Automatic` | approval-free autonomous write | `automatic` |
| `Prohibited` | never offered | — |

Rules that admit only the five broker fingerprints admit no mutation. The
read-only coordinator binds no rules, so through it no mutation is ever
discovered, described or executed; an execute that resolves to one anyway is
refused with the fixed correction `AI_CAPABILITY_CONSEQUENTIAL_UNROUTED`.
Discover takes `kind: "generated_mutation"`. Describe returns the mutation's
risk, approval and `effect` and one compact signature, built from the
mutation's compact plan schema exactly as for a query. Describe decides
admission from the held candidate (its kind, fingerprint and reviewed
approval) before it looks the capability up in the current index, so a
mutation the turn's rules refuse is the same `selection_stale` whether or not
the catalogue has since changed.

**Known limitation: rule admission rotates with the catalogue.** Rules admit
a brokered mutation by its exact capability fingerprint, and a generated
capability fingerprint covers the finished-schema and semantic-catalogue
fingerprints. Any catalogue edit, even to an unrelated capability or to one
the principal is denied, therefore changes every generated mutation's
fingerprint, and rules pinned to the earlier fingerprint stop admitting it:
discover no longer lists it and a held candidate or loaded binding is
`selection_stale` or refused, until the rules are refreshed with the new
fingerprint. A stable mutation admission identity is tracked as a follow-up.

**Describe does not report operation assurance.** The capability index
carries no assurance metadata, so a described mutation does not say whether
it requires recent authentication or another assurance level. The owning
service enforces assurance only at execution, exactly as for the direct
definition: a mutation that needs fresh assurance is refused there, after
approval for an approval-required one.

Execute only **resolves** a mutation. The broker proves the loaded binding,
the current index, host capability policy and the bound rules, and returns
the exact capability and closed plan; it never reaches a resolver. The
coordinator then classifies and rule-checks the **resolved** mutation, not
the broker definition, from the registered catalogue. Because resolution
awaited, it resolves current rules again: they must still be the turn's
rules (same fingerprint) and admit the mutation's own fingerprint at its
reviewed classification. From there it is the direct definition's path:

- an automatic mutation runs once, with the same pre-effect fence and
  `RecoveryRequired` convergence. Its rule admission is re-proved once more
  inside the execution bridge, as the last step before the executor: after
  the pre-effect checkpoint, principal rehydration, host tool policy and
  request-context construction. A rule change or revocation committed
  before that check refuses the mutation as a durable
  `not_started_authorization_denied` result, and the executor never runs;
- an approval-required one is prepared and parked exactly as a direct one,
  with the same server-built preview, one-shot binding of the resolved
  fingerprint and plan hash, forced step-up for `HighImpact` and approval
  lifetime cap; nothing runs until the approval is consumed;
- while an approval is pending, a later consequential call in the same
  native turn receives the closed paused reply.

**Approved execution re-proves rules.** Construct the consequential service
with `OrmAiConsequentialToolCallService::with_rule_resolver`, using the
coordinator's resolver. For a broker-routed call, native finalization and
`execute_approved` resolve current rules for the exact claimed lease and
scope. They require the rules that admitted the mutation (their fingerprint
is bound in the call's protected evidence) and a one-shot admission of the
mutation's own fingerprint: before the approval is consumed, where a
refusal leaves it unconsumed, and again inside the execution bridge as the
last step before the executor (after consumption, principal rehydration,
bound host tool policy and request-context construction), where a refusal
is a durable `not_started_authorization_denied` result. Without a rule
resolver, every broker-routed approval is refused. A worker that claims an
approval and calls `execute_approved` directly is held to the same proof.

The approval binding itself records the broker origin
(`AiApprovalBinding::broker_origin`: the execute tool ID, the exact offered
fingerprint, the provider argument hash and the admitting rule
fingerprint). Execution recomputes the binding from the durable call, so an
approval granted for a broker-routed call matches only while the call still
records that origin and those rules. A direct binding omits the field and
hashes as before.

A rule recheck cannot be atomic with the resolver's own work, which runs in
the owning service. What is guaranteed is ordering: a revocation committed
before the final check, the last step before the executor, always wins.

Every generated authorization check (auth mode, scopes, operation assurance,
entity, row and field policies, RLS) runs in the resolver as for the direct
tool.

**Two identities.** The durable tool-call row keeps both. `tool_id` and
`tool_fingerprint` are the resolved mutation, and the new nullable
`provider_tool_id` and `provider_tool_fingerprint` (AI schema module 0.86.0)
are the broker execute definition the provider called. A direct row leaves
both new columns null. Preparation, approval binding, the protected
arguments (the resolved closed plan), rules and execution use the resolved
identity. Every tool result, native control reply, approved-outcome
continuation and checkpoint comparison with the provider's own call uses the
provider-facing one, so the approved result answers the provider's
`graphql.capabilities.execute` call ID with the broker tool ID. Approved
execution and native finalization re-resolve a broker-routed row from its
stored resolved identity and arguments only; the broker's process-local
loaded binding is never needed again.

**Protected evidence.** A broker-routed row's protected arguments are an
envelope (`graphql-orm-ai/brokered-call/v1`). It records the broker origin
(the execute tool ID and the exact offered definition fingerprint), the hash
of the provider's original broker arguments, the fingerprint of the rules
that admitted the mutation, and the resolved plan. The row's `argument_hash`
stays the hash of that plan. Checkpoint validation compares the evidence
with the provider's own requested call (its fingerprint and argument hash),
and finalization does the same against the settled provider turn.

Only the clear dual-identity columns claim broker origin; the envelope is
parsed only for a row that has them. A direct row's protected arguments are
taken verbatim, so direct arguments that happen to contain the envelope's
`format` value stay ordinary arguments. The envelope and the columns must
agree:

- columns added to a direct row are refused, because its arguments are not
  an envelope;
- a broker-routed row whose columns are cleared no longer hashes to its own
  `argument_hash`, and its approval binding still records the broker
  origin, so it is refused rather than run as a direct call;
- a provider fingerprint other than the current fixed-broker execute
  definition is refused.

A direct row, including every row from before schema module 0.86.0, keeps
its own arguments and both columns null.

**Database writers.** With the built-in database-managed content
protection, protected values are stored as readable JSON: an actor who can
write the AI tables can rewrite a row's columns, its envelope and its
argument hash together. The approval binding defeats the downgrade above for
approvals, but database-managed protection does not defend the durable
state against database writers in general. A host that needs that must use
an encrypting or authenticating `AiContentProtector`, which can bind each
protected value to the entity, row and field it is given.

Brokered mutations add no provider definition, so first-turn definitions stay
the five fixed broker definitions at every catalogue size.

### Turn amplification

Delivery work consumes normal bounded-loop capacity:

| Path | Extra broker calls before/including application execution |
| --- | ---: |
| Eager exact | 0 |
| Provider deferred | Provider-native discovery only; exact application call still counts normally |
| Client deferred, newly selected capability | 1 discover, then the exact application call |
| Fixed broker, novel capability | discover (or browse) + describe + execute = 3, plus one `describe_type` per expanded type and one browse per extra hierarchy level |
| Fixed broker, still-loaded capability | execute = 1 |
| Fixed broker, next page of a continuable result | execute = 1 |

Every completed broker result is ordinary provider input, so stateless loops
also consume the corresponding continuation turns. Size provider-turn,
tool-call, duration, budget and hierarchical-rule ceilings for this bounded
amplification. The library never raises a host ceiling automatically.

`AiProviderCapabilitySessionBinding` fingerprints delivery mode, static
bootstrap definitions, projection algorithm, model, reasoning effort and the
underlying registration identity. Pass it to
`AiProviderSessionDescriptor::new_with_capability_binding`; the existing
durable registration fence then makes any change cleanup-and-rebind only.

What else the binding covers depends on the mode:

| Mode | Also bound | A schema or catalogue change |
| --- | --- | --- |
| `EagerExact`, `ProviderDeferred`, `ClientDeferred` | the complete canonical index set | rebinds the retained session |
| `FixedBroker` | `AI_CAPABILITY_BROKER_VERSION` (3) | keeps the retained session |

Broker version 3 also versions the fixed-broker result contracts it
changes: discover results carry disclosure contract
`capability-broker-discover/v4` and describe results
`capability-broker-describe/v4` (version 4 of each contract dropped the
model-facing index and candidate fingerprints); browse is
`capability-broker-browse/v1`,
and a continuable execute result has a contract derived from its query's
disclosure schema. Client-deferred discovery is unchanged: it still returns
`version: 2` under the version 2 contract
(`AI_CAPABILITY_CLIENT_DEFERRED_DISCOVERY_VERSION`).

Bumping the broker version changes the fixed broker definitions and every
fixed-broker binding, so a session bound under an earlier version matches no
current binding and rebinds once; the Grok registration check also refuses a
tool set that carries earlier definitions. The five fixed broker definitions are
the same for every catalogue, so a
retained fixed-broker session, and the chat it carries, survives a schema
change in any target. The index is checked where it matters instead: every
describe and execute compares the current index-set, index, schema, semantic
catalogue, entry, capability, target-policy and host-policy fingerprints with
the ones the selection was made under, as
[Stale selections](#stale-selections) describes.

### Stale selections

Both outcomes are retryable, execute nothing, and carry a fixed `correction`
(failure envelope version 2) telling the model what to do next.

| Model-facing code | When | Model recovery |
| --- | --- | --- |
| `capability_stale` | The run holds the candidate or loaded capability, and what this principal is shown of it has changed since (at describe, `describe_type` or execute). | Discover again, describe the new candidate, use the new signatures and `loadedReference`. |
| `selection_stale` | The run does not hold the selection: discovery or browse never returned that candidate reference to it (including a reference issued to another run or attempt, and an earlier contract's `candidateFingerprint`), the `loadedReference` is unknown or expired, or the continuation handle is unknown, expired or belongs to another `loadedReference`. | Discover (or describe) again. |

When any exact server-side fingerprint has moved, the broker loads the
capability again from the current index under current policy and compares
its **visible fingerprint**: the capability's model-facing metadata and
contract fingerprint and, for an entity, exactly the roots this principal is
admitted to, each with its own metadata and contract fingerprint. A contract
fingerprint (`AiGraphqlQueryCapability::contract_fingerprint`) covers the
root's identity, target, semantic operation, plan schemas and limits, and
leaves out the catalogue-wide schema and semantic fingerprints. When the
visible fingerprint is unchanged, the change lies only in state the
principal cannot see (a root it is denied, another capability, another
target, the target policy or a host-policy fingerprint that admits the same
view): describe loads from the current index, and `describe_type`, execute
and continued pages replace the session's binding with the refreshed one,
under the same `loadedReference` and expiry, and proceed against the exact
current capability. Execution always binds the exact current capability,
plan compiler and disclosure schema. Otherwise the call is
`capability_stale`.

`capability_stale` is reported only for a candidate the current principal was
already shown, or a capability it already loaded, and only for a change to
what it was shown, so it reveals nothing about capabilities or operations the
principal cannot see. `selection_stale` says nothing about
whether the named capability exists. A current policy denial with unchanged
fingerprints is `authorization_denied`, never a stale code.

Native OpenAI deferred loading is disabled by default. Review exact compatible
models and opt them in with
`OpenAiProviderConfig::with_native_tool_search_models`. The adapter emits
`defer_loading:true` only for coordinator-marked definitions and adds the
native `tool_search` control; it never sends a definition absent from the
host-filtered surface. Provider call IDs and the ordinary canonical tool
binding remain unchanged.

## Compact schema-derived plans

Generated provider definitions advertise query-plan wire version 3. The model
selects explicit scalar paths and bounded relationship paths inside one exact
capability:

```json
{
  "arguments": {"id": "job-123"},
  "selections": [
    "id",
    "status",
    "labour.id",
    "stock.quantity",
    "comments.text"
  ],
  "relationshipArguments": {
    "labour": {"page": {"limit": 25}},
    "stock": {"page": {"limit": 25}},
    "comments": {"page": {"limit": 10}}
  },
  "relationshipMaximumItems": {
    "labour": 25,
    "stock": 25,
    "comments": 10
  }
}
```

The schema is a finite list of public paths, not a recursively expanded
boolean object. Root and relationship arguments retain their generated types;
ordering, paging and list ceilings remain schema-derived. The compiler expands
the compact form internally, rejects unknown/hidden/secret/cross-target/stale
paths, enforces relationship depth and cardinality, computes the complete
sibling/nested result-record budget, enforces result bytes, and produces the
exact server-authored GraphQL document and disclosure projection. There is no
select-all behavior.

`compile_compact_correctable` preserves the compiler as final authority while
returning only `invalid_arguments`, `selection_too_large`,
`relationship_depth_exceeded`, `result_budget_exceeded` or `capability_stale`
with reviewed limits. A schema-valid cross-field aggregate overflow is
therefore a correctable model outcome, not execution uncertainty.

## Strict provider projection

Canonical argument schemas remain provider-neutral. For native OpenAI strict
functions, `OpenAiStrictToolProjection` recursively sets
`additionalProperties:false`, requires every property and represents optional
values through nullable fields or an explicit present/value envelope when
canonical explicit null must remain distinguishable from omission. The
adapter validates projected arguments, applies the inverse mapping, validates
the unchanged canonical schema and binds the algorithm, schema and inverse in
the projection fingerprint. Unsupported, ambiguous or lossy schema features
fail before dispatch; a definition is never marked strict on the wire without
a valid strict projection.

## Final request sizing and dispatch

Construct the complete `AiProviderCallPlan` only after adding the crate-owned
continuation and exact loaded definitions. The plan replaces the inference
manifest's estimate with the final provider-specific conservative byte/token
requirement. `egress_requirement()` exposes only counts, classifications and
stable source references; it contains no continuation, prompt or result. The
host authorizes that exact manifest and the executor passes the corresponding
proof and budget reservation in `ProviderRequestContext`.

The typed provider boundary reports `RejectedBeforeDispatch`, `Dispatched` or
`FailedAfterPossibleDispatch`; accepted/streaming/completed states are then
derived from validated events. Local schema, policy, manifest, budget,
registration, projection, credential and adapter-preparation failures release
unused reservation capacity. Once bytes may have crossed the boundary, a
failure is uncertain. Provider acceptance and usage reconciliation retain the
existing idempotent run/attempt/lease fences.

## One durable application-tool outcome

All ordinary, dynamic, stateless, retained, native-function and fixed-broker
read calls use `OrmAiApplicationToolCallService`. Its order is:

```text
validate -> rehydrate -> current policy -> fenced row -> exact capability
-> resolver authorization -> disclosure/protection -> durable outcome
-> one model result -> continuation checkpoint
```

The canonical `AiDurableApplicationToolOutcome` is either protected successful
model input plus egress metadata, or a persisted bounded failure code and
retryability. Invalid arguments, plan/depth/result limits, revocation,
disclosure-permitted not-found, temporary unavailability and resolver-safe
validation do not make a read run recovery-required. A crash after the row is
finished reopens the same protected result and does not execute the resolver a
second time. Recovery remains mandatory for uncertain consequential effects,
irreconcilable persistence/fencing and ambiguous retained-provider execution.
Raw resolver/provider errors never enter the model envelope, events or normal
logs.

## Conversation bootstrap and live handoff

`aiConversationBootstrap` performs a bounded owner/current-policy read and
returns the session shell, newest messages plus backward cursor, durable
watermark, active runs, recent terminal codes, related tool calls, safe
provider activity and reset-required state. It never returns prompts, tool
results, provider payloads, credentials or authorization details. The ORM
implementation uses a bounded optimistic snapshot and retries only when a
field it actually returns changed during assembly.

### The watermark is a resume floor

The returned watermark is captured before the snapshot is assembled and is a
lower bound, not an equality point:

- every durable effect at or before the watermark is reflected in the
  snapshot, so subscribing with `after_sequence = watermark` cannot miss an
  event;
- the message window never leads the watermark, because a new message changes
  the message head and forces the snapshot to be reassembled;
- run and tool-call rows may already reflect an effect after the watermark.
  Both are identified state keyed by row ID, so re-applying the replayed event
  that produced them is idempotent. A client must apply replayed events by ID
  rather than assuming every replayed event is unseen.

Deliberately excluded from the retry predicate are the session stream head,
last-activity timestamp and CAS row version. A coalesced live delta advances
all three at roughly the streaming coalescer rate while an assistant is
answering, but appends only a session event and cannot change anything the
bootstrap returns. Including that churn made the bounded snapshot fail with
`Conflict` for exactly the sessions a user is most likely to open, which a
client cannot distinguish from a disconnection.

A client renders the snapshot, begins durable event replay strictly after the
returned watermark, drains to the captured/current head, then attaches live
subscription using the normal replay-before-live protocol. This avoids replay
from sequence zero and makes completed, failed, cancelled and
recovery-required turns authoritative after browser reconnect, service restart
or HMR. `reset_required` means retention or a configured bootstrap bound
prevents exact reconstruction and the client must discard older derived state.

The tool-call window for the selected runs is read whole, up to the requested
`toolCallLimit` (at most 500), in pages no larger than the database's maximum
page limit. Tool calls set `reset_required` only when the selected runs hold
more calls than that requested window. Before AI 0.149.0 the window was
clamped to the page maximum (100 by default), so a conversation with more tool
calls than that always reported `reset_required`. The terminal-run window still
follows the page maximum and does not set `reset_required`.

## Host readiness and recovery

The existing recovery APIs are sufficient; readiness is a host sequence, not
an automatic consequence of migrations:

1. Apply and verify the exact AI schema module.
2. Call `OrmAiRunService::recover_expired_leases` to bounded convergence before
   accepting work; keep it scheduled while running. Alert on its
   `quarantined` and `skipped` counts: a malformed row is closed for manual
   recovery and a failing row is retried, without stopping the pass. The same
   pass settles expired budget reservations of closed attempts.
3. Reconcile approval and subscription waits so cancellation and terminal
   events converge before claims resume.
4. Drain provider-session cleanup. For every `claim_cleanup`, select the
   deletion adapter from the persisted provider kind and registration
   metadata, open only the exact protected cursor, then record exact absence or
   bounded retry.
5. Compile every target's finished schema, semantics, index and target policy;
   compile their canonical index set and provider registrations; only then
   start ordinary workers and subscriptions.

Changing provider profile, delivery mode, model/effort or projection requires
the same cleanup/absence/rebind lifecycle. So does a catalogue change for
`EagerExact`, `ProviderDeferred` and `ClientDeferred` sessions; a `FixedBroker`
session keeps its binding across catalogue changes. A live provider session is
never relabeled in place.

## Correcting rejected broker query plans

After reauthorizing an exact loaded capability, the broker compiles its plan before
resolver execution. A local input rejection becomes a persisted failure with a
version-2 bounded `correction` string. The closed guidance distinguishes unsupported
relationship collection bounds, unsupported relationship arguments and other plan
mismatches. It never includes model values, hidden schema metadata or internal error
messages. The response uses its own disclosure fingerprint and the ordinary current
access, egress-audit, tool-budget and checkpoint path. If egress or persistence fails,
no correction is delivered. Non-input failures retain the existing version-1 envelope.

The model can submit a new corrected query. The rejected plan executes nothing;
there is no automatic query rewrite, provider replay or repeated completed tool.
Connection paging uses the exact described relationship arguments; an empty
`relationshipMaximumItems.properties` means the broker call must carry no relationship
collection-bound entries.
