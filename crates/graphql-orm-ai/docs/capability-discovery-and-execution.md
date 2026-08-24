---
title: "Capability discovery and execution"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-23
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

## Discovery is not authority

The built-in search uses bounded normalized lexical terms, optional exact
namespace/kind/entity-or-operation filters, deterministic scores and stable ID
tie-breaking. Narrow, explicit list, details, search, keyset-page, or aggregate
intent ranks the matching compiler-owned operation shape before entity,
target/namespace and lexical-description relevance. Every result must retain a
positive lexical match, and relevant non-matching shapes are not discarded. It
needs no embeddings or external database. A host may later
implement the closed current-index-set and authority traits with a reviewed
search service, but returned IDs and fingerprints must still bind to the
canonical set and the candidate's exact owning index.

`AiCapabilityDiscoveryBroker` rehydrates the current principal and reapplies
host scope, target, kind, classification, provider and session policy while
filtering search results, again while loading, and again immediately before
execution. `AiCapabilityAuthorityPolicy` receives the exact owning index on
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
| `ProviderDeferred` | Already-filtered definitions marked for reviewed native deferred loading | Native provider tool search |
| `FixedBroker` | Exact static bootstrap plus frozen discover/describe/execute definitions | Retained sessions whose generated definitions cannot change |

`prepare_client_deferred_continuation` accepts only definitions matching the
crate-owned loaded bindings and the configured selection count. Fixed broker
arguments use a closed scalar argument list and public selection paths; the
selected capability's authoritative compiler validates every name and type.
No broker accepts arbitrary GraphQL, target, alias, fragment, introspection,
SQL, URL or callback.

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
ordinary tool executor. The coordinator recognizes only the three frozen IDs
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

Fixed-broker `describe` returns the exact compact planning schema only on
demand. It also reports conservative compiler-owned maximum root and total
result-record costs plus whether the plan must choose a positive root bound;
these values are planning metadata, not execution authority or observed row
counts. The complete description, including that schema, is bounded by
`AiCapabilityDeliveryLimits::maximum_describe_bytes` (512 KiB by default and
4 MiB at the compiled ceiling). An oversized schema is not truncated or
partially exposed: the response sets `planSchemaAvailable` to `false`, and the
provider must choose another reviewed capability or return a bounded answer.

### Turn amplification

Delivery work consumes normal bounded-loop capacity:

| Path | Extra broker calls before/including application execution |
| --- | ---: |
| Eager exact | 0 |
| Provider deferred | Provider-native discovery only; exact application call still counts normally |
| Client deferred, newly selected capability | 1 discover, then the exact application call |
| Fixed broker, novel capability | discover + describe + execute = 3 |
| Fixed broker, still-loaded capability | execute = 1 |

Every completed broker result is ordinary provider input, so stateless loops
also consume the corresponding continuation turns. Size provider-turn,
tool-call, duration, budget and hierarchical-rule ceilings for this bounded
amplification. The library never raises a host ceiling automatically.

`AiProviderCapabilitySessionBinding` fingerprints delivery mode, the complete
canonical index set, static bootstrap definitions, projection algorithm,
model, reasoning effort and the underlying registration identity. Pass it to
`AiProviderSessionDescriptor::new_with_capability_binding`; the existing
durable registration fence then makes any change cleanup-and-rebind only.

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

## Host readiness and recovery

The existing recovery APIs are sufficient; readiness is a host sequence, not
an automatic consequence of migrations:

1. Apply and verify the exact AI schema module.
2. Call `OrmAiRunService::recover_expired_leases` to bounded convergence before
   accepting work; keep it scheduled while running.
3. Reconcile approval and subscription waits so cancellation and terminal
   events converge before claims resume.
4. Drain provider-session cleanup. For every `claim_cleanup`, select the
   deletion adapter from the persisted provider kind and registration
   metadata, open only the exact protected cursor, then record exact absence or
   bounded retry.
5. Compile every target's finished schema, semantics, index and target policy;
   compile their canonical index set and provider registrations; only then
   start ordinary workers and subscriptions.

Changing provider profile, delivery mode, model/effort, projection or catalogue
requires the same cleanup/absence/rebind lifecycle. A live provider session is
never relabeled in place.
