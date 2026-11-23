---
title: "Capability discovery and execution"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-23
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
stamps out one realistic generated family per target: a wide entity with
forty public scalar fields and five to-many relationships, plus the five
related entities, each with its five generated query roots (list, keyset
and aggregate among them). That is 12 entities and 60 query roots per
target. Each target gets
its own query catalogue and index. The example then drives the version 2
broker the way the coordinator does, on a host that caches its index set.
Each point runs in its own process. Byte sizes are deterministic; times are
for one machine and one build. Approximate tokens are bytes / 4. The *eager
reference* is the size of one exact definition per root: what eager or
provider-deferred delivery would send for the same roots.

Release build, 2026-11-23 (AI 0.155.0, tool profiles 0.28.0):

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

All points fit the default index and index-set limits.

| Targets | Discover p50 / p95 | Startup, one target | Peak memory |
| ---: | ---: | ---: | ---: |
| 1 | 0.04 / 0.05 ms | 19 s | 174 MiB |
| 10 | 0.25 / 0.28 ms | 19 s | 910 MiB |
| 100 | 4.0 / 5.8 ms | 19 s | 7.3 GiB |

Discover time follows the number of matches, not the catalogue as such. The
benchmark's questions share words such as "records" with every entity, and
host policy is applied to every match before the result ceiling (see
[Discovery is not authority](#discovery-is-not-authority)). A host with an
expensive policy should override `authorize_discovery`. Startup and memory
are linear in the number of targets of a given shape: about 0.3 s and
1.2 MiB per wide-entity root. Within one target, catalogue compile time
still grows faster than linearly: 300 roots in one target took about 500 s.
Size a very large single target from the startup benchmark above.

A single-target host can supply its index through `AiCurrentCapabilityIndex`,
and the crate then forms a one-member set on every broker call. Since AI tool
profiles 0.28.0, that set reuses the index's measurements, so each call costs
what it costs a host that caches its set. Describe and `describe_type`
render a signature on every call. Since 0.28.0, a capability recovers the
type names behind its signature once, so only the first describe of a
capability pays for that. Measured on one target with 60 entities (300
roots):

| | Before (tool profiles 0.27.1) | After (0.28.0) |
| --- | ---: | ---: |
| Discover p50, single-index host | 1.78 ms | 0.145 ms |
| Discover p50, host-cached set | 0.164 ms | 0.137 ms |
| Describe p95, first describe of a wide entity | 5.2 s | 4.1 s |
| Describe p95, the same entities described again | 5.0 s | 0.015 s |

The benchmark also reports whether a keyset operation offers a
continuation. It does not: the page argument is server-owned, so a model
reads only the first page of a keyset or list root. Continuation cursors are
planned for the next fingerprint-breaking release.

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
the batch unless decision `i` names ranked match `i`. The default
`authorize_discovery` calls `authorize` per match with up to 16 calls in
flight; hosts with an expensive check override it to decide the batch from one
current lookup. A broad query can send up to the index set's `maximum_entries`
matches to policy. `with_maximum_discovery_authorizations` optionally refuses
such a query before any policy call with the stable
`AI_CAPABILITY_DISCOVERY_TOO_BROAD` input error. Report a denial as
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
| `FixedBroker` | Exact static bootstrap plus frozen discover/describe/describe_type/execute definitions | Retained sessions, and large surfaces by default |

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
the session binding or the four broker definitions.

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
provenance refuses it. Mutations and subscriptions keep their own approval
and wait contracts and cannot enter the broker.

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
ordinary tool executor. The coordinator recognizes only the four frozen IDs
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

`AI_CAPABILITY_BROKER_VERSION` is 2. Its four definitions are discover,
describe, `graphql.capabilities.describe_type` and execute.

- **Discover** returns, for an entity, one `generated_entity` candidate with
  its `operations` (each root's name and shape). A root without an entity and
  a brokered static tool are candidates of their own. `kind` filters:
  `generated_query` (every generated read), `generated_entity` or
  `reviewed_static`.
- **Describe** loads the candidate and returns one entry per operation under
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
policy fingerprints recorded at describe, before anything runs.
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

### Turn amplification

Delivery work consumes normal bounded-loop capacity:

| Path | Extra broker calls before/including application execution |
| --- | ---: |
| Eager exact | 0 |
| Provider deferred | Provider-native discovery only; exact application call still counts normally |
| Client deferred, newly selected capability | 1 discover, then the exact application call |
| Fixed broker, novel capability | discover + describe + execute = 3, plus one `describe_type` per expanded type |
| Fixed broker, still-loaded capability | execute = 1 |

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
| `FixedBroker` | `AI_CAPABILITY_BROKER_VERSION` (2) | keeps the retained session |

Bumping the broker version changes the fixed broker definitions and every
fixed-broker binding, so a session bound under version 1 matches no current
binding and rebinds once; the Grok registration check also refuses a tool set
that carries the version 1 definitions. The four fixed broker definitions are
the same for every catalogue, so a
retained fixed-broker session, and the chat it carries, survives a schema
change in any target. The index is checked where it matters instead: every
describe and execute compares the current index-set, index, schema, semantic
catalogue, entry, capability, target-policy and host-policy fingerprints with
the ones the selection was made under.

### Stale selections

Both outcomes are retryable, execute nothing, and carry a fixed `correction`
(failure envelope version 2) telling the model what to do next.

| Model-facing code | When | Model recovery |
| --- | --- | --- |
| `capability_stale` | The run holds the candidate or loaded capability, and a fingerprint above has changed since (at describe, `describe_type` or execute). | Discover again, describe the new candidate, use the new signatures and `loadedReference`. |
| `selection_stale` | The run does not hold the selection: discovery never returned that candidate to it, or the `loadedReference` is unknown or expired. | Discover (or describe) again. |

`capability_stale` is reported only for a candidate the current principal was
already shown, or a capability it already loaded, so it reveals nothing about
capabilities the principal cannot see. `selection_stale` says nothing about
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
