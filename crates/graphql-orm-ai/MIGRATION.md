---
title: "Migration Guide"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-22
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

`graphql-orm-ai` is not yet published. This guide is still mandatory so early
Git consumers and disposable test deployments can track schema and API changes
without guessing.

Migration entries preserve the dependency and schema facts for the checkpoint
they describe. For the current workspace baseline and active delivery gates,
use [implementation status](docs/implementation-status.md) and the central
[AI production-readiness plan](../../docs/plans/active/ai-production-readiness/README.md).

## 0.88.2 to 0.88.3: agql-auth 0.16 type-universe alignment

Adopt `graphql-orm-ai` 0.88.3 from one reviewed full monorepo revision and
align every direct host dependency on `agql-auth` to version 0.16.0 at exact
revision `3bc38cd94794f1e868a9cc3a5551047b95a32105`. Mixing that revision with
the earlier 0.15 workspace pin creates distinct principal types and must not
be worked around with path overrides or duplicated conversion layers.

The AI schema module remains **0.63.0**. There is no database, data, table,
column, index, constraint, backfill, GraphQL SDL, protected-payload, backup or
restore migration. Version 0.16 adds host-configured exact-only matcher
requirements; `graphql-orm-ai` does not choose or relax those resource-server
rules and requires no AI provider or coordinator API change.

## 0.88.1 to 0.88.2: retained capability-session admission

Adopt `graphql-orm-ai` 0.88.2 from one reviewed full monorepo revision. The AI
schema module remains **0.63.0**. There is no database, data, table, column,
index, constraint, backfill, GraphQL SDL, protected-payload, backup or restore
migration.

Codex app-server hosts that persist
`AiProviderSessionDescriptor::new_with_capability_binding` descriptors must
install the same exact `AiProviderCapabilitySessionBinding` on the immutable
`AiCodexAppServerRegistration` before constructing the provider and cleanup
service. The registration rejects a binding unless its model and reasoning
effort are admitted and its embedded registration identity equals the exact
effort-bound executable, sandbox, launch-profile, bootstrap, and adapter
identity.

The admission overlay deliberately does not alter that underlying identity:
the binding already incorporates it, avoiding a circular fingerprint. Hosts
must not accept or reconstruct a bare final fingerprint. Existing raw
registration descriptors remain compatible.

## 0.88.0 to 0.88.1: Codex FixedBroker schema projection

Adopt `graphql-orm-ai` 0.88.1 from one reviewed full monorepo revision. The AI
schema module remains **0.63.0**. There is no database, data, table, column,
index, constraint, backfill, GraphQL SDL, protected-payload, backup or restore
migration.

Hosts using the Codex app-server adapter should rerun readiness against their
complete capability surface. The adapter now projects the crate-authored
FixedBroker discovery, describe, and execute definitions without rewriting
their bounded nullable scalar `type` arrays. Codex 0.148.0 was measured to
accept the preserved form and deliver the offered dynamic tool directly.

No host API changes are required. Unknown, duplicate, non-nullable, structural,
or otherwise malformed type unions continue to fail closed before a provider
turn. Keep the 0.88.0 closed launch profile, full-surface readiness guard, and
negative native-item checks unchanged.

## 0.87.0 to 0.88.0: direct GPT-5.6 dynamic tools on Codex 0.148.0

Adopt `graphql-orm-ai` 0.88.0 from one reviewed full monorepo revision. The AI
schema module remains **0.63.0**. There is no database, data, table, column,
index, constraint, backfill, GraphQL SDL, protected-payload, backup or restore
migration.

Hosts must continue applying
`AiCodexAppServerLaunchProfile::codex_arguments()` unchanged. On Codex 0.148.0,
the profile now omits only `--disable code_mode_host`: an otherwise identical
GPT-5.6 Luna probe completed without calling its offered direct tool when that
argument was present, and emitted `dynamicToolCall` / `item/tool/call` when it
was absent. Do not infer that Code Mode or another native surface is admitted.
The actor still sends `features.code_mode_host=false`,
`features.code_mode=false`, `features.code_mode_only=false`, and every other
closed feature setting per thread; the process sandbox and protocol actor
continue to deny shell, file, MCP, browser, hosted-search, collaboration,
image, and arbitrary server-request items.

Before changing Codex versions, run a retained direct-tool readiness probe and
the negative native-item lifecycle tests. A non-dynamic route or any native
item must fail readiness rather than falling back to Code Mode or execution.

## 0.86.0 to 0.87.0: retained dynamic-tool readiness input

Adopt `graphql-orm-ai` 0.87.0 from one reviewed full monorepo revision. The AI
schema module remains **0.63.0**. There is no database, data, table, column,
index, constraint, backfill, GraphQL SDL, protected-payload, backup or restore
migration.

Hosts that readiness-test an installed Codex app-server may construct a
bootstrap-fingerprint-bound retained turn with
`AiCodexAppServerTurnInput::retained_dynamic_tool_readiness_probe`. Supply the
same complete dynamic-tool definitions and reasoning effort used to create the
empty retained thread, plus the exact tool ID that the model must select. The
constructor only validates input; it does not grant process, provider,
application-tool, egress or result authority. After the actor admits that
exact call, use `dynamic_tool_readiness_response` to return the sole fixed
`{"ready":true}` result, finish the turn and delete the thread. The actor
rejects this response on an ordinary turn or another tool, and the result
cannot contain application data. Keep the probe inside the reviewed host
sandbox.

## 0.85.0 to 0.86.0: metered stateless native-item refusal

Adopt `graphql-orm-ai` 0.86.0 and `graphql-orm-ai-tool-profiles` 0.9.0 from one
reviewed full monorepo revision. The AI schema module remains **0.63.0**. There
is no database, data, table, column, index, constraint, backfill, GraphQL SDL,
protected-payload, backup or restore migration.

An adapter may end a dispatched stream with
`ProviderError::StatelessNativeItemRejected` only after it has emitted an
authoritative `Usage` and `ResponseCompleted`, and only when its deployment
contract proves the refused provider-native item was contained. The executor
accepts that claim only for `ModelContinuationMode::StatelessReplay` with no
provider cursor, assistant text, citation, application-tool event,
provider-hosted-tool event, or unknown event. It settles the authoritative
usage, commits the reservation, and returns
`AiError::StatelessNativeItemRejected`. Do not use either variant for a parser
error, incomplete stream, retained session, unmetered response, or an operation
that might have escaped the provider sandbox.

The read-only and supervised coordinators close this proof as `Failed` with
outcome code `provider_native_item_rejected`. That code is explicitly admitted
by `classify_run_retry` when no assistant output exists, so the failure record
offers a new run over the same user message. All generic provider errors still
close for recovery because their effects remain uncertain. Clients that
previously rendered this exact adapter refusal as
`provider_turn_uncertain` should render the new bounded failure code and expose
their existing retry action.

## 0.84.0 to 0.85.0: executable federated bounded capability delivery

Adopt `graphql-orm-ai` 0.85.0 and `graphql-orm-ai-tool-profiles` 0.8.0 at one
reviewed full monorepo revision. The AI schema module remains **0.63.0**. There
is no database, data, table, column, index, constraint, backfill, GraphQL SDL,
protected-payload, backup or restore migration.

The delivery types introduced in 0.81 now have an ordinary durable execution
path. Build one complete current index per owning logical target, combine them
with `AiCapabilityIndexSet::compile`, and supply the set through
`AiCurrentCapabilityIndexSet`. Use the aggregate set fingerprint for the
delivery surface and retained session binding. Do not synthesize a combined
schema/catalogue fingerprint; global capability-ID collisions fail readiness.
Existing single-index implementations retain a compatibility adapter.
`AiCapabilityAuthorityPolicy::authorize` now also receives the exact owning
`AiCapabilityIndex`; use its logical target and fingerprints when applying
current target policy. Do not infer an owner from capability naming.

Build the exact currently eligible read definitions, then construct one
`AiCapabilityDeliveryTurn::select`. Build the provider request from
`delivery.current_surface()` using
`AiProviderCallPlan::new_with_capability_surface`, and attach a clone of the
same delivery value with `AiReadOnlyAgentTurnPlan::with_capability_delivery`.
Do not construct or edit broker definitions.

Client-deferred installation is required after every accepted discovery,
including a discovery that returns no currently permitted candidates. The
crate then clears previously installed generated definitions and retains only
the exact static bootstrap and discovery definitions.

Override
`AiReadOnlyAgentTurnPlanner::continuation_plan_with_capability_delivery` when
using capability delivery. The optional value is the crate-owned run state
that survived the preceding turn. After client-deferred discovery it already
contains the exact loaded definitions. Use its `current_surface()` with
`AiProviderCallPlan::new_continuation_with_capability_surface`, then attach a
clone of that same delivery turn. Recreating state under the same public
fingerprint, retaining an earlier surface, or returning a plan without the
delivery turn fails closed.

Hosts that constrain tools through hierarchical agent rules must admit the
exact broker-definition fingerprints present in the selected surface as
approval-free read tools. This grants no application authority: discover and
describe return bounded metadata, and execute still performs fresh principal,
policy, target/schema/catalogue/capability, resolver, disclosure, egress,
budget and fence checks.

Size loop limits for delivery amplification. A novel fixed-broker capability
uses discover, describe and execute (three application-tool calls); a
completed-turn adapter also needs a provider continuation for each result,
while an in-turn dynamic adapter can keep the same provider turn open. A
still-loaded capability reuses execute only. Client-deferred use adds discovery
and one continuation before the exact tool call. All calls count against the
existing tool-call, provider-turn, duration, budget and rule ceilings;
increasing them is a host policy decision, never an automatic library bypass.

Size `AiCapabilityDeliveryLimits::maximum_describe_bytes` together with the
ordinary application-tool result and provider-input ceilings. It bounds the
complete on-demand planning contract (512 KiB by default); a larger exact
schema is omitted with `planSchemaAvailable: false`, never truncated.

Changing delivery mode, index set, static bootstrap tools, projection, model,
reasoning effort or registration identity changes the provider capability
session binding. Retained sessions with a different binding remain
cleanup-only and must reach exact absence before rebind. Process restart loses
only the bounded non-authoritative broker cache: a later describe/execute
returns a safe stale-selection result and the model must rediscover.

## 0.83.0 to 0.84.0: budget reclamation and pre-transport denial (schema 0.62.0 to 0.63.0)

### Schema module

The AI schema module advances **0.62.0 to 0.63.0**. It adds **no entity, no
column, and no constraint**, and needs **no data migration or backfill**.
`graphql_orm_ai_budget_reservations` exposes `scope_kind`, `scope_id`,
`tenant_id`, and `expires_at` to typed internal predicates and gains the composite index
`idx_graphql_orm_ai_budget_reservations_scope_state`
(`scope_kind, scope_id, tenant_id, state, expires_at`), so the new stranded-reservation
report is a bounded indexed read rather than a table scan. Existing rows and
events remain readable at the previous module version and after the upgrade.
Apply and verify the module before serving traffic.

### Source-breaking changes

`AiConfigurationAction` gained the variant `ManageBudgetReclamation`. Any
exhaustive `match` in a host `AiConfigurationAccessPolicy` must handle it.
Authorize it only for the administrators you would trust to charge an
unprovable provider turn to a budget; a host that does not want the surface
returns `false` and it stays closed.

`AiConfigurationService` gained `budget_scope_capacity` and
`reclaim_budget_reservation`. Both have fail-closed default implementations
that return `AiError::InvalidConfiguration`, so an existing custom
implementation still compiles and does not silently gain the surface.

### New public API

- `AiBudgetScopeCapacityView`, `AiBudgetPolicyCapacityView`, and
  `AiBudgetReservationCapacityView` are the redacted capacity views. They carry
  capacity accounting, reservation state, expiry, owning-run linkage and CAS
  versions only; never a prompt, transcript, provider payload, principal
  identity, or credential. `reclaimable` identifies a deployment/time/run
  candidate only; mutation authorization, recent MFA, CAS, scope and stored
  graph integrity are rechecked separately.
- `ReclaimAiBudgetReservationInput { scope, reservation_id, expected_version }`.
- `AiBudgetReclamationLimits::new(minimum_expired_age, maximum_reservation_scan)`
  and `OrmAiConfigurationService::with_budget_reservation_reclamation`.

### New GraphQL

`AiConfigurationQueryRoot` gains `aiBudgetScopeCapacity(scope)`, authorized by
the existing `ReadBudgetPolicies` action. `AiConfigurationMutationRoot` gains
`reclaimAiBudgetReservation(input)`, authorized by `ManageBudgetReclamation`
plus recent MFA plus the deployment opt-in. No existing field changed.

To enable reclamation:

```rust,ignore
let configuration = OrmAiConfigurationService::new(/* ... */)
    .with_budget_policy_management(policy_limits)
    .with_budget_reservation_reclamation(AiBudgetReclamationLimits::new(
        time::Duration::hours(6),
        200,
    )?);
```

Without that call, `aiBudgetScopeCapacity` still works and every reservation
reports `reclaimable: false`, while `reclaimAiBudgetReservation` fails closed
as invalid configuration.

### Behavioural changes with no API change

A run returning the proof-bearing `AiError::PreTransportBudgetDenied` now
terminates `Failed` with outcome code `provider_budget_denied`. The executor
may produce that variant only when reservation failed before dispatch or when
an already-created reservation was durably released before dispatch. It
previously terminated `RecoveryRequired` with `provider_turn_uncertain`, which
told users a proven local refusal could not be confirmed and made the run
permanently unretryable. `provider_budget_denied` is on the retryable failure
allowlist, so `AiRunFailure.admission` is `AiRunRetryAdmission::Allowed` and a
client may author a new run for the same durable user message once capacity
exists. A client that keys UI text off `provider_turn_uncertain` for this case
must move it to the new code. The supervised coordinator makes the same
distinction. Generic `AiError::BudgetDenied`, including post-transport dynamic
tool-call and rule ceilings, remains uncertain and must not use this path.

A provider call whose post-reservation authorization binding fails now releases
its reservation. Nothing had been dispatched, so the release is provable.

### What deliberately did not change

Reclamation commits; it never releases. An `uncertain` or `reserved`
reservation carries no durable proof that the provider was not reached, so
releasing it would fabricate an absence proof. Committing the held estimate can
only over-count.

Reclamation therefore **does not create headroom**: the reserved column falls
by exactly the amount the committed column rises, and `reserved + committed`
against the ceiling is unchanged. A deployment whose ceiling is already
exhausted by stranded reservations raises or replaces the policy through
`upsertAiBudgetPolicy`. The value of reclamation is that held capacity becomes
accountable, reportable, and finite instead of permanently unreachable, and
that the condition is now observable before it becomes an outage.

Reclamation is not automatic. Expired-lease recovery and every other
maintenance pass are unchanged. Automating a commit would free no headroom
while adding an unattended writer of authoritative usage facts attributed to an
absent principal; the decision to charge an unprovable turn stays with an
authorized, MFA-current, audited human.

## 0.82.0 to 0.83.0: settled retained Codex interruption

Adopt `graphql-orm-ai` 0.83.0 at one reviewed full monorepo revision.

### Schema module

The AI schema module advances **0.61.0 to 0.62.0** as a persistent-semantic
version. There is no DDL, table, column, index, constraint, protected-payload,
row-rewrite, or backfill change. Apply and verify the module while AI workers
are stopped, then restart all workers on the same revision. The semantic bump
records that an existing provider-session binding can now advance its durable
watermark and transcript fingerprint after a settled interrupt.

### Provider adoption

`AiProviderRunInterruptOutcome` gains the non-exhaustive
`RequestedSettled` variant. Ordinary adapters should continue returning
`Requested`; only an adapter with exact acknowledged-interrupt, unresolved-tool
absence, and provider-thread discard proof may report the new variant.

`AiRuntime::interrupt_all_provider_runs` keeps its existing count result. The
new `interrupt_all_provider_runs_with_settlement` returns the aggregate proof
for coordinator implementations. A caller must still apply durable evidence;
provider acknowledgement alone never permits retaining a thread.

The Codex app-server proof is version-observed for `codex-cli 0.148.0` with
`gpt-5.4`, not guaranteed by the empty `turn/interrupt` response. Re-run the
documented interrupt probe before changing the Codex version or admitted model.
The adapter fails closed when a dynamic call remains in flight or starts after
interruption begins. The ORM implementation then transactionally rechecks the
cancelled run, exact claim, message watermark, and absence of an assistant
message, tool call, or checkpoint before retaining the binding. A failure uses
the existing disclosed cleanup-required path.

The interrupted user message remains in Codex and in the durable transcript,
with no assistant reply. That unanswered prompt is expected and is incorporated
into the provider-session transcript fingerprint before a later run resumes.

## 0.81.0 to 0.82.0: session reliability and failure disposition

Adopt `graphql-orm-ai` 0.82.0 at one reviewed full monorepo revision.

### Schema module

The AI schema module advances **0.60.0 to 0.61.0** and adds one entity,
`graphql_orm_ai_run_failure_dispositions`, with a unique index on
`source_run_id` and a session/decision index. Apply and verify the module
before serving traffic. There is no backfill, no column change to an existing
table, and no protected-payload migration. Existing rows and events remain
readable.

### Source-breaking changes

`AiAgentProviderTurnExecutor::interrupt_run` now returns
`AiRunInterruptSettlement` instead of `()`. Existing implementations that
interrupt without proving settlement should return
`AiRunInterruptSettlement::RequestedUnsettled`, and one that finds no live
resource should return `NotActive`. Do not return `Settled` unless the adapter
can prove the interrupted turn left the provider's retained thread consistent
with the durable transcript; `retains_thread()` is the only thing that keeps a
binding, and it fails closed.

`AiSessionEventEnvelope` gained a nullable `closed` field. Build envelopes with
`AiSessionEventEnvelope::delivered` or `AiSessionEventEnvelope::ended` instead
of struct literals.

### Behavioural changes with no API change

`conversation_bootstrap` no longer returns `Conflict` while an assistant is
streaming. Its `watermark` is now documented as a **resume floor** rather than
an equality point. Subscribe with `after_sequence = watermark`; no event at or
below it is missing, and the message window never leads it, but run and
tool-call rows may already reflect an event after it. Apply replayed events by
identifier so re-applying one the snapshot already reflects is idempotent. A
client that assumed every replayed event was unseen must be updated.

Session-event streams now tolerate a briefly unavailable authorization
dependency within a bounded per-session jittered grace window, and emit a typed
close envelope before ending. Authoritative denials are unchanged: the stream
still fails immediately, and the existing `AiError` still follows the close
envelope, so a client reading only errors keeps working.

A run whose provider-session cleanup stays pending past its retry allowance now
closes as `Failed` with `provider_session_cleanup_unavailable` instead of
expiring into `RecoveryRequired`. That code is retryable, because nothing
executed.

### New GraphQL surface

`retryAiRun` and `acknowledgeAiRunFailure` are additive; regenerate typed and
PascalCase clients. Install `Arc<dyn AiRunDispositionService>` in schema data
or both mutations return a configuration error. `OrmAiRunDispositionService`
is the generated-ORM implementation.

Four event types are additive on the existing session stream:
`run_retry_queued`, `run_failure_acknowledged`, `provider_session_reset`, and
`provider_session_rebound`. Clients that reject unknown event types must be
updated to ignore them.

The `run_failed` and `run_recovery_required` payloads advance from the tagged
`...-v1` shape to `...-v2` and carry a `failure` record. Readers accept both
shapes; a v1 payload written before this release stays readable.

## 0.80.0 to 0.81.0: capability discovery and durable provider loops

Adopt `graphql-orm-ai` 0.81.0 and
`graphql-orm-ai-tool-profiles` 0.6.0 at one reviewed full monorepo revision.
The AI schema module remains `0.60.0`: there is no database/data/table/column/
index/constraint/backfill or protected-payload migration. Do not rerun or
invent a schema module. The new `aiConversationBootstrap` field is an additive
GraphQL API change; regenerate typed clients and PascalCase clients if used.

This pre-1.0 minor is source-breaking for public struct literals. Add
`defer_loading: false` to direct `ModelToolDefinition` literals. Direct
`OpenAiProviderConfig` literals add `native_tool_search_models`; prefer
`OpenAiProviderConfig::new`. Direct `ProviderCapabilities` literals add
`capability_delivery_modes` or use `..Default::default()`. Provider trait
implementations may retain the default dispatch bridge, but production
adapters should implement `prepare_dispatch` for every validation step they
can prove occurs before transmission.

Rebuild all generated capabilities and use compact v3 plan definitions as
described in the tool-profile migration guide. Use
`select_capability_delivery_mode` and `prepare_capability_delivery_surface`;
prompt text must not select a mode. Opt native OpenAI tool search in only for
exact reviewed compatible model IDs:

```rust
let config = OpenAiProviderConfig::new(secret_ref)
    .with_native_tool_search_models(["gpt-5.4".to_owned()])?;
```

OpenAI strict projection is crate-owned. Keep the canonical JSON Schema
unchanged and pass it in `ModelToolDefinition`; the adapter projects optional
values to a reversible required/nullable form and normalizes returned
arguments before the canonical validator and ordinary tool broker.

Retained provider definitions are compatibility-sensitive. Build
`AiProviderCapabilitySessionBinding` from the selected mode, canonical index,
static bootstrap tools, projection version, exact model/effort and underlying
registration identity, then use
`AiProviderSessionDescriptor::new_with_capability_binding`. Existing 0.80
sessions lack that complete registration fingerprint and are cleanup-only:
stop assigning new turns, invoke the persisted provider-kind/registration
deletion adapter through the ordinary cleanup worker until exact absence is
recorded, then bind a new session. Never rewrite a stored cursor or fingerprint.

Before declaring workers ready after deployment or restart:

1. Apply/verify `AiSchemaModule` 0.60.0.
2. Run `OrmAiRunService::recover_expired_leases` to convergence and reconcile
   approval/subscription cancellation and terminal events.
3. Drain `AiProviderSessionService::claim_cleanup` work, selecting the deletion
   adapter from each persisted descriptor, and record exact absence or bounded
   retry backoff.
4. Compile/verify the current schema, semantic catalogue, compact index,
   target policy and provider registrations.
5. Start ordinary bounded recovery and cleanup loops before accepting run
   claims. Migration completion alone is not readiness.

Provider inference manifests must come from the final
`AiProviderCallPlan::egress_requirement`, after the opaque continuation and
exact loaded definitions are installed. A pre-dispatch rejection is retryable
and releases unused budget. A failure after possible transmission remains
uncertain and follows the retained-session recovery policy.

## 0.80.0: model reasoning effort (schema 0.59.0 to 0.60.0)

Adopt `graphql-orm-ai` 0.80.0 at one reviewed full monorepo revision and apply
`AiSchemaModule` 0.60.0. The module adds a non-null
`AiBudgetReservationRecord.reasoning_effort` column with database default
`unspecified`. Existing reservations, protected provider-result format 1, and
protected continuation formats 1/2 therefore retain the pre-0.80
provider-default behavior. New protected provider results use format 2 and new
continuations use format 3 so their selected effort participates in protected
checkpoint fingerprints. No prompt, reasoning, token, cursor, tool, or result
content is migrated.

This pre-1.0 minor is source-breaking for public struct literals. Add
`reasoning_effort: ModelReasoningEffort::Unspecified` to existing
`ModelRequest` and `AiBudgetReservationRequest` literals. Prefer
`OpenAiProviderConfig::new`; direct config literals must add
`reasoning_effort_profiles`. Direct `ProviderCapabilities` literals must add
the same field or use `..Default::default()`. Implementations of
`AiCodexAppServerRunProcess::create_empty_thread` must accept and preserve the
new typed effort argument. Calls to
`AiCodexAppServerProtocolActor::start_persistent_empty_thread` must pass the
validated selected effort.

Build exact reviewed profiles rather than assuming a universal model matrix:

```rust
use graphql_orm_ai::{
    ModelReasoningEffort as Effort, ModelReasoningEffortProfile,
};

let profile = ModelReasoningEffortProfile::new(
    "gpt-5.6-sol",
    [Effort::None, Effort::Low, Effort::Medium, Effort::High,
     Effort::XHigh, Effort::Max],
    Effort::Medium,
)?;
```

Install native profiles with
`OpenAiProviderConfig::with_reasoning_effort_profiles(vec![...])`. Install one
exact logical-model profile in each Codex registration with
`AiCodexAppServerRegistration::with_reasoning_effort_profile(profile)`. Read
the admitted UI set and default from
`AiRuntime::provider_capabilities(kind)` and
`ProviderCapabilities::reasoning_effort_profile(model)`; do not accept a
browser-authored string outside `profile.supported()`. `Unspecified` is not an
explicit supported option: it means omit the provider override and use the
registration/provider default. Explicit `None` remains a different value.

For every initial and continuation plan, set the same selected value on
`ModelRequest::reasoning_effort` and
`AiBudgetReservationRequest::reasoning_effort`. Custom budget services must
use `AiBudgetReservation::new_reserved_with_reasoning_effort` when restoring
an explicit selection, then
`authorize_provider_call_with_reasoning_effort`. The compatibility methods
authorize only `Unspecified`. The crate binds effort into request hashes,
budget proof, provider-result checkpoints, continuation checkpoints and
provider-session fencing; a planner must not rewrite it during retry or
recovery.

Codex app-server generated schema places the optional override at
`turn/start.params.effort` and describes it as applying to the current and
subsequent turns. Retained sessions are therefore effort-frozen. Construct the
descriptor with the effort-bound value:

```rust
let registration_fingerprint =
    registration.provider_session_fingerprint(selected_effort)?;
let descriptor = AiProviderSessionDescriptor::new(
    ProviderKind::LocalHarness,
    registration.provider_profile_id(),
    registration.logical_model(),
    registration_fingerprint,
    registration.protocol_version(),
    transcript_fingerprint,
)?;
```

Do not use `registration.identity()` as the descriptor fingerprint. Changing
the effort or reviewed profile must enter the existing cleanup/absence/rebind
flow and create a new empty thread; it must never resume the old cursor.
Registration identity is now v4, so every pre-0.80 Codex retained cursor is
incompatible with execution even when the new selection is `Unspecified`.
Before a rolling upgrade, stop creating Codex sessions and drain them with the
old deployment, or let 0.80's deletion service consume the legacy v3
fingerprint as cleanup-only evidence. Wait for exact absence, then issue the
ordinary rebind and create a descriptor with
`provider_session_fingerprint(selected_effort)`. Do not mix old and new
workers on one resumable cursor.

A reviewed host GPT-5.6 profile may register `gpt-5.6-sol`,
`gpt-5.6-terra`, and `gpt-5.6-luna` with the explicit set `none`, `low`,
`medium`, `high`, `xhigh`, `max` and default `medium`, as documented by the
[OpenAI GPT-5.6 guide](https://developers.openai.com/api/docs/guides/latest-model).
That is deployment registration data, not a hard-coded library catalogue.
The Codex generated schema itself accepts a non-empty string; the crate's
closed enum and exact profile provide the narrower provider-specific
projection. Keep visible summary selection independently disabled or
capability-checked.

## 0.79.0: safe tool failures and provider-session deferral (schema remains 0.59.0)

No database, data, GraphQL SDL, backup, restore or AI schema-module migration
is required. Existing provider-session cursors do not need to be drained.

Adopt `graphql-orm-ai` 0.79.0 with `graphql-orm` 0.23.0,
`graphql-orm-ai-tool-profiles` 0.5.0, and `graphql-orm-operation-catalog`
0.3.0 at one reviewed full monorepo revision. Rebuild generated query
capabilities so server-fixed object lists compile without paging arguments.

Hosts that inspect `AiError` should match `ProviderSessionDeferred` for a
later run that arrives while the previous retained cursor is cleaning up.
That run is retry-scheduled; do not treat it as `RecoveryRequired`. After
exact absence, the existing `RebindAllowed` path creates a fresh empty
thread. The host planner still supplies the next turn from the durable
transcript watermark.

Safe dynamic-tool failures return
`{ "version": 1, "ok": false, "code": "...", "retryable": ... }` to the
provider. Do not parse raw `AiError` strings for model-visible text.

## 0.78.5: typed newly-bound Codex turn rejection (schema remains 0.59.0)

No database, data, GraphQL SDL, backup, restore or AI schema-module migration
is required. Existing provider-session cursors do not need to be drained.

A retained first turn may now carry the exact registration bootstrap blocks
in `ModelRequest::instructions` or leave that field empty. Hosts that already
copy the frozen bootstrap into the planning request do not need a definition
rewrite, activation flag, raw constructor, or fresh-thread fallback. Any other
instruction text is still rejected.

Hosts that inspect `ProviderError` should match
`NewlyBoundTurnRejected(AiCodexBoundTurnRejection)` for the first turn after
`create_empty_session`. Coordinator hosts that only see `AiError::ProviderFailed`
can override
`AiProviderFailureDiagnosticSink::record_newly_bound_turn_rejection`. The
phase names are stable machine codes such as
`codex_bound_turn_frozen_definition_mismatch` and
`codex_bound_turn_bootstrap_fingerprint_mismatch`. They never include
cursors, prompts, tool names, arguments, or provider payloads.

Pin `graphql-orm-ai` `0.78.5` at the reviewed full monorepo revision.

## 0.78.4: Codex omitted-required object projection (schema remains 0.59.0)

No database, data, GraphQL SDL, backup, restore or AI schema-module migration
is required. The authoritative generated query argument schema, operation,
schema and catalogue fingerprints, target binding, principal rehydration,
delegated authority, resolver authorization, disclosure, result bounds and
execution validation are unchanged.

The Codex adapter now treats an omitted object `required` keyword as the
canonical empty set and writes `"required": []` into the projected app-server
schema. `additionalProperties` remains exactly `false`. Malformed `required`
arrays, unknown keywords and unbounded object shapes still fail closed.

Existing provider-session cursors do not need to be drained or recreated.
Previously admitted static and profile-generated definitions already emitted
an explicit `required` array, so their projection fingerprints are unchanged.
Generated query capabilities that omitted `required` never passed thread
create, so they have no retained cursor to rewrite. After upgrade, recreate
only those failed readiness probes so the same registered definition set can
be installed.

Pin `graphql-orm-ai` `0.78.4` at the reviewed full monorepo revision. Leave
`graphql-orm-ai-tool-profiles` at `0.4.1` unless a later companion change
requires a coordinated pin. Do not rewrite host-owned argument schemas or
weaken fingerprint checks.

## 0.78.3: exact remote read-capability delegation (schema remains 0.59.0)

No database, data, GraphQL SDL, backup, restore or AI schema-module migration
is required. Existing static-only, generated-only and mixed-read provider-call
constructors are source compatible. Initial, provider-retained and stateless
continuations retain the same registered tool IDs and fingerprints.

Deploy the updated remote issuer and adapter together. The serialized
`AiRemoteGraphqlDelegationRequest` now has one required
`capability_binding`, so its `stable_hash()` intentionally changes. Issuers
should inspect `request.capability_binding().kind()` and its read-only
accessors. Preserve the existing exact static allowlist for
`StaticOperation`. For `GeneratedQuery`, validate the exact capability ID and
fingerprint plus logical target, finished-schema fingerprint, semantic
catalogue/operation fingerprints and root field against deployment-owned
active metadata before minting narrowly scoped short-lived authority. Never
infer generated eligibility from `AiQuery_*` operation-name syntax.

Do not construct a binding in host code or add a continuation side channel.
The authenticated runtime supplies it only after catalogue compilation and
current generated-target admission. Direct calls to the remote adapter's raw
`GraphqlRequestContextFactory::build` hook now fail closed; execute through
`AuthenticatedToolBridge`/`AiRuntime`, which invokes the crate-authored
registered-binding hook after fresh principal and current host policy checks.
Remote mutation, subscription and internal-operation delegation remains
closed. Existing local supervised mutation execution is unchanged.

Delegated credentials are ephemeral, so there is no durable credential or
request-hash migration. Drain in-flight remote requests during a rolling
upgrade if the deployment transports the serialized request between separately
versioned processes.

## 0.78.2: mixed static and generated read plans (schema remains 0.59.0)

No database, data, GraphQL SDL, backup, restore or AI schema-module migration
is required. Existing static-only `AiProviderCallPlan::new_with_tools` and
generated-only `new_with_generated_queries` call sites remain valid.

Hosts that deliberately expose both kinds in one read-only provider turn may
replace the initial constructor with `new_with_read_capabilities`, passing the
same registered `AiToolCatalog`, exact static `AiToolPolicySet` and exact
`AiGeneratedGraphqlTargetPolicySet` already used by the runtime. Replace a
stateless or checkpoint-adopted read-tool continuation constructor with
`new_continuation_with_read_capabilities`. Provider-retained execution uses
that same continuation constructor because the crate-owned opaque
`AiAgentContinuation` selects provider-response versus bounded stateless
history; there is no host-authored retained-result route.

Do not copy generated capabilities into static descriptors or create static
policy rows for them. Catalogue registration remains discovery only, and the
ordinary fresh-principal authorization and resolver path remains unchanged.

## 0.78.1: atomic retained approval waits (schema 0.58.0 to 0.59.0)

Apply `AiSchemaModule` `0.59.0` with run, approval, provider-session,
retention, restore and cleanup workers stopped. The module advances because a
retained approval now gives existing checkpoint and attempt-outcome rows a new
authoritative persistent meaning: the approval, protected
`approval_wait_parked` checkpoint, nonterminal source-attempt outcome, latest
checkpoint pointer and ordinary lease release commit atomically.

This migration changes no table, column, index, constraint, GraphQL SDL or
public browser payload. Existing rows require no rewrite, data migration or
backfill. Historical waits are not manufactured or made resumable. After the
module is applied, newly parked retained approvals use the exact graph;
stateless approvals retain their existing in-attempt behavior.

An approved retained wait is not claimable until its provider binding confirms
the exact parked checkpoint. A crash between the atomic wait transaction and
confirmation remains repairable by the bounded provider-session maintenance
pass. `claim_next_approved` then creates a fresh attempt/generation and
refences the exact call and step; it never reopens the closed source attempt.

## 0.78.0: retention traversal under bounded ORM pagination (schema remains 0.58.0)

No schema or data migration is required. Retention continues to honor the
database's configured public pagination maximum. Internal completeness proofs
now traverse generated ORM pages up to the existing per-session retention
bound before mutating any all-or-nothing set. Terminal subscription waiter and
adoption tombstones are removed in bounded batches across retention passes,
and session finalization independently proves that no waiter, adoption, run,
or protected checkpoint remains.

Deployments may keep pagination maxima below the retention bounds; they no
longer need to align those settings to make the look-ahead row visible. The
change broadens no GraphQL operation, retention limit, raw database access, or
protected-content disclosure.

## 0.78.0: durable bounded subscription waits (schema 0.57.0 to 0.58.0)

Apply `AiSchemaModule` `0.58.0` with run, coordinator, cancellation, retention,
restore and provider workers stopped. The additive migration creates private
`graphql_orm_ai_subscription_waiters` and
`graphql_orm_ai_subscription_wait_adoptions` tables and their generated
indexes. No existing row is backfilled and existing subscriptions remain
best-effort unless their canonical semantic descriptor explicitly advertises
`ReplayThenLive` and the deployment registers the matching authenticated
source.

Wait variables, projection, completion condition, replay cursor and adopted
result are protected. Portable backups deliberately redact those columns, so
restored live waiters must converge to `RecoveryRequired`; do not manufacture
or skip a cursor. Same-database process restart can reclaim an exact valid
waiter through its short worker fence. Registration and every event/adoption
boundary rehydrate the stored credential-free `PrincipalReference`, check the
exact target policy and current rules, and preserve ordinary subscription
resolver authorization.

Construct `AiSubscriptionCheckpointAdopter` around the wait service and the
existing coordinator adopter, and run the bounded waiter worker separately
from ordinary run workers. Source registration and catalogue discovery grant
no authority. Best-effort sources are ineligible, and model-authored GraphQL,
arbitrary predicates, raw cursors and indefinite monitors remain unsupported.

## 0.78.0: parked provider sessions across durable waits (schema 0.58.0)

Apply the coordinated AI schema module `0.58.0` before enabling approval or
subscription suspension of a provider-retained turn. The migration adds
private nullable parked-wait identity, source/parked checkpoint fingerprints,
continuation fingerprint, confirmation/expiry/reclaim fields and a bounded
cleanup-scan index to `graphql_orm_ai_provider_session_bindings`. Existing
bindings remain valid with no rewrite or backfill; all new nullable fields are
empty and `park_generation` starts at zero, so no historical binding becomes
parked or reclaimable.

Ordinary hosts do not construct parking or reclaim authority. The owning
approval/subscription coordinator obtains a crate-issued opaque park request
only after `provider_turn_persisted` is durable, then uses the shared lifecycle:

```rust,ignore
let parked = provider_sessions
    .park_for_wait(&lease, opaque_park_request)
    .await?;

// The owning wait transaction persists its exact wait row and parked
// checkpoint, transitions the run to WaitingApproval/WaitingSubscription,
// records the nonterminal source-attempt outcome, and clears the ordinary run
// lease atomically.
provider_sessions.confirm_parked_wait(&parked).await?;

// After a fresh run claim adopts and consumes the exact one-shot wait result:
let claim = provider_sessions.reclaim_after_wait(&fresh_lease).await?;
```

`confirm_parked_wait` is a two-phase crash-convergence check, not authority.
It succeeds only for the exact run/wait/checkpoint graph and may be repeated
only for that same opaque proof. Cleanup scanning can idempotently confirm the
same graph after a crash between the wait transaction and explicit
confirmation. An expired unconfirmed park, terminal/cancelled run, reset,
abandoned adoption or expired confirmed wait instead enters the existing
provider-deletion lifecycle; provider absence is never inferred from expiry.

For a retained approval, `claim_next_approved` does not reuse the closed source
attempt. It first requires the exact parked provider binding to be confirmed,
then atomically creates a fresh attempt/generation and refences the pending tool
call and step. Approval that wins before confirmation remains unclaimed; the
maintenance pass can confirm the exact durable graph and a later bounded claim
can proceed. Stateless/non-retained approvals retain the historical in-attempt
handoff contract.

Only a fully completed provider-retained tool-request turn is suspendable.
Stateless continuation, an in-flight stream and a provider-native synchronous
dynamic-tool responder are rejected because they do not provide a stable
resumable checkpoint. After a successful reclaim, any failure before provider
continuation must call `require_cleanup` on the returned claim; it must not put
the cursor back into `Active` or retry with stale adoption evidence.

This changes no GraphQL SDL or public browser payload and stores no provider
cursor, prompt, output, tool argument/result or authorization secret in the
new columns. Portable backup continues to redact the protected cursor, and
the existing provider-session restore audit keeps every restored binding
closed. The package version and schema constant are intentionally updated
together as `graphql-orm-ai` 0.78.0 / schema 0.58.0.

## 0.78.0: generated GraphQL query and mutation capabilities (schema remains 0.58.0)

Subgraphs that want automatic bounded reads compile
`AiGraphqlQueryCapabilityCatalog` from their exact finished SDL and canonical
`GraphqlSemanticCatalog`, register the complete set in `AiToolCatalog`, and
retain the capability fingerprint supplied with each provider definition.
Call `AiRuntime::execute_query_capability` with that exact fingerprint and the
closed provider plan. Do not accept a GraphQL document, target or descriptor
from the provider.

The addition does not enable any root. Install a fresh descriptor-driven host
policy and preserve current-principal plus ordinary resolver authorization.
Existing explicit profiles and manifest wire version 2 remain supported.

For generated reads, install
`AiGeneratedGraphqlAuthorizationPolicy::generated_only` or wrap the existing
static policy with `AiGeneratedGraphqlAuthorizationPolicy::new`. Bind the
exact logical target, finished SDL and semantic catalogue once; do not create
one static tool-policy row per generated query.

Generated mutations are absent unless their semantic operation is explicitly
classified. `ApprovalRequired` continues through the existing durable preview,
one-shot approval and supervised resume services. `Automatic` additionally
requires target-level opt-in and an `AutonomousWrite` rule ceiling and now
uses `automatic_mutation_batch_persisted` as its protected continuation
checkpoint kind. Hosts must not construct, reopen or consume that proof.

These generated query/mutation changes require no additional AI schema,
GraphQL SDL, table, column, constraint, backup, data migration or backfill.
AI schema module `0.58.0` remains current after applying the separate durable
subscription-wait migration above.

The existing descriptor `maximum_result_records` field now enforces the total
selected GraphQL result rather than the greatest individual list. Review
explicit static profiles with sibling or nested object/list projections and
set the limit to their checked complete maximum. Corrected limits participate
in the existing descriptor fingerprint. No wire field or persistence contract
was added.

## 0.77.1: independent provider feature builds (schema remains 0.57.0)

Update the AI package to 0.77.1 at the same reviewed full Git revision as its
workspace companions. No API, provider wire, GraphQL SDL, database, table,
column, index, constraint, protected-row, backup, restore, or schema-module
migration is required.

Hosts may continue enabling one provider feature. The shared Responses adapter
now compiles OpenAI background overrides only in the OpenAI lane. xAI, Ollama,
and approved OpenAI-compatible profiles continue using the provider-neutral
fail-closed default and report background execution as unsupported. Feature
selection grants no capability or egress authority.

Run the exact local feature lane before deployment:

```sh
scripts/check-ai-provider-lanes.sh test provider-xai
scripts/check-ai-provider-lanes.sh clippy provider-xai
```

## 0.77.0: absence-proven provider-session rebind (schema 0.56.0 to 0.57.0)

Apply AI schema module `0.57.0` before starting workers from this release. The
migration records a persistent semantic version only: it produces no table,
column, index, constraint, or row rewrite. Existing active/claimed/cleanup
rows retain their prior meaning, and provider-session cleanups completed by an
older release remain ordinary `New` sessions because no binding row exists.
No historical cursor, absence proof, or tombstone is manufactured.

Provider-session cleanup now clears the protected cursor and leaves an exact
private `Deleted` tombstone after the registered deletion service proves the
provider thread absent. A later run asks
`AiProviderSessionService::disposition_for_run` for one of four closed results:

```rust
match provider_sessions
    .disposition_for_run(&lease, &turn_plan)
    .await?
{
    AiProviderSessionRunDisposition::New => { /* allow the executor to bind empty state */ }
    AiProviderSessionRunDisposition::Resume(binding) => {
        assert_eq!(binding.descriptor(), turn_plan.descriptor());
    }
    AiProviderSessionRunDisposition::RebindAllowed(_) => {
        /* allow the executor to create, atomically rebind, or discard */
    }
    AiProviderSessionRunDisposition::Unavailable(_) => {
        return Err(AiError::Conflict);
    }
}
```

Ordinary hosts should use `AiProviderCallExecutor::execute_with_provider_session`,
which owns this branch and discards a newly created empty provider session if
the bind/rebind CAS loses. Hosts that inspect readiness before constructing a
turn should consume the closed disposition rather than infer eligibility from
`AiProviderSessionState::Deleted` or treat it as row absence.

`RebindAllowed` is issued only after exact persisted provider absence and is
bound to the current principal reference, owner/session/scope, run ID, attempt
ID, lease generation, deleted binding row/generations, immutable provider
descriptor, and host-supplied canonical transcript fingerprint. The service
rehydrates and rechecks authority again during `rebind_for_run`. Cleanup
backoff, expiry alone, uncertain transport, restore quarantine, descriptor
drift, and stale/replayed authorizations remain unavailable.

Hosts may optionally install `AiProviderFailureDiagnosticSink` on the provider
executor. It receives only `AiProviderFailureCategory`; the category is
operational evidence, not retry authority, and `RecoveryRequired` remains
mandatory whenever provider execution may have occurred.

There is no GraphQL SDL, entity, table, column, index, constraint, backup,
restore-format, or data migration. The schema-module bump records the new
durable meaning of a successfully cleaned provider-session row. No backfill is
required.

## 0.76.1: agql-auth 0.15 session-bound delegation alignment (schema remains 0.56.0)

Update every monorepo dependency to one reviewed full revision and align any
direct auth dependency to:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "e841ffd382082ad7419be259fe957f949b956ff7", version = "0.15.0" }
```

This release makes the upstream `VerifiedActiveUserSessionResolver`, opaque
`VerifiedActiveUserSession`, `SessionBoundDelegationBinding`, and
`prepare_session_bound_access_token_only` /
`issue_session_bound_access_token_only` contracts available in the same type
universe as AI current-principal rehydration. `graphql-orm-ai` does not issue
tokens itself and introduces no application-specific delegation API.

Hosts adopting session-bound delegation must follow agql-auth's 0.15 migration:
install a read-only authoritative active-session resolver, narrow current
roles/scopes, and bind the actor, resource, correlation ID, and exact reviewed
operation. Delegated credentials remain non-refreshable and normal resolver
session assurance remains authoritative.

There is no GraphQL SDL, entity, table, column, index, constraint, backup,
restore, data, or AI schema-module migration. No backfill is required; AI
schema module `0.56.0` remains current.

## 0.76.0: authoritative durable run terminal events (schema 0.55.0 to 0.56.0)

Apply AI schema module `0.56.0` before starting workers from this release. The
migration records a persistent semantic version only: it produces no table,
column, index, constraint, or row rewrite.

Every successful authoritative transition to `Completed`, `Failed`,
`Cancelled`, or `RecoveryRequired` now appends exactly one owner-visible
session event and owner-inbox event in the same fenced transaction as the run
state and immutable attempt outcome. The event names are available through
the closed `AiRunTerminalEvent` API:

```rust
assert_eq!(
    AiRunTerminalEvent::RecoveryRequired.event_type(),
    "run_recovery_required",
);
```

No run-service constructor or coordinator integration changes are required.
Continue composing the existing `OrmAiSessionService`, `OrmAiInboxService`,
and subscription services. Clients should page replay to its captured
watermark before subscribing live and close local Working/Stop state when a
canonical terminal event arrives. On `ResetRequired`, discard provisional
per-run rendering and reload authoritative session/message windows before
reconnecting at the returned watermark.

Canonical terminal payloads contain only a format marker and the matching
closed run state. They deliberately use a content-free database-managed
metadata envelope because the same state already appears in the non-secret
event type and private run row. All nonterminal and content-bearing payloads
continue through the configured scope content-protection policy unchanged.

Historical runs are not backfilled. Manufacturing events for an already
terminal run would require choosing a stream position after the original
transaction and could falsely imply atomic observation. Existing durable rows
remain valid; applications may clear stale transient UI state during a reset
or one-time deployment reconciliation. No data migration, backfill, or row
rewrite is required.

## 0.75.1: complete durable event replay at the ORM page limit (schema remains 0.55.0)

No host API changes are required. `AiSessionEventPage.HasMore` and
`AiInboxEventPage.HasMore` now compare the final contiguous event sequence
with the page's captured watermark instead of requiring one row beyond the
configured ORM page limit. Hosts should remove any workaround that requests
one fewer event than their configured database maximum; the maximum value is
now a supported replay page size.

The watermark, cursor, reset-required, owner authorization, scope policy,
payload protection, retention, and replay-then-live contracts are unchanged.
There is no GraphQL SDL, database entity, table, column, index, constraint,
backup/restore, or persistent storage semantic change. No data migration,
backfill, or row rewrite is required, and AI schema module `0.55.0` remains
current.

## 0.75.0: canonical Codex tools and retained bootstrap (schema remains 0.55.0)

Construct provider definitions from the registered manifest instead of
copying descriptor fields:

```rust
let definition = tool_catalog.read_only_model_definition(
    &registered_tool_id,
    "inventory_count",
)?;
```

The alias is provider-local correlation metadata. The library copies and later
revalidates the exact stable ID, description, argument schema, and descriptor
fingerprint. Hosts must not strip `$schema`, scalar bounds, or projection
metadata. The Codex adapter now performs its own closed, fingerprint-bound
projection of canonical argument JSON Schema into the subset accepted by the
app-server.

Move retained-thread developer instructions out of
`ModelRequest::instructions` and into the immutable registration:

```rust
let bootstrap = AiCodexAppServerBootstrapInstructions::from_static(&[
    "Use a registered application tool whenever current facts are needed to answer the request.",
])?;
let registration = AiCodexAppServerRegistration::new(
    provider_profile_id,
    logical_model,
    executable_sha256,
    executable_version,
    sandbox_profile,
    AI_CODEX_APP_SERVER_PROTOCOL_V2,
)?
.with_launch_profile(launch_profile)
.with_bootstrap_instructions(bootstrap);
```

Only compile-time static deployment policy belongs in this value. Never place
user input, tenant or route context, secrets, resolver output, or model-authored
text in it. Retained requests now reject non-empty
`ModelRequest::instructions`; ordinary business text remains in bounded input
blocks. Update `AiCodexAppServerRunProcess::create_empty_thread` implementations
to accept the added `&AiCodexAppServerBootstrapInstructions` argument and pass
it unchanged to `AiCodexAppServerProtocolActor::start_persistent_empty_thread`.

Registration identity version 3 includes the bootstrap fingerprint. Drain and
delete older provider-session bindings through their exact cleanup lifecycle;
do not resume them under a replacement registration. There is no GraphQL SDL,
database entity, table, column, index, constraint, backup/restore, or persistent
storage semantic change. No data migration, backfill, or row rewrite is
required, and AI schema module `0.55.0` remains current.

The provider-neutral session value types are also available when compiling the
MSSQL feature profile so provider adapters can retain one canonical public
contract across backend lanes. This does not add an MSSQL provider-session
persistence implementation or change its experimental compile/schema-only
status.

## 0.74.0: closed Codex dynamic-tools-only launch profile (schema remains 0.55.0)

Replace the former boolean dynamic-tool registration switch with the closed
profile and make the trusted process factory attest that it applies that exact
profile:

```rust
let launch_profile = AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
    AiCodexAppServerModelToolMode::Direct,
)?;
let registration = AiCodexAppServerRegistration::new(
    provider_profile_id,
    logical_model,
    executable_sha256,
    executable_version,
    sandbox_profile,
    AI_CODEX_APP_SERVER_PROTOCOL_V2,
)?
.with_launch_profile(launch_profile);
```

`AiCodexAppServerRunProcessFactory::supports_launch_profile` defaults to true
only for the strict text-only profile. A factory enabling dynamic tools must
return true only after it launches the reviewed executable with
`registration.launch_profile().codex_arguments()` unchanged, clears inherited
environment and credentials, supplies an isolated configuration home with no
project configuration or MCP servers, uses an empty working directory, and
applies its fixed external sandbox. If this proof is absent,
`ProviderCapabilities::custom_tools` is false and dynamic calls return
`Unsupported` before process launch.

The model tool mode comes from the reviewed model catalogue bound to the exact
executable digest. Codex 0.147.0 models declared `code_mode_only` cannot use
this profile: with Code Mode disabled their direct dynamic definitions are not
model-visible. Keep their text-only provider registration or choose a reviewed
`Direct` model for the separate dynamic-tool profile. Do not relabel the
catalogue mode or enable Code Mode, shell, unified execution, filesystem, MCP,
browser, hosted web, remote control, or another native surface as a workaround.

Registration identity version 2 includes the launch profile. Existing
provider-session bindings created with the earlier dynamic registration must
be invalidated and deleted through the ordinary exact cleanup lifecycle before
replacement; they must not be resumed under the new identity.

The protocol actor now accepts unsigned server-request ID `0` for an otherwise
exact dynamic call because Codex 0.147.0 emits that valid JSON-RPC identifier.
Hosts need no special case and must continue passing complete frames unchanged
to `accept`.

This release changes only public provider API and runtime compatibility. There
is no GraphQL SDL, database entity, table, column, index, constraint,
backup/restore, or persistent storage semantic change. No data migration,
backfill, or row rewrite is required, and AI schema module `0.55.0` remains
current.

## 0.73.4: closed Codex notification profile and retained resume compatibility (schema remains 0.55.0)

Existing Codex process implementations continue calling
`AiCodexAppServerProtocolActor::initialize` or
`initialize_with_dynamic_tools`; no host-authored capability object is added.
Both methods now include the library-owned exact notification opt-out profile.
Do not add, remove, or rewrite its methods in the host, and continue passing
every received frame unchanged to `accept`. The stable path does not opt into
the experimental API; the dynamic-tool path still adds only
`experimentalApi: true`.

Hosts should treat the additive non-exhaustive inbound variants as follows:

- `ReasoningLifecycle` is content-free progress metadata. Do not invent or
  display reasoning text. The actor accepts only paired empty reasoning items
  because every turn explicitly requests `summary: "none"`.
- `RetainedResumeUsageSnapshot` is cumulative provider state replayed during
  an exact retained-thread resume and before the new active turn. Do not emit
  it as usage or charge it to the current run. It may satisfy retained-resume
  readiness after the correlated response because Codex 0.147.0 does not emit
  `thread/started` on that exact resume path. It never replaces the response
  or completes initial thread creation.

Deletion adapters should finish only after the exact correlated empty
`thread/delete` response. Stop waiting for or locally admitting a
`thread/status/changed` `notLoaded` notification. The fixed initialization
profile suppresses unused thread status, thread settings, cleared goal, MCP
startup, and account rate-limit notifications. If the server sends any of
those despite negotiation, pass the frame to the actor and fail closed.

This release changes only the provider protocol/API contract. There is no
GraphQL SDL, database entity, table, column, index, constraint,
backup/restore, or durable semantic change. No data migration, backfill, or
row rewrite is required, and AI schema module `0.55.0` remains current.

## 0.73.3: content-free Codex runtime warnings (schema remains 0.55.0)

Codex app-server process adapters should handle
`AiCodexAppServerInbound::RuntimeWarning` as a non-fatal, content-free control
event and continue waiting for authoritative turn, item, usage, and completion
events. Continue passing every complete provider frame unchanged to
`AiCodexAppServerProtocolActor::accept`; do not inspect, log, forward, or
substring-match warning messages in the host.

The actor admits a warning only after a typed `turn/start` has opened the exact
thread-bound turn and before its terminal `turn/completed`. It validates the
positive signed timestamp, exact envelope and parameter keys, optional thread
correlation, a non-empty control-free message of at most 4 KiB, at most eight
warnings, and at most 16 KiB of warning text per turn. All content is discarded
before the public inbound value is returned. Warning budgets reset only when a
new typed turn begins and after terminal completion.

This is an additive provider protocol-compatibility fix. There is no GraphQL
SDL, database entity, table, column, index, constraint, backup/restore, or
durable semantic change. No data migration or row rewrite is required, and AI
schema module `0.55.0` remains current.

## 0.73.2: newly bound provider-session activation (schema remains 0.55.0)

`AiProviderCallExecutor::execute_with_provider_session` now preserves whether
the opened cursor was created empty and durably bound by the current run or
claimed from a previously committed turn. This evidence is crate-owned and is
not a host input, GraphQL value, model value, or public reset mechanism.

Codex app-server process implementations should add the new typed
`AiCodexAppServerRunProcess::start_bound_turn` and
`start_bound_dynamic_turn` methods. These methods receive the first turn only
after cursor protection, durable binding, current-principal reauthorization,
and exact reopening have succeeded. Start `turn/start` directly on the loaded
thread and do not issue `thread/resume`. Keep existing
`start_retained_turn` and `start_retained_dynamic_turn` implementations for a
cursor claimed by a later run; those paths must still perform the full
`thread/resume` response/notification lifecycle before `turn/start`.

The new trait methods have fail-closed default implementations, so unrelated
providers remain source-compatible. A Codex host must implement them to use
new persistent sessions. Do not infer activation from request order, local
flags, cursor shape, or actor state, and do not recreate the actor or process
between empty creation and the first bound turn.

This is a provider/runtime lifecycle correction only. There is no GraphQL SDL,
database entity, table, column, index, constraint, backup/restore, or durable
semantic change. No data migration or row rewrite is required, and AI schema
module `0.55.0` remains current.

## 0.73.1: repeatable retained Codex lifecycles (schema remains 0.55.0)

`AiCodexAppServerProtocolActor` now owns a separate bounded observation phase
for every typed thread creation or resume operation. Hosts may use the same
actor for `thread/start` followed by one or more exact `thread/resume` cycles.
For each cycle, continue passing complete frames unchanged and wait for exactly
one correlated response plus one matching `thread/started` notification before
starting a turn. Either ordering remains supported.

No reset method is added. Starting the next lifecycle fails while the previous
pair, a turn, or deletion remains incomplete. Retained model and dynamic-tool
definitions are immutable across cycles and terminal turns. Existing process
adapters need no source changes; remove any host-side actor replacement or
protocol-frame workaround introduced for this bug.

This is a runtime protocol-state fix only. There is no GraphQL SDL, database,
entity, table, column, index, constraint, backup/restore, or persistent semantic
change. No data migration or row rewrite is required, and AI schema module
`0.55.0` remains current.

## Unreleased: strict Codex lifecycle envelopes (crate 0.72.0 to 0.73.0; schema remains 0.55.0)

The `provider-codex-app-server` protocol actor now requires the complete Codex
CLI 0.147.0 notification envelope for every admitted lifecycle event. The
signed `emittedAtMs` value must be present, positive, and representable as an
`i64`; it is validated and discarded rather than exposed through
`AiCodexAppServerInbound`. Missing, negative, zero, overflowing, duplicate, or
extra-field envelopes fail closed.

Thread and turn starts now track the correlated response and authoritative
notification independently, so either wire ordering is accepted while IDs,
duplicates, and late frames remain rejected. Ordinary agent-message item
start/completion pairs are also lifecycle-fenced. A generated
`thread/status/changed` notification is accepted only when it reports exactly
`notLoaded` for the same thread already under a delete request; other status
values and unsolicited status traffic remain unsupported.

Host process implementations must pass each complete app-server frame to the
actor unchanged. Continue waiting until both the `thread/start` or
`thread/resume` response and `thread/started` notification have been admitted.
Ignore `emittedAtMs` for application behavior and treat deletion-bound
`thread/status/changed` as lifecycle evidence only. No generic notification,
remote-control, command, filesystem, MCP, browser, hosted-web, or tool
authority is added.

No database or GraphQL schema migration, table/index/constraint change,
backfill, or row rewrite is required. AI schema module 0.55.0 remains current.

## Unreleased: strict Codex 0.147.0 initialization (crate 0.71.0 to 0.72.0; schema remains 0.55.0)

The `provider-codex-app-server` adapter admits Codex CLI 0.147.0's
`remoteControl/status/changed` initialization notification only when its full
bounded payload reports exactly `disabled`, contains valid server and
installation identifiers, has a null environment, and carries a valid
emission timestamp. The public non-exhaustive
`AiCodexAppServerInbound::RemoteControlDisabled` variant carries no identifiers
or payload. Process implementations should continue waiting for the correlated
initialization or thread response when they observe it. They must not turn it
into provider output or model-visible activity.

`AiCodexAppServerProvider::capabilities()` now advertises
`provider_retained_continuation: true`, matching its implemented
`AiProviderSessionTurnPlan` path. Every encoded `thread/start` and
`thread/resume` now supplies `approvalPolicy: "never"` and
`sandbox: "read-only"`. Hosts should align immutable provider-profile
capability declarations with the corrected adapter value; dynamic tools remain
separately default-off and coordinator-owned.

No database or GraphQL schema migration, table/index/constraint change,
backfill, or row rewrite is required. AI schema module 0.55.0 remains current.

## Unreleased: canonical GraphQL tool manifests (crate 0.70.0 to 0.71.0; schema remains 0.55.0)

The re-exported `graphql-orm-ai-tool-profiles` package moves from 0.2.0 to
0.3.0 and `AI_GRAPHQL_TOOL_MANIFEST_VERSION` moves from 1 to 2. Update every
owning-subgraph producer and AI consumer to the same reviewed monorepo
revision. Version 2 recursively canonicalizes JSON object keys before hashing
manifests and nested tool descriptors, so canonical router-extension transport
does not invalidate an unchanged contract.

Existing version 1 payloads remain unsupported. Exact descriptor/tool-policy
fingerprints may change when their JSON Schema object members were authored in
a noncanonical order; review and update immutable allowlists rather than
copying old fingerprints. Array order, schemas, projections, disclosures,
logical targets, documents, versions, and all validation remain authoritative.

No database or GraphQL schema migration, table/index/constraint change,
backfill, or row rewrite is required. AI schema module 0.55.0 remains current.

## Unreleased: retained Codex threads and experimental dynamic tools (crate 0.69.0 to 0.70.0; schema 0.54.0 to 0.55.0)

Apply AI schema module 0.55.0 before enabling retained provider turns. This is
a persistent-semantic migration only: it adds no table, column, index,
constraint, backfill, or row rewrite. Existing 0.54.0 provider-session rows
remain structurally compatible. The new module version records that a provider
watermark becomes reusable only after protected assistant output, its exact
checkpoint, and canonical terminal run completion are durable.

The Codex app-server adapter now implements protected empty-thread creation,
exact resume, run interruption, and exact deletion/absence. A host:

1. implements `AiCodexAppServerRunProcess` with the strict
   `AiCodexAppServerProtocolActor`, including `create_empty_thread`, retained
   turn methods, `interrupt`, and `delete_thread`;
2. registers `AiCodexAppServerProvider` and its
   `provider_session_deletion_service` under the immutable profile;
3. constructs `OrmAiProviderSessionService` and attaches it with
   `AiReadOnlyAgentCoordinator::with_provider_session_service`;
4. computes the authoritative transcript-prefix fingerprint and an
   `AiProviderSessionDescriptor` whose policy fingerprint changes with any
   retention, rule, or offered-tool policy change;
5. adds `AiProviderSessionTurnPlan` through
   `AiReadOnlyAgentTurnPlan::with_provider_session`; and
6. runs the existing cleanup worker so invalidated/expired cursors receive
   exact provider deletion and absence proof.

Experimental native dynamic tools are closed unless the immutable registration
calls `with_experimental_dynamic_tools` and the planner uses
`AiReadOnlyAgentTurnPlan::new_experimental_dynamic_tools`. The process installs
the exact reviewed dynamic definitions while creating an otherwise empty
persistent thread because app-server cannot add them through `thread/resume`.
No instruction or user content is sent before durable binding. Each exact
`item/tool/call` is answered only by the coordinator's ordinary registered
read-only GraphQL tool service; the process receives no principal credential,
delegated token, router transport, or generic callback.

Public implementors of `AiProvider::create_empty_session` must accept the new
`&ModelRequest` parameter and use it only for immutable model/tool binding.
Public implementors of `AiCodexAppServerRunProcess::create_empty_thread` must
accept the exact reviewed dynamic definitions and either install all of them
or fail closed. Existing providers/process actors that do not support retained
sessions may keep the default `Unsupported` implementation. Dynamic tools are
experimental and require a live compatibility gate against the deployed Codex
app-server version; shell, files, MCP, hosted web, browser, screenshots, raw
reasoning, and generic JSON-RPC remain unavailable.

After terminal assistant persistence the coordinator completes the run before
calling `AiProviderSessionService::commit_turn`. A commit failure now calls
`require_cleanup` but preserves the successful answer and `Completed` run;
provider retention is an optimization, not user-visible completion authority.

## Unreleased: provider sessions, hosted activity, and run-scoped app-server (crate 0.68.0 to 0.69.0; schema 0.53.0 to 0.54.0)

Apply AI schema module 0.54.0 before constructing an
`OrmAiProviderSessionService`. The additive migration creates the private
`graphql_orm_ai_provider_session_bindings` table, its unique session binding,
and bounded run-claim and cleanup indexes. There is no backfill and no existing
session, message, run, event, protected block, budget, rule, or usage row is
rewritten. Provider-session retention is disabled unless a host explicitly
constructs and uses the new service.

The durable provider-session contract is intentionally stricter than an
in-memory process cache. A host creates an empty provider thread, retains a
provider-side deletion guard, and calls `bind_for_run` before sending business
content. Resume uses `claim_for_run` with the exact immutable
`AiProviderSessionDescriptor` and canonical transcript fingerprint, then
`open_for_run` under current principal/scope/run fencing. Advance the binding
only through `commit_turn` after the assistant message and matching
`assistant_output_persisted` checkpoint are durable. Cancellation, ambiguous
transport, cursor rejection, policy/registration drift, or failed output
commit calls `require_cleanup`. A managed cleanup worker uses
`claim_cleanup`, `open_for_cleanup`, a registered
`AiProviderSessionDeletionService`, and `complete_cleanup`; retryable provider
failure uses `schedule_cleanup_retry`. Session final deletion now waits until
the binding row has been removed after exact provider-absence proof.

Portable backup/restore does not carry provider cursor material:
`protected_cursor` is backup-redacted and any provider-session binding makes
the new required `ProviderSessionBindings` restore audit fatal. Drain retained
provider sessions and prove provider absence before taking a portable backup
that must restore ready. Redaction or expiry is never treated as provider
absence, and restored provider threads are never resumed automatically.

Hosts enabling ordered progress should replace
`AiProviderCallExecutor::with_live_delta_sink` with
`with_provider_activity_sink` using the same `OrmAiLiveDeltaService`. The new
protected `provider_activity` session/inbox events include typed visible text,
provider-generated visible summary, hosted-tool start/completion, and validated
citation metadata in provider order. They contain no hosted-tool result body,
application-tool argument/result, raw provider frame, hidden reasoning, or
credential. Existing `provider_live_delta` rows remain readable and retain
their prior retention behavior.

Public Rust request/profile code requires source updates:

- add `reasoning_summary: ModelReasoningSummaryRequest::Disabled` to direct
  `ModelRequest` literals unless a selected provider advertises
  `visible_reasoning_summaries` and current rules permit
  `VisibleReasoningSummaries`;
- add `visible_reasoning_summaries` to exhaustive `ProviderCapabilities`
  literals, normally through `..ProviderCapabilities::default()`;
- replace `ModelBuiltinTool::WebSearch { allowed_domains }` with one explicit
  `ModelWebSearchDomainPolicy::PublicWeb`, `allowed_domains(...)`, or
  `blocked_domains(...)` value;
- update exhaustive `ProviderEvent::Citation` matches to use the validated
  `ProviderCitation` value; and
- add `maximum_web_search_calls` to exhaustive rule budget/input literals.
  Missing serialized budget/usage/summary fields decode to the closed default,
  but Rust struct literals remain intentionally exhaustive.

For native OpenAI Responses, a host may request a bounded automatic visible
summary and may offer hosted web search beside exact reviewed application tools
in `ProviderRetained` mode. Keep the existing `StatelessReplay` mixed-tool
prohibition; do not remove it as a migration workaround. Web search remains
absent by default, needs its own egress/rule capability, immutable per-call and
per-run ceilings, pricing/budget reservation, and provider-normalized
start/completion evidence. Assistant Markdown links are not citations.

The optional `provider-codex-app-server` feature is a separate first-phase
local adapter. Construct an immutable `AiCodexAppServerRegistration`, bounded
`AiCodexAppServerRunLimits`, trusted
`AiCodexAppServerRunProcessFactory`, `AiCodexAppServerRunPool`, and
`AiCodexAppServerProvider`. The factory is responsible for direct verified
execution, cleared environment, empty working directory, OS/container sandbox,
and an effective process-tree kill callback. This release accepts only fresh
text-only turns and does not replace the existing JSONL local harness for
stateless application-tool loops. It exposes no dynamic tools, retained Codex
thread, shell, files, web, images, MCP, collaboration, or generic JSON-RPC.

## Unreleased: protected tool lifecycle previews (crate 0.67.0 to 0.68.0; schema remains 0.53.0)

Tool-profile producers should update to `graphql-orm-ai-tool-profiles` 0.2.0.
Profiles remain non-browser-disclosable by default. To permit a lazy UI
preview, attach a validated `AiBrowserResultPreviewPolicy` to the exact
generated or custom profile with `with_browser_result_preview`. The policy is
part of the canonical descriptor and manifest fingerprint, so all owning
subgraphs and consumers must update to one reviewed monorepo revision and
republish/re-register their manifests together.

Hosts that expose previews construct `OrmAiToolCallResultPreviewService` from
the AI database, closed runtime, and a mandatory
`AiToolResultPreviewAuthorizer` that reapplies current application row/field
policy and returns a bounded subset. Compose it as
`Arc<dyn AiToolCallResultPreviewService>` beside `AiQueryRoot`; clients may
then call `AiToolCallResultPreview(Input:)` by exact session and tool-call ID.
The service never executes a resolver and returns no protected content unless
the current owner, session/scope policy, current tool policy, descriptor,
disclosure, retention, classification, protection, host projection, and all
limits still permit it.

Session and owner-inbox consumers may now observe
`application_tool_started` before the existing terminal tool event. Start is
authoritative only after the call is fenced for execution; approval staging
and pre-execution denial emit no start. Both lifecycle events contain metadata
only. Existing event replay/watermark handling needs no cursor migration.

This is an additive Rust, GraphQL, descriptor, and event-contract change.
There are no entity, column, index, constraint, backup/restore, or stored-row
changes, so AI schema module 0.53.0 remains current and no database/data
migration is required.

## Unreleased: owner-authorized run cancellation (crate 0.66.0 to 0.67.0; schema 0.52.0 to 0.53.0)

Apply AI schema module 0.53.0 before exposing `CancelAiRun` or starting a
cancellation-aware coordinator. The additive migration adds nullable
`cancellation_request_id` and `cancellation_requested_at` run columns plus the
private `graphql_orm_ai_run_cancellation_requests` idempotency table. Existing
runs remain unchanged and no protected content is rewritten.

Construct one shared `AiRunCancellationHub`; install it on
`OrmAiRunService::with_cancellation_hub` and pass it to
`OrmAiRunCancellationService`. Compose the service as
`Arc<dyn AiRunCancellationService>` beside `AiMutationRoot`. The GraphQL
`CancelAiRun(Input:)` mutation accepts only an exact session ID, run ID, and
client-generated request UUID. The owner receives an authoritative terminal
view; replay and inbox consumers observe `run_cancellation_requested` followed
by `run_cancelled`.

The database marker, not the in-process notification, is authoritative.
Provider futures are dropped when cancellation wins; local harness providers
must retain their established terminate-on-drop contract. Custom run-control
implementations remain source compatible through default cancellation methods,
but they do not become cancellation-aware until they implement the durable
observation boundary. This is an additive Rust and GraphQL API change with an
additive schema migration and no application-data migration.

## Unreleased: backend-neutral tool-profile producers (crate 0.65.0 to 0.66.0; schema remains 0.52.0)

Owning subgraphs that only publish reviewed AI GraphQL tool manifests should
depend on `graphql-orm-ai-tool-profiles` 0.1.0. The package accepts the same
profile, builder, manifest, descriptor, disclosure, execution-target, and
generated-operation policy types previously exposed by `graphql-orm-ai`, but
has no database, persistence, backup, storage, provider, or coordinator
dependency. `graphql-orm-ai` re-exports those canonical types, so wire payloads
and fingerprints are byte-identical and require no transformation.

Generated resolver catalog types are now owned by
`graphql-orm-operation-catalog` 0.1.0 and remain source-compatible re-exports
from `graphql-orm`. Mixed-backend workspaces should update `graphql-orm` and
`graphql-orm-macros` together to 0.21.0. Reusable companion crates whose own
`sqlite`, `postgres`, and `mssql` features are mutually exclusive may use
`#[backend_selected_graphql_entity(...)]` to keep derive selection local to
the consuming package despite Cargo feature unification.

`graphql-orm-ai` no longer pulls `graphql-orm-backup` transitively. Hosts using
backup/restore orchestration must declare `graphql-orm-backup` directly with
the matching backend and reviewed revision. AI schema module 0.52.0, GraphQL
SDL, stored rows, manifest wire version, and persistent semantics are
unchanged; no schema or data migration is required.

## Unreleased: durable session titles (crate 0.64.0 to 0.65.0; schema 0.51.0 to 0.52.0)

Apply AI schema module 0.52.0 before starting a 0.65.0 session or title worker.
The migration adds `title_revision` and `title_source` to the private session
record, plus private title-mutation and title-work tables and their bounded
lookup indexes. Existing rows receive revision zero and the closed `user`
source. This deliberately prevents the automatic worker from replacing a
pre-upgrade title whose original default-versus-user intent cannot be proven.
No existing session, message, event, or protected content is rewritten.

Hosts may expose the new `RenameAiSession(Input:)` mutation through the
composable AI mutation root. Clients should generate one
`ClientMutationId`, optionally send the last observed `TitleRevision`, and
replace their shell with the authoritative returned value. Retrying the same
normalized title and mutation ID is effect-idempotent. Reusing the ID for a
different session or title, or supplying a stale expected revision, fails with
a conflict.

To generate first-message titles, construct `OrmAiSessionTitleWorkService`
with the same database, access/content-protection policies, durable current-
principal resolver, trusted clock, and validated
`AiSessionTitleWorkLimits`. A managed host worker claims work, calls
`open_first_message`, invokes its fixed reviewed provider without tools, and
calls `complete`. Provider selection and output generation stay host-owned;
the library stores only scheduling/fencing facts and the accepted bounded
title. Use `schedule_retry` or `fail` with redacted stable error codes. A
manual, custom, or pre-upgrade title is never automatically replaced.

This is an additive Rust and GraphQL API change with a required additive
database migration and persistent-semantic change. The two new records remain
private and do not create generated GraphQL CRUD roots. Backup/restore includes
their ordinary ORM rows; first-message text and title event payloads retain
their existing content-protection contexts. Update all monorepo dependencies
to one reviewed revision before enabling the worker.

## Unreleased: generated GraphQL tool profiles (crate 0.63.0 to 0.64.0; schema remains 0.51.0)

Hosts may replace hand-maintained GraphQL tool documents with
`AiGraphqlToolManifestBuilder`. Construct it inside the owning subgraph from
the complete finished SDL, stable public subgraph identity, and registered
logical execution target. Add `AiGraphqlToolProfile::read_only` profiles for
queries through `add_generated_profile` or `add_custom_profile`.

Every profile must explicitly provide bounded model inputs, a closed typed
argument plan, selected output fields, list bounds, an exact disclosure
schema, and byte/record ceilings. Fixed values and semantic variable aliases
are compiled into the generated document. Unused/unknown inputs, missing
required arguments, invalid nested input fields, conflicting aliases,
unbounded lists, projection/disclosure mismatch, schema drift, and stale ORM
catalog bindings fail during construction or registration. One root may have
multiple profile IDs. Existing manually authored descriptors continue to
work.

Handwritten mutations are not accepted through the read-only constructor.
Use `AiGraphqlToolProfile::supervised_mutation`, an explicit write risk, and
the existing one-shot approval path. The compiler never discovers or enables
shells, remote control, screenshots, arbitrary GraphQL, arbitrary URLs, or
unregistered operations.

For federated transport, encode `manifest.extension_payload()` inside the
optional generic router descriptor extension named by
`AI_GRAPHQL_TOOL_MANIFEST_EXTENSION_NAME`, version 1. Consumers decode through
`AiGraphqlToolManifest::from_extension_payload`, aggregate against exact
active SDL values, and then register through
`AiGraphqlToolManifestSet::register_into`. The owning subgraph supplies its ORM
operation catalogue and application-operation policy to
`add_generated_profile`; the federated AI consumer does not import the owning
service crate. Unknown/incomplete versions and roots advertised by multiple
subgraphs fail closed.

This is an additive Rust/wire API change. AI schema module 0.51.0 and all
database entities, columns, indexes, constraints, backup/restore contracts,
and stored rows are unchanged; no data migration is required.

## Unreleased: tool-free coordinator turns (crate 0.62.1 to 0.63.0; schema remains 0.51.0)

Hosts can now wrap an ordinary initial `AiProviderCallPlan::new` result with
`AiReadOnlyAgentTurnPlan::new_chat(provider_call, rules, uses_byok)`. The new
factory accepts only an exact-scope provider call with no application tools,
provider-built-in tools, continuation, or tool-result input. It deliberately
has no `AiToolResultEgressRoute`, tool checkpoint, tool execution, or
continuation path. Existing tool-bearing planners continue to call
`AiReadOnlyAgentTurnPlan::new` unchanged.

The coordinator still freshly resolves the planned rule fingerprint before
and after provider transport, projects provider kind/capability,
classification, retention, BYOK, and estimated rule budgets, and relies on the
ordinary provider executor for current-principal, egress-manifest, provider,
and atomic-budget enforcement. Authoritative usage is accepted before the
ordinary protected final output and `Completed` transition. A provider tool
event cannot normalize without an exact offered definition and fingerprint;
even a custom executor returning a manufactured tool result reaches no tool
service, checkpoint, or continuation.

This additive pre-1.0 Rust API advances the crate to 0.63.0. It changes no
GraphQL SDL, entity, column, index, constraint, backup/restore contract, or
persistent semantic, so AI schema module 0.51.0 remains current and no data
migration is required.

## Unreleased: provider message preview compatibility (crate 0.62.0 to 0.62.1; schema remains 0.51.0)

Provider-output persistence now writes protected assistant-message previews as
the canonical top-level JSON string already written for user messages and read
by `AiMessages`. Both the synchronous and OpenAI background output paths use
that representation. Reads also accept the exact bounded `{"text":"..."}`
object form emitted for assistant messages by crate 0.62.0. Other object shapes,
additional fields, non-string values, and previews exceeding the configured
session preview byte limit fail closed with `AI_PERSISTENCE_FAILED`.

Hosts need only update every `graphql-orm` monorepo dependency to the same
reviewed 0.62.1 revision. Do not rewrite or decrypt existing preview rows: the
compatibility reader recovers valid 0.62.0 values under their original content
protection context, owner, and scope. This patch changes no Rust API, GraphQL
SDL, entity, column, index, constraint, backup/restore contract, or intended
persistent semantic. The top-level string was already the canonical contract;
the legacy object was an unreadable 0.62.0 writer defect. AI schema module
0.51.0 therefore remains current and no data migration is required.

## Unreleased: deployment-only current rules (crate 0.61.0 to 0.62.0; schema remains 0.51.0)

Deployments whose AI constraints are immutable process configuration can now
construct `DeploymentAiCurrentRuleResolver` from their durable
`CurrentPrincipalResolver`, trusted `Clock`, validated
`AiCurrentRuleResolverLimits`, and validated `AiRuleDeploymentLimits`. The
deployment ceiling becomes the exact effective constraint set for every
requested target scope; the library records an empty applied-layer lineage and
computes the canonical rule fingerprint internally.

Use this resolver instead of provisioning artificial `AiScopePolicyRecord`
rows when no operator-editable rule hierarchy exists. Deployments that manage
hierarchical rows continue to use `OrmAiRulePolicyService` with
`OrmAiCurrentRuleResolver` and require no source changes. Both paths share the
same exact-reference, freshness, expiry, trusted-clock, scope-validation, and
canonical-fingerprint implementation.

The new resolver returns narrowing evidence only. Hosts must still provide all
ordinary provider routing, tool registration and authorization, GraphQL
resolver authorization, egress, atomic budget, approval, credential, and
resource-access proofs. This additive pre-1.0 Rust API advances the crate to
0.62.0. It changes no GraphQL SDL, entity, column, index, constraint,
backup/restore contract, or persistent semantic, so AI schema module 0.51.0
remains current and no data migration is required.

## Unreleased: attachment restore metadata audit (crate 0.60.0 to 0.61.0; schema remains 0.51.0)

The aggregate `AiRestoreAuditKind::Attachments` category has been replaced by
two exact required categories: `AttachmentMetadataGraph` and
`AttachmentObjectBytes`. This is a pre-1.0 public Rust and serialized audit-name
break. Update downstream matches and issue-reference handling to use the two
new variants. `AiRestoreSnapshotFacts::invalid_attachment_count` is replaced by
`invalid_attachment_metadata_count` and `invalid_attachment_object_count`;
deserialization accepts the old field as a metadata-count alias for legacy
dry-run simulations, but new Rust construction must name both fields.

Hosts can now construct `AiRestoreAttachmentMetadataAuditLimits` from their
host-attested `AiAttachmentServiceLimits` and independent attachment/artifact
row bounds, then pass it to
`OrmAiRestoreFactCollector::with_attachment_metadata_audit`. The collector
reads attachment and artifact rows through generated ORM queries in the same
quiescent transaction as the other database facts. It performs bounded exact
session/message parent lookups, checks owner and message-session linkage,
validates the current attachment/artifact lifecycle and cleanup tuples,
rechecks MIME, size, checksum, safe reference and protection-envelope shape,
and rejects duplicate ownership of one local object or provider reference.
Backup-redacted quarantine/upload/provider references cannot be inferred; a
transient row that needs them remains invalid until a later repair contract
handles it explicitly. Reaching either row bound yields `LimitExceeded` with
no partial attachment evidence.

`AttachmentMetadataGraph::Complete` proves only the accepted database graph.
The collector hashes the complete rows, parent evidence, host-attested limits,
and expected local-object facts into its opaque digest, but deliberately does
no BlobStore/provider/application I/O. `AttachmentObjectBytes` remains fatal
`NotImplemented`. A later restore auditor must bind a verified backup snapshot
and manifest, stream every object from the restored target BlobStore, and
recheck its exact key, byte count, and SHA-256; optional object metadata or a
successful restore sink call is insufficient.

This breaking pre-1.0 API advances the crate to 0.61.0. It changes no entity,
column, index, constraint, backup policy, or GraphQL SDL, so AI schema module
0.51.0 remains current and no data migration is required. The encryption-key,
attachment-object, usage/counter, rule, skill, checkpoint, provider, UI-intent,
retention, and stream audits plus the repair/validation/recovery-epoch/startup
proof remain closed.

## Unreleased: policy restore auditors (crate 0.59.0 to 0.60.0; schema remains 0.51.0)

Restored deployments may now construct `AiRestorePolicyAuditLimits` from
host-attested `AiBudgetPolicyManagementLimits` and
`AiPricingCatalogManagementLimits`, plus explicit
budget-policy, pricing-policy, and audit-event row bounds. Pass that value to
`OrmAiRestoreFactCollector::with_policy_audits` before `collect`. Omitting it
leaves `BudgetPolicies` and `PricingPolicies` as fatal `NotImplemented` audits;
the collector never invents deployment ceilings.

The budget-policy audit rederives exact scope identity, validates optional
principal pairing and syntax, checks interval and CAS shape, requires at least
one nonnegative ceiling, enforces every supplied deployment maximum, and
rechecks the per-scope policy cardinality. The pricing audit validates the
deterministic `pricing:<uuid>` identity, scope and route, provider/model shape,
all token/fixed/built-in rate ceilings, cached-input ordering, creator fields,
and per-route version cardinality. Every immutable pricing version must have
exactly one matching `ai.pricing_policy.create` audit with the same creator and
the canonical allowed outcome. Orphan, malformed, missing, or duplicate
pricing creation audits are fatal.

Because audit-event fields are intentionally not exposed as generated ORM
filters, the pricing proof performs a bounded scan of audit-event rows and
then selects the relevant immutable creation facts in memory. Size
`maximum_audit_events` for the complete restored audit history. Reaching any
policy or audit bound returns `LimitExceeded` and contributes no partial policy
evidence. The host-attested deployment limits and complete accepted row-set
digests are bound into `AiCollectedRestoreFacts` and its dry-run plan digest.

Supplying these values does not prove that they match the live budget/pricing
services. Before policy audit completion can contribute to runtime readiness,
the future applied validator must bind the exact live configuration epoch to
the recovery epoch. This checkpoint remains dry-run only.

This additive pre-1.0 API advances the crate to 0.60.0. It changes no entity,
column, index, constraint, backup policy, or GraphQL SDL, so AI schema module
0.51.0 remains current and no data migration is required. Encryption-key,
attachment/object, usage/counter, rule, skill, checkpoint, provider,
UI-intent, retention, and stream audits remain fatal until implemented.
Applied repair, validation, recovery epochs, and runtime reopening remain
closed.

## Unreleased: bounded restore fact collection (crate 0.58.0 to 0.59.0; schema remains 0.51.0)

Hosts beginning an empty-target restore may now construct
`OrmAiRestoreFactCollector` from the restored ORM database and pass the
verified backup manifest's AI module fingerprint to `collect`. Configure hard
bounds through `AiRestoreCollectorLimits`. Reaching a bound returns an opaque
fact set whose affected audit is `LimitExceeded`; it is not a successful
partial collection.

Use `AiRestoreReconciler::plan_collected` for new restore work. It accepts only
crate-created `AiCollectedRestoreFacts`, emits a fatal issue for every
`NotImplemented`, `LimitExceeded`, or `Invalid` audit, and returns exact fact
and plan digests for the future recovery epoch. The initial collector completes
only conservative run classification plus approval and non-revoked egress-
consent revalidation counts. Encryption-key, attachment, usage, budget,
pricing, skill, rule, checkpoint, provider-background/webhook, UI-intent,
retention, and stream audits deliberately remain `NotImplemented` and
therefore fatal.

Existing serialized `AiRestoreSnapshotFacts` and `AiRestoreReconciler::plan`
remain available for compatibility and pure simulations, but caller-populated
zero counts are not database audit evidence and must not be used as production
readiness authority. `AiCollectedRestoreFacts` does not expose the raw fact
structure publicly, and `AiCollectedRestorePlan` cannot be consumed into an
unbound plan. Use its read-only plan and digest accessors for inspection.

`AiRestorePlan::readiness_report_after_apply` has been removed. There is no
applied-restore replacement in this checkpoint because a pure plan cannot
prove mutations or post-apply validation. Existing normal-start integrations
may still supply the host-attested `AiRuntimeReadinessReport` accepted by
`AiRuntimeStartGate::open`, but that compatibility seam is not restore
authority and must remain closed after database/object import.

This pre-1.0 breaking minor advances the crate to 0.59.0 because it removes the
misleading readiness helper. It changes no
entity, column, index, constraint, backup policy, or GraphQL SDL, so AI schema
module 0.51.0 remains current and no data migration is required. Applied
restore and runtime reopening remain closed until the remaining audits,
bounded repair applier, post-apply validator, exact recovery epoch, and
non-forgeable readiness path are implemented.

## Unreleased: repository consolidation (crate 0.57.0 to 0.58.0; schema remains 0.51.0)

The source repository is now
`https://github.com/Dastari/graphql-orm.git`. Git consumers should use that URL
for AI, backup, storage, and ORM packages and pin every selected package to the
same reviewed full monorepo revision.

The consolidated workspace resolves `graphql-orm` 0.16.0,
`graphql-orm-backup` 0.7.0, and `graphql-orm-storage` 0.6.0 through workspace
paths. `agql-auth` 0.12.0 remains an exact external dependency. Remove old
internal Git URLs and local patches, regenerate `Cargo.lock`, verify one source
for every internal package, and rerun the complete backend/provider matrix.

The crate advances to 0.58.0 because the backup/storage dependency identity is
a pre-1.0 compatibility boundary. The alignment does not otherwise change AI
Rust APIs, GraphQL SDL, persistent entities, backup policy, or restore
readiness. AI schema module 0.51.0 remains current and no data migration is
required.

## Unreleased: backup 0.6 compatibility checkpoint (crate 0.56.0 to 0.57.0; schema 0.50.0 to 0.51.0)

Update `graphql-orm-backup` to the exact reviewed 0.6.0 merge at
`6a9ccedd76fd140c351c8861de72c4cb7c99feea`. It resolves the already reviewed
`graphql-orm` 0.16.0 revision
`dd68a001f47f04178bf3389dd47ee952faa6ecf0` and
`graphql-orm-storage` 0.5.0 revision
`f1a1f06483d5fd3a0b8fd17f013b3ad4dd9849c5`. Remove local path, patch, or
branch overrides and regenerate `Cargo.lock`; there must be one ORM and storage
source/type universe.

Apply AI schema module 0.51.0 before creating the next backup. There is no DDL
or row rewrite: the version bump records a persistent backup-policy semantic.
Finalized local attachment and derived-artifact `blob_reference` columns now
use `Include`, because the opaque local key is required to reconnect a restored
row to its separately restored object bytes. Treat the backup repository as
confidential and access-controlled. Quarantine keys, upload-token hashes,
provider references, credentials, and secrets remain redacted.

Existing snapshots exported under schema 0.50.0 contain a redaction sentinel
instead of the local object key and cannot prove a complete local-object
restore. Do not use them to open runtime readiness; create and verify a new
full snapshot after applying 0.51.0. Incremental ORM backup remains unavailable
until a reliable upstream change journal exists.

This checkpoint does not expose a production restore API and does not open the
runtime after database/object import. The remaining downstream work is still
to collect facts from restored rows, apply generated-ORM repairs, revalidate
all invariants, record the exact recovery epoch, and open readiness only for a
zero-fatal applied epoch. The repository consolidation section above supersedes
this checkpoint's standalone-repository source pins.

## Unreleased: generated resolver-operation bindings (crate 0.55.0 to 0.56.0; schema remains 0.50.0)

Update the exact `graphql-orm` and `graphql-orm-macros` dependency to 0.16.0 at
`dd68a001f47f04178bf3389dd47ee952faa6ecf0`. Keep `agql-auth` 0.12.0 at
`3f3b0c5365adfbe436514a681d977b600991b797`. Remove path/patch/branch
overrides and regenerate `Cargo.lock` so the runtime and derive macro resolve
one reviewed source/type universe.

Hosts registering derive-generated application operations may now build a
`GraphqlOperationContract` with `with_generated_operation` and register the
descriptor through `AiToolCatalog::register_generated_with_disclosure`.
Supply an explicit `AiGeneratedGraphqlOperationPolicy` that admits only
reviewed application entities/modules; the provided
`DenyAllAiGeneratedGraphqlOperationPolicy` is the fail-closed default.
Registration rejects hidden or ambiguous operations, catalog/operation
fingerprint drift, kind mismatches, subscriptions, and documents that do not
contain exactly one named operation selecting exactly one unaliased generated
root. Ordinary tool enablement, principal-aware host policy, resolver
authorization, finished-schema validation, projection, and disclosure remain
separate required checks.

`GraphqlOperationContract` adds the public optional `generated_operation`
field. This is a pre-1.0 public Rust struct-literal change; prefer
`GraphqlOperationContract::new` rather than struct literals. The serialized
field defaults to absent and is omitted when absent, so existing custom-root
serialized contracts continue to decode and encode as before. A generated
binding is included automatically in descriptor and approval fingerprints,
and malformed deserialized bindings fail approval/catalog validation.
Custom roots continue to use `register_with_disclosure`; that method now
rejects generated-bound contracts so catalog revalidation cannot be skipped.

This change adds no entity, column, index, constraint, public GraphQL SDL,
backup descriptor, or persistent semantic. AI schema module `0.50.0` remains
unchanged and no data migration is needed. Revalidate the complete composed
host SDL and server-authored documents during deployment. Applied restore
remains closed. The later 0.57.0 section records the reviewed backup 0.6.0
alignment and the remaining downstream collector/applier work.

## Unreleased: close raw provider file-search authority (crate 0.54.0 to 0.55.0; schema remains 0.50.0)

`ModelRequest::validate` now rejects
`ModelBuiltinTool::FileSearch { store_ids, maximum_results }`. The public enum
variant remains source-compatible as a reserved shape, but valid raw
provider vector-store IDs no longer pass provider-neutral request validation.
This is a deliberate pre-1.0 behavioral breaking change: a caller-supplied ID
cannot prove the provider object's exact creation, owner/scope/session,
attachment hash, logical profile, retention, byte-time cost, or
dependency-ordered deletion.

Hosts must remove any construction of this variant; there is no replacement
search API in this checkpoint. Continue using released attachment references
with the separately authorized ephemeral inline provider-input path where its
MIME/size policy permits. The exact profile-bound provider-file deletion seam
also remains available for already durable cleanup artifacts.

The complete future upload/index/logical-use/deletion contract is documented in
`docs/provider-files.md`. Do not work around the closed boundary by injecting a
provider ID through a custom tool, application GraphQL field, egress manifest,
or locally authored artifact row. Provider upload/search may reopen only after
all creation ambiguity, storage-time pricing, quotas, retention, cleanup, and
restore gates pass.

This change adds no entity, column, index, constraint, GraphQL SDL, backup
descriptor, or persistent semantic. AI schema module `0.50.0` is therefore
unchanged and no data migration is needed.

At that checkpoint the accompanying Slice 3-7 audit documents did not open
another public Rust, GraphQL, provider, backend, or persistence capability.
Durable tool-policy management, generated resolver-operation metadata, applied
backup/restore, provider-persistent upload/search, and MSSQL production writes
were unavailable. The later 0.56.0 section above records the generated
resolver-metadata integration; the other gates remain closed. Ignored
`.handoffs/` prompts are coordination state, not dependencies or packaged
migration artifacts.

## Unreleased: complete OpenAI background terminal reconciliation (crate 0.53.0 to 0.54.0; schema 0.49.0 to 0.50.0)

Apply AI schema module `0.50.0` while provider workers, webhook intake,
backup/restore, and runtime start are closed. The generated migration adds a
non-unique composite index over
`graphql_orm_ai_provider_webhook_receipts(provider_kind,
provider_profile_id, provider_response_id, state)`. It does not add, remove, or
rewrite columns. Existing receipt and submission rows require no data rewrite;
the index is the only structural data-store change.

The module identity also activates the previously reserved reconciliation,
terminal-message, receipt linkage/state, budget, usage, checkpoint, session
event, and inbox event facts as one terminal lifecycle contract. A restored
adapter must validate each terminal or recovery-required background submission
against its exact run, original attempt/fence, immutable outcome, uncertain or
committed reservation, usage row, optional protected output/checkpoint/events,
and processed/duplicate/recovery receipt states. Report any invalid graph
through `invalid_provider_background_submission_count` or
`invalid_provider_webhook_receipt_count`; a nonzero count keeps runtime
readiness closed. No restore path performs provider I/O.

Hosts can use the new lifecycle-aware APIs:

- `retrieve_classified` returns `Observed`, a private retryable failure proof,
  or a private recovery-required failure proof after the exact retrieval egress
  marker has been durably bound. Existing `retrieve` remains available and maps
  either classified transport failure to `AiError::ProviderFailed`.
- `handle_retrieval_failure` consumes that exact proof. Timeout, rate limiting,
  and temporary unavailability release under bounded exponential backoff;
  credential/configuration/validation/rejection failures close for recovery.
- `release_nonterminal` releases exact `queued` or `in_progress` observations.
  `close_expired` closes a bounded batch past its immutable response deadline.
- `OrmAiOpenAiBackgroundTerminalService::commit` consumes a terminal
  observation and atomically commits authoritative usage, budget/counters,
  optional protected completed output, checkpoint/session/inbox events,
  receipt states, immutable outcome, audit, submission, and run. Failed,
  incomplete, and cancelled responses never create assistant output.

The terminal service must use the deployment's immutable
`AiProviderUsageAccounting`, current `AiRuntime`, content-protection policy,
trusted clock, output bounds, principal-freshness bound, and transaction retry
bound. Do not clear an uncertain reservation, synthesize a receipt, or complete
a parked run manually. Exact replay returns `AlreadyReconciled` only after
validating the already-durable graph; conflicting evidence becomes
`RecoveryRequired`.

This is an additive public Rust API and private persistence/behavioral contract
change. It changes no public GraphQL SDL and requires no application-domain
data migration. Regenerate `Cargo.lock`, rehearse the generated prior-to-current
migration in test-owned stores, and rerun the backend-specific release matrix.

## Unreleased: exact OpenAI background retrieval boundary (crate 0.52.0 to 0.53.0; schema 0.48.0 to 0.49.0)

Apply AI schema module `0.49.0` while provider workers, webhook intake,
backup/restore, and runtime start are closed. This increment does not add or
remove a column: it activates the previously reserved nullable
`retrieval_egress_decision_id` as the durable marker that a specific
reconciliation generation was authorized to cross the provider retrieval
boundary. That persistent lifecycle meaning changes the module identity and
fingerprint even though the generated database target has no new structural
DDL.

Existing supported `0.48.0` rows require no data rewrite and have a null
retrieval-egress marker. A non-null marker created outside the `0.53.0`
generated-ORM service is not trusted migration input; keep that submission and
its run closed for reviewed recovery rather than clearing or synthesizing an
egress event manually. No application-domain data migration is required.

Hosts may construct `OrmAiOpenAiBackgroundRetrievalService` with:

- the same generated-ORM database used by the authoritative
  `OrmAiEgressDecisionAudit`;
- the ready `AiRuntime`;
- a credential-free `AiOpenAiBackgroundRetrievalRoute` fixing the original
  logical profile/destination and current policy, residency, and optional
  consent references; and
- bounded `AiOpenAiBackgroundRetrievalLimits` for the full response, visible
  content, item counts, request timeout, and principal freshness.

`retrieve` reloads the complete claim graph, rehydrates the current principal,
checks current scope/session write access and content-protection readiness,
audits a new exact `provider_response` egress decision, and atomically binds
that allow ID before transport. The registered native OpenAI provider then
performs only `GET /v1/responses/<bound resp_ ID>` at its compiled official
endpoint, with redirects disabled, just-in-time credential resolution, and a
timeout strictly shorter than the claim. Provider/profile/response swaps,
unsupported output items, malformed usage, body/visible/item overflow, and
security-relevant metadata drift fail closed.

The returned `AiOpenAiBackgroundRetrievalObservation` is bounded in-memory
provider truth only. This increment does not select or mutate webhook receipts,
release a nonterminal observation with policy backoff, settle uncertain
budget, protect/persist assistant output, append an attempt outcome, close
deadline/retry exhaustion, or terminally mutate the submission/run. If no
later terminal service consumes a retrieval result, the claim expires; a
higher-generation reclaim validates the prior egress event and clears its
stale marker before another attempt. Do not treat retrieval success as run
completion.

This is an additive public Rust API and private persistence/behavioral
contract change. It changes no public GraphQL SDL. Exact receipt selection,
nonterminal retry classification, terminal budget/output transactions,
deadline/exhaustion closure, and terminal restore validation remain closed.

## Unreleased: OpenAI background reconciliation claim schema (crate 0.51.0 to 0.52.0; schema 0.47.0 to 0.48.0)

Apply AI schema module `0.48.0` while provider workers, webhook intake,
backup/restore, and runtime start are closed. The generated migration keeps 40
private entities and adds nullable reconciliation owner, lease-expiry,
next-attempt, deadline, reconciliation-time, retrieval-egress-decision, and
terminal-message columns to
`graphql_orm_ai_provider_background_submissions`. It also adds generation and
retry counters with zero defaults and indexes the lease,
next-attempt, and deadline fields used by bounded workers. The module
fingerprint and backup descriptor change.

The migration itself does not rewrite existing submission rows. A row created
under schema `0.47.0` therefore has no reconciliation deadline or next-attempt
time and remains ineligible for future automated reconciliation. Keep such a
row parked and resolve it through the deployment's reviewed recovery process;
do not infer a historical provider-retention promise or backfill it through
application-authored SQL. No stored content or application data migration is
required. An empty background-submission table needs no operational data
handling.

Newly accepted submissions initialize generation and retry count to zero,
schedule the first attempt at local acceptance time, and capture an immutable
response-availability deadline. The deadline uses the earlier of the provider
creation timestamp or local acceptance time plus the window selected by the
acknowledged `store` value, so timestamp skew and later configuration changes
cannot extend it.

Hosts may supply public
`AiOpenAiBackgroundReconciliationWindows` through
`OrmAiOpenAiBackgroundSubmissionService::with_reconciliation_windows`.
Both values must be at least one second. Temporary `store: false` responses
default to five minutes and cannot exceed ten minutes; stored responses
default to 29 days and cannot exceed 30 days. Narrow these values when the
reviewed logical provider profile promises a shorter availability period.

Hosts may now construct
`OrmAiOpenAiBackgroundReconciliationService` with
`AiOpenAiBackgroundReconciliationLimits`. The service can claim or reclaim
only a complete accepted submission graph, heartbeat an exact owner/
generation/row-version fence, and voluntarily release that fence only before
provider retrieval. Lease lifetimes are limited to five minutes, retry delays
to one hour, candidate scans to 256 rows, nonterminal releases to 100, and
serialization retries to 16. Defaults are one minute, five minutes, 64 rows,
16 releases, and eight transaction retries. The submission ID is now an
available stable tiebreaker in the private generated ordering contract.

The returned `AiOpenAiBackgroundReconciliationClaim` is opaque. It does not
authorize credential resolution, provider retrieval, current egress, output
normalization, budget settlement, receipt mutation, or run mutation. A
deployment should not treat successful claiming as progress toward a terminal
result. Exact-response retrieval, current-authority revalidation, receipt
matching, terminal normalization, atomic terminal persistence, and deadline/
retry exhaustion closure were deliberately unavailable in that increment.

This is an additive public Rust API and private persistence/behavioral contract
change. It changes no public GraphQL SDL and requires no additional data
migration beyond schema `0.48.0`. At that checkpoint, exact-response retrieval,
receipt matching, budget settlement, protected output persistence, terminal run
mutation, and terminal restore validation remained closed.

## Unreleased: upstream dependency alignment to graphql-orm 0.15.0 and agql-auth 0.12.0

Update the exact Git dependency universe to:

- `graphql-orm` 0.15.0 at
  `6beef53633befd90a4d4810887a3e4640dc4ad91`; and
- `agql-auth` 0.12.0 at the peeled `v0.12.0` target
  `3f3b0c5365adfbe436514a681d977b600991b797`.

Remove host patches, path overrides, or direct dependencies that resolve an
older source identity. Hosts enabling the ORM's optional `auth-agql` bridge
must use the same exact auth version and revision so one public type universe
resolves.

The ORM update includes the reviewed PostgreSQL constraint-index
introspection fix from 0.13.0. Constraint-owned PRIMARY KEY and UNIQUE backing
indexes are no longer planned as ordinary `DROP INDEX` operations, and
composite UNIQUE constraints are rendered and introspected in key order.
Operators must replan the complete generated target and run an owned
prior-to-current migration rehearsal before rollout. Do not mark an older
module version as newly applied to conceal a real historical schema mismatch.

The 0.15.0 ORM also corrects bounded generated updates, deletes, and retention
purges whose `MutationLimit + 1` sentinel exceeds the public 100-row read cap.
Public GraphQL and repository read limits are unchanged. Residual or in-memory
bounded-mutation predicates now fail before selection or writes; callers must
use fully database-renderable predicates.

Direct `agql-auth` consumers must also follow its 0.10.0-to-0.12.0 migration.
Version 0.11.0 replaces split durable rate-limit load/save behavior with
revision-bound compare-and-swap; custom durable stores need an atomic revision
column or equivalent and must backfill existing rows. Version 0.12.0 adds the
typed list-valued OIDC `EssentialAcrs` request and `matched_acrs` outcome,
advances stored OIDC policies containing that requirement to representation
version 2, and requires updates to exhaustive matches and public struct
literals. This crate does not implement an auth rate-limit store or infer local
MFA from provider ACR/ACRS evidence.

This dependency alignment advances the unreleased crate from 0.50.0 to 0.51.0
and does not change the AI schema module from 0.47.0. It changes no AI entity,
GraphQL SDL, persisted AI data, backup descriptor, or application
authorization policy, so no AI data migration is required. Regenerate
`Cargo.lock`, verify one source/type universe, and rerun the full SQLite,
PostgreSQL, MSSQL, Rustdoc, Clippy, naming, SemVer, and release-policy matrix.

## Unreleased: exact OpenAI background submission binding (crate 0.49.0 to 0.50.0; schema 0.46.0 to 0.47.0)

Apply AI schema module `0.47.0` while provider workers, webhook intake,
backup/restore, and runtime start are closed. The generated migration adds the
fortieth private entity,
`graphql_orm_ai_provider_background_submissions`, with unique deterministic
submission, attempt, budget-reservation, and optional provider-response
bindings plus the exact requested output ceiling and acknowledged provider
storage choice. Existing 0.46 rows and tables need no rewrite, so no data
migration is required. The module fingerprint and backup descriptor change.

With `provider-openai` plus SQLite/PostgreSQL, hosts may construct
`OrmAiOpenAiBackgroundSubmissionService` from the existing fenced run service,
runtime, atomic budget service, egress audit, and trusted clock. Its `submit`
method accepts an active `Running` lease and an `AiProviderCallPlan` only when:

- the provider is exactly native `OpenAi`;
- the request is an initial provider-retained turn with no continuation,
  application tools, built-in tools, or attachments and with a nonzero
  provider-enforced maximum-output-token ceiling;
- exactly one model-inference manifest binds the same scope, session, run,
  profile, model, and `provider_response` retention; and
- current principal, scope/session write access, budget, egress, runtime
  readiness, and provider background capability all pass.

The service freshly rehydrates current authority, prepares a content-free
deterministic binding, and renews the exact fence in one transaction. It then
marks the reservation `uncertain`, periodically heartbeats the same fence
while awaiting the acknowledgement, and performs one provider create call. It
never retries that external boundary. Failures known to precede preparation or
transport release unused reserved capacity. The native OpenAI adapter forces
`background: true` and `stream: false`, retains the configured `store`
setting, embeds only the opaque submission UUID and collision-check key in
response metadata, bounds the JSON acknowledgement to 1 MiB, and validates its
response ID, status, timestamp, background flag, object kind, model, output
ceiling, configured storage choice, and exact echoed metadata.

A valid acknowledgement binds the response atomically and changes the run to
the new lease-free `AiRunState::WaitingProvider`. Any transport or malformed/
unpersisted acknowledgement ambiguity conservatively changes both the binding
and run to `RecoveryRequired`, retains the uncertain budget, and records only
a safe error code plus the exact immutable attempt outcome. Callers must update
exhaustive `AiRunState` matches for `WaitingProvider`; ordinary workers cannot
transition out of that state.

This slice does not retrieve provider output, match webhook receipts to a
submission, settle usage, persist assistant output, or complete/requeue a run.
Do not invoke background submission in a deployment that lacks an independently
reviewed operational process for parked or recovery-required work. The next
reconciler must freshly prove current authority, budget, egress, retention,
exact submission/receipt/response binding, and bounded provider output before
any run mutation.

Trusted third-party adapters implementing the additive default-deny
`AiProvider::submit_background` method can construct the content-free
`ProviderBackgroundSubmission` only after validating and supplying the exact
response ID/status/timestamp/model/output ceiling/storage choice;
`ProviderBackgroundBinding` remains runtime-authored and redacted.
`AiRuntime::submit_provider_background` checks runtime readiness, exact
provider registration, request proofs, and the declared background capability.
None of these values grants retrieval, reconciliation, or run-mutation
authority.

Restore fact collectors must populate the serde-defaulted but Rust-source-
breaking
`AiRestoreSnapshotFacts::invalid_provider_background_submission_count` after
checking deterministic identity, exact run/attempt/fence/profile/request/
budget/egress/response facts, output ceiling, acknowledged storage choice,
lifecycle state, and preparation/acceptance audit links. Any nonzero count is
fatal. Restored `WaitingProvider` runs always plan
`RecoveryRequired`; they are never replayed. Legacy serialized fact payloads
decode the new count as zero, but the old module fingerprint still fails until
current facts are collected and validated.

This is an additive public Rust API, new public enum variant, private schema,
backup/restore, retention-authorization, provider, budget, and run-lifecycle
contract change. It adds no public GraphQL SDL, credential persistence,
provider-file lifecycle, generic CRUD root, or application data migration.

## Unreleased: verified OpenAI webhook receipt intake (crate 0.48.0 to 0.49.0; schema 0.45.0 to 0.46.0)

Apply AI schema module `0.46.0` while provider routes/workers, backup/restore,
and runtime start are closed. The generated migration keeps 39 private entities
and extends the existing webhook-receipt placeholder with `receipt_key`,
`provider_profile_id`, `provider_event_kind`, and `provider_created_at`. Its
private primary-key metadata becomes the deterministic receipt UUID plus the
existing provider-family column so generated `insert_if_absent` works
atomically on SQLite and PostgreSQL. The full SHA-256 receipt key remains a
separate exact collision check. No generic GraphQL CRUD root is added.

No supported 0.48 deployment can contain a webhook receipt because the crate
previously exposed no writer. Therefore no data migration is needed for a
supported deployment, and the private receipt table must be empty before this
migration. A nonempty table is unsupported state: stop, keep the runtime
closed, and investigate through reviewed backup/restore or migration tooling.
Do not infer profile/event bindings or use application-authored SQL to make the
migration pass.

With `provider-openai`, construct `OpenAiWebhookHeaders` from the exact
`webhook-id`, `webhook-timestamp`, and `webhook-signature` values and pass those
with the unparsed request bytes to an exact-profile `OpenAiWebhookVerifier`.
The verifier now adds `hmac` to that feature's dependency graph. Its
`SecretRef` must resolve the profile's OpenAI webhook signing secret, not grant
provider request authority. `OpenAiWebhookVerifierLimits` can narrow the body
and replay-window bounds. Verification failures remain redacted and produce no
receipt.

On SQLite/PostgreSQL, pass only `OpenAiVerifiedWebhookEvent` to
`OrmAiProviderWebhookReceiptService::record`. Return route success only after
`Recorded` or `AlreadyRecorded`; later exact deliveries preserve the first
receipt. A same-profile/provider-event collision with changed immutable facts
returns `AiError::Conflict`. Validly signed unsupported events are recorded as
ignored. Hosts must not persist or log the raw body, signature, signing secret,
or `Debug`-redacted identifiers around this boundary.

Restore fact collectors must populate
`AiRestoreSnapshotFacts::invalid_provider_webhook_receipt_count` after checking
each receipt's deterministic identity, exact provider/profile/event/response
binding, verified-signature fact, lifecycle state, and redacted creation-audit
link. Any nonzero count is fatal to readiness. Legacy serialized fact payloads
default the new count to zero for decoding compatibility, but their old module
fingerprint still fails against schema `0.46.0` until a trusted adapter has
collected and validated current facts.

This is an additive public Rust API, feature dependency, private schema,
backup/restore metadata, and operational route contract change. It adds no
public GraphQL SDL, entity, credential persistence, background response
submission/retrieval, run binding/mutation, usage settlement, or reconciliation
worker. Supported receipts intentionally remain `pending_reconciliation` until
a future worker can re-prove the original run, attempt, fence, provider,
profile, response, budget, egress, retention, and current-authority bindings.

## Unreleased: native OpenAI exact-reference deletion (crate 0.47.0 to 0.48.0; schema 0.44.0 to 0.45.0)

Apply AI schema module `0.45.0` while attachment, retention, backup/restore,
and runtime workers are closed. The generated migration keeps 39 private
entities and adds nullable `provider_kind` and `provider_profile_id` columns to
private attachment artifacts. It adds no table, index, or constraint. Rows
without a provider reference need no data rewrite.

Every artifact carrying `provider_reference` must now carry its exact supported
provider family and logical provider profile. Legacy provider-reference rows
without either binding deliberately fail validation and cleanup. Reconcile
those rows only while the runtime is closed through a reviewed trusted
migration or restore process that can prove the original owner; do not infer a
provider/profile, issue application-authored SQL, or clear the reference to
make validation pass. Successful fenced cleanup clears the reference and both
ownership bindings atomically before retention may delete artifact metadata.

`AiProviderFileDeletionRequest` adds `provider_kind()` and
`provider_profile_id()` getters. Host implementations of
`AiProviderFileDeletionService` should route and authorize from those exact
values and continue to treat the opaque reference as sensitive.

With `provider-openai`, hosts may construct `OpenAiFileDeletionService::new`
with the exact logical profile ID, `OpenAiProviderConfig`, and `AiSecretStore`,
then install it through
`OrmAiAttachmentService::with_provider_file_deletion_service`. The adapter is
fixed to OpenAI's official Files endpoint, cannot list/upload/search/read file
content, validates the exact deletion acknowledgement, and confirms absence by
retrieving the same file and requiring not found. It rejects another provider
family, logical profile, artifact kind, or malformed OpenAI file ID before
transport. A configured profile must continue to resolve the same OpenAI
project/organization ownership domain used when the artifact was created.

This is an additive public Rust API plus private schema and retention-contract
change. It adds no provider upload/search lifecycle, GraphQL SDL, egress or
budget authority, credential persistence, entity, or application data rewrite
for ordinary rows.

## Unreleased: authoritative provider built-in unit pricing (crate 0.46.0 to 0.47.0; schema 0.43.0 to 0.44.0)

Apply AI schema module `0.44.0` while provider workers, pricing administration,
budget reconciliation, backup/restore, and runtime start are closed. The
generated migration keeps 39 private entities and adds two nonnegative,
defaulted columns to each append-only private pricing version:
`web_search_microunits_per_call` and `file_search_microunits_per_call`.
Existing versions receive zero for both dimensions; no application-authored
SQL, row copy, or content rewrite is required. Create a new immutable pricing
version before enabling either built-in rather than changing an old row.

`CreateAiPricingPolicyInput` and `AiPricingPolicyView` add both rate fields.
They are deployment-supplied exact integer rates: the crate does not embed,
discover, or refresh provider prices. Nonzero administration remains denied
until the host explicitly configures
`AiPricingCatalogManagementLimits::with_maximum_builtin_tool_microunits_per_call`.
Restore collectors must validate both new rates, the exact scope/provider/model
binding, unique version reference, and creation-audit linkage before reporting
zero `invalid_pricing_policy_count`.

Preflight callers must populate `AiPricingQuoteRequest::builtin_tools` with the
distinct enabled `AiPricedBuiltinToolKind` values and copy the exact shared
`ModelRequest::maximum_builtin_tool_calls` value into
`maximum_builtin_tool_calls`. With no supported built-ins, pass an empty vector
and zero. A supported quote reserves the shared maximum as `tool_units` and
prices every possible call at the greatest enabled per-call rate, so a mixed
web/file-search request remains conservative.

Every `ModelRequest` literal must initialize `maximum_builtin_tool_calls`. Use `None`
when no provider built-in is exposed. A built-in request requires `Some(1..=64)`;
the native OpenAI adapter sends that value as `max_tool_calls` and the executor
rejects a request ceiling above its deployment-owned stream limit.
Configure that independent local limit with
`AiProviderCallLimits::with_maximum_builtin_tool_calls`; the existing
`with_maximum_tool_calls` continues to bound custom application-tool calls.
`AiBudgetReservation::authorize_provider_call` adds
`requested_maximum_tool_units`; pass zero for requests without built-ins.
The resulting opaque proof and `ProviderRequestContext` recheck both output and
tool ceilings immediately before transport.

`AiProviderUsageObservation::builtin_tools` is replaced by
`builtin_usage`, returning `AiProviderBuiltinUsage`. Accounting now sees only
exact normalized completed counts, never the requested tool configuration.
Unknown, duplicate, unmatched, over-limit, or incomplete start/completion pairs
fail after the transport boundary and leave the reservation uncertain.
Requested-but-unused built-ins contribute zero units. `OrmAiPricingService`
settles exact web/file-search counts; completed code-interpreter or
image-generation calls remain fail-closed because their authoritative billing
dimensions are not yet modeled.

This is an intentional pre-1.0 breaking Rust API, GraphQL input/output SDL,
private schema, budget-proof, provider-transport, accounting, backup, and
restore-validation change. It adds no entity, index, constraint, credential,
provider-persistent file, background response, or webhook lifecycle. Existing
pricing rows need no data rewrite beyond the generated defaulted-column
migration.

## Unreleased: content-free operational telemetry (crate 0.45.0 to 0.46.0; schema remains 0.43.0)

No database migration, private entity change, GraphQL SDL change, persistent
semantic change, backup change, or restore-fact format change is required. The
AI schema module remains `0.43.0` with 39 private entities.

Hosts may install an exporter-neutral `AiOperationalTelemetrySink` behind the
cloneable `AiOperationalTelemetry` emitter. Emitters pass an owned typed event
to a synchronous, infallible method; sinks should enqueue and return promptly,
may drop events under bounded backpressure, and must not make telemetry
availability affect provider, tool, recovery, retention, or restore outcomes.
There is intentionally no built-in OpenTelemetry dependency or network
exporter.

Use one fresh `AiTelemetryOperationId` to correlate a start/finish pair. It is
random telemetry-only state, not derived from a session, run, attempt,
principal, tool, or provider reference. Do not use it as a metric attribute.
Provider observations expose `gen_ai.operation.name = chat`, authoritative
token counts after success, and well-known native provider values only. Model
and profile names remain excluded because their registry/cardinality and
classification are deployment-owned.

The typed vocabulary cannot carry prompts, model output, tool arguments or
results, GraphQL documents, principal/durable resource IDs, provider response
IDs, endpoint URLs, secret references, arbitrary errors, restore fingerprints,
restore issue/resource text, or retention cursors. `AiRetentionTelemetry`,
`AiRestorePlanTelemetry`, and `AiRestoreReadinessTelemetry` project existing
reports into content-free aggregates; the projections are operational signals,
not audit, erasure, recovery, or runtime-readiness proofs. Existing integrations
need no code change unless they choose to install a sink. This is an additive
public Rust API change with no data migration.

## Unreleased: deleting-session lifecycle closure (crate 0.44.0 to 0.45.0; schema 0.42.0 to 0.43.0)

Apply AI schema module `0.43.0` while session, inbox, retention, attachment,
backup/restore, and runtime workers are closed. Do not run 0.45.0 code against
a module still registered as 0.42.0. The generated migration keeps 39 private
entities, makes the private inbox protected-payload column nullable, and adds a
nullable payload-purge timestamp plus a defaulted CAS row version. It adds no
table, index, constraint, or entity. Existing inbox rows remain in the retained
payload state; no application-authored SQL, data copy, or content rewrite is
required. Private session-state and inbox-session filters also become explicit,
and `deleted` becomes a durable terminal session state.

Deleting-session retention now queries protected principal-inbox events by
their exact session binding and CAS-clears one bounded page of payloads after
the current `deleted_content_purge_seconds` cutoff. Each row receives a trusted
purge timestamp while its principal sequence is retained, so this worker does
not punch a hole in the shared cross-session stream. Inbox readers encountering
a tombstone require an explicit cursor reset; ordinary prefix pruning may later
delete the row contiguously. Message content cannot be scrubbed in the same pass
that finds an unpurged inbox page, so a duplicated notification payload cannot
outlive its source message. Use
`AiSessionRetentionLimits::with_inbox_event_limit` when inbox and session-event
cardinality need different hard bounds.

Once every ordered phase is exhausted, the worker re-proves the current policy
and cutoff, zero remaining session/context/attachment protected rows, a
complete database-side proof that no exact-session inbox row lacks its payload-
purge timestamp, complete retained message tombstones, a bounded entirely
terminal run set, zero current checkpoint pointers, and zero immutable
coordinator checkpoints.
The immediately preceding append-only purge transaction has independently
re-proved proposal/item and tool/approval tombstones plus exact external-object
absence. Only then does one state-machine transaction replace the user-authored
title with an empty tombstone, transition `deleting` to `deleted`, and append a
redacted `finalize_session_deletion` audit. Audit, usage, egress, attempt,
message/run tombstone, ownership/scope, and other required non-content security
facts remain.

Session list queries now constrain state before pagination so deleting/deleted
shells cannot consume visible windows. Direct lookup also hides any session
with `deleted_at`, and repeated delete requests return success for either
`deleting` or `deleted`. No public GraphQL SDL changes. The public Rust report
adds `deleting_session_inbox_payloads_purged` and
`deleting_sessions_finalized`; exhaustive literals must initialize them or use
`..Default::default()`. Restore collectors must extend their existing
`invalid_session_retention_count` validation to reject a `deleted` shell with a
nonempty title, a malformed inbox payload tombstone, any remaining
protected/external session dependency, a nonterminal run, current or retained
checkpoint, missing message tombstone, or an invalid deletion audit transition.

There is no application-authored data migration. Existing `deleting` sessions
continue through bounded scheduled passes and finalize only after all current
proofs succeed. Existing serialized restore facts are unchanged. This is an
intentional pre-1.0 public Rust API, private persistent-semantics, retention,
restore-validation, session-query, and audit behavior change.

## Unreleased: protected context compaction (crate 0.43.0 to 0.44.0; schema 0.41.0 to 0.42.0)

Apply AI schema module `0.42.0` while session, run, provider, compaction,
retention, backup/restore, and runtime workers are closed. Do not run 0.44.0
code against a module still registered as 0.41.0. The generated migration
keeps 39 private entities and adds no table, column, index, constraint, or row
rewrite. It changes private generated insert metadata so the trusted context-
checkpoint writer supplies the primary key before protecting its payload, and
advances the module because exact checkpoint coverage and ordinary-retention
invalidation are new persistent semantics. No consumer schema, application
SQL, protected-value copy, or data migration is required.

The new `OrmAiContextCompactionService::prepare` operation requires a current
running `AiRunLease`, rehydrates its principal, rechecks owner/session/scope
write access, resolves current content protection, and renews the fence. The
requested boundary must advance the latest valid checkpoint, cover a complete
contiguous message range within hard message/block/byte bounds, and leave at
least `minimum_recent_messages` verbatim. A subsequent checkpoint may use the
latest protected summary as its parent and adds only the next exact contiguous
message segment. The prepared provider request contains sensitive opened
content and must remain inside the trusted backend.

Hosts construct the ordinary `AiProviderCallPlan` from
`AiPreparedContextCompaction::model_request`. Its single model-inference
manifest must use purpose `context_compaction`, the exact
`egress_sources`, the returned session/run/scope/provider/model, and byte/token
estimates no smaller than the prepared values. All message blocks and parent
summaries are conservatively classified `Restricted`; user messages retain
`UserProvided` trust and assistant/summary sources remain
`ExternalUntrusted`. The ordinary provider executor still supplies fresh
principal authorization, exact egress decision/audit, atomic budget
reservation, transport uncertainty, and authoritative usage settlement.

Pass only that executor's `AiProviderCallResult` to `persist`. Persistence
rejects a swapped fence/request/provider/model/manifest, custom or built-in
tools, non-visible event kinds, empty/oversized summary text, nonpositive
committed output usage, stale parent lineage, changed message/block rows, and
checkpoint lookahead overflow. The final state-machine transaction re-proves
every source and the current running lease before inserting a protected
payload containing the chained source hash, direct message/block provenance,
parent reference, and run/attempt/budget evidence. Carry the returned renewed
lease into the next run operation. `load_latest` likewise renews and
reauthorizes before opening the latest valid summary; loaded summary text is
untrusted model output and never grants tool, resolver, egress, or replay
authority.

Ordinary message retention now invalidates coverage by physically deleting
every checkpoint whose `through_sequence` could include an eligible expiring
message before scrubbing the message in the same transaction. The checkpoint
query uses one-row lookahead; an over-bound set blocks the message without any
partial deletion. Deleting-session retention keeps its stronger existing
context-before-content page ordering. `AiSessionRetentionReport` adds
`context_checkpoints_invalidated`; downstream exhaustive literals must
initialize it or use `..Default::default()`.

`AiRestoreSnapshotFacts` adds the serde-defaulted but Rust-source-breaking
`invalid_context_checkpoint_count`. Restore collectors must validate exact
prefix and parent lineage, 64-character lowercase source hashes, protected
payload associated identity, direct provenance ordering, positive token
observation, provider/model metadata, run/attempt/generation and committed
budget evidence, plus retention-invalidated rows. Any invalid row increments
that count and keeps readiness closed. Existing serialized restore facts
default the new count to zero, but trusted collectors must populate it after
upgrading.

Existing private checkpoint rows were never produced by a supported service.
The new reader refuses legacy or malformed payloads; deployments that contain
such rows must classify them through restore preflight and remove/rebuild them
under their reviewed maintenance process before opening the runtime. This is a
pre-1.0 public Rust API, private schema-metadata, restore, provider-orchestration,
and retention-behavior change. It changes no public GraphQL SDL, Cargo feature
or default, table/entity count, append-only policy, or dependency revision.

## Unreleased: attachment-artifact retention (crate 0.42.0 to 0.43.0; schema 0.40.0 to 0.41.0)

Apply AI schema module `0.41.0` while session-retention, attachment-cleanup,
provider-file, backup/restore, and runtime workers are closed. Do not run
0.43.0 code against a module still registered as 0.40.0. The generated
migration keeps the existing 39 private entities and adds five nullable
cleanup columns to `graphql_orm_ai_attachment_artifacts`: state, generation,
lease expiry, retry count, and next-attempt time. It also adds stable
created-time/ID keyset metadata and redacts provider references from generated
backup descriptors. Existing artifact rows retain null cleanup state and no row
data is rewritten. No consumer table, protected value, local object key, or
provider reference needs an application-authored migration or copy.

After the deleting-session cutoff, retention now loads the complete artifact
set under `maximum_attachment_artifacts_per_session` with one-row lookahead.
It first CAS-moves each valid artifact into private cleanup state. The separate
attachment worker re-proves the exact parent attachment, deleting session,
current scope policy, and cutoff before rotating a generation and lease. Local
blob deletion must be followed by exact absence confirmation. A provider
reference additionally requires `provider_file_delete_required = true` and an
installed `AiProviderFileDeletionService`; its `Ok(())` contract must mean the
exact provider object is authoritatively absent. Provider expiry, an
unconfigured boundary, or an ambiguous response is not deletion proof.

Only after every external object is confirmed absent does one CAS clear the
artifact blob/provider references and protected derivative, write a tombstone,
and append redacted audit. A later retention pass physically deletes that
artifact metadata, then requests cleanup of the parent attachment. Parent
metadata and linked message content cannot be removed earlier. Retry backoff,
expired leases, concurrent workers, and over-bound sets retain the unsafe
dependency.

Public Rust additions are `AiProviderFileDeletionRequest`,
`AiProviderFileDeletionService`,
`OrmAiAttachmentService::with_provider_file_deletion_service`,
`AiSessionRetentionLimits::with_attachment_artifact_limit`, and its getter.
`AiAttachmentCleanupReport` adds four artifact counters;
`AiSessionRetentionReport` adds artifact cleanup-request and metadata-delete
counters. Downstream exhaustive struct literals must initialize the new fields
or use `..Default::default()`. The cleanup report's original four counters
continue to describe parent attachment rows only.

This is a pre-1.0 public Rust API, private persistent-shape,
backup/schema-fingerprint, keyset-metadata, and retention-behavior change. It
changes no public GraphQL SDL, Cargo feature/default, table/entity count,
append-only policy, or consumer schema. Restore fact collectors must count an
invalid artifact cleanup state, broken parent link, or unconfirmed object
reference in `invalid_attachment_count`; nonzero restore facts keep readiness
closed. Run complete repeated retention and cleanup cycles after migration;
one report is not an erasure certificate.

## Unreleased: orphaned protected-checkpoint retention (crate 0.41.0 to 0.42.0; schema 0.39.0 to 0.40.0)

Apply AI schema module `0.40.0` while session, run, coordinator, retention, and
restore workers are closed. Do not run 0.42.0 code against a module still
registered as 0.39.0. The generated migration adds no table, column, index,
constraint, entity, or retention opt-in and rewrites no row data. The module
version advances because `raw_payload_retention_seconds` now also governs
physical deletion of narrowly selected protected coordinator checkpoints. No
application SQL, consumer-table change, protected-value rewrite, or
application-authored data copy is required.

The age-based checkpoint phase considers only the protected
`provider_turn_persisted`, `tool_batch_persisted`, and
`supervised_tool_batch_persisted` kinds on terminal runs. A candidate must be
at or before the checked current cutoff and absent from every current run
pointer. Inside the database-enforced append-only retention transaction, the
worker re-proves the exact current scope policy, bounded run history, closed
attempt outcome, committed/reconciled budget reservation, and checkpoint
metadata without reading `protected_state`. A provider-turn checkpoint also
requires either its exact terminal tombstoned tool set or the later current
final-output checkpoint plus durable assistant message. Tool-batch checkpoints
require their exact terminal tombstoned calls and approvals. Every selected row
validates before a deterministic exact-cardinality purge and redacted audit.

Current checkpoints, nonterminal or recovery-required runs, missing/ambiguous
attempt outcomes, untombstoned tool authority, incomplete final-output proof,
lookahead overflow, and malformed correlation remain intact. Post-deletion-
cutoff sessions continue through the stronger whole-session deletion workflow
instead. Audit, attempt/outcome, budget/usage, egress, tool/approval metadata,
and session/run shells remain.

`AiSessionRetentionReport` adds `expired_run_checkpoints_deleted` and
`raw_checkpoint_purges_blocked`. Downstream exhaustive struct literals must
initialize them or use `..Default::default()`. This is a pre-1.0 public Rust
API, schema-module semantic, and retention-behavior change. It changes no
public GraphQL SDL, Cargo feature/default, private entity shape/count,
append-only opt-in, or consumer schema. Run complete bounded scan cycles after
migration; one report is not an erasure certificate.

## Unreleased: age-based terminal tool payload retention (crate 0.40.0 to 0.41.0; schema 0.38.0 to 0.39.0)

Apply AI schema module `0.39.0` while session, run, tool, approval,
coordinator, retention, and restore workers are closed. Do not run 0.41.0 code
against a module still registered as 0.38.0. The generated migration adds no
table, column, index, constraint, or entity and rewrites no row data. The module
version advances because `raw_payload_retention_seconds` now has an operational
persistent meaning for the tool/approval tombstones introduced in 0.40.0. No
application SQL, consumer-table change, protected-value rewrite, or
application-authored data copy is required.

For active, archived, and pre-deletion-cutoff sessions, a bounded retention
pass now computes the exact current policy cutoff and considers only tool calls
whose trusted `completed_at` is at or before it. The owning run and exact
application-tool step must be terminal, and any referenced one-shot approval
must be exact, terminal, and state-compatible. Eligible approval resource
bindings/action previews are cleared before the matching tool arguments/result,
and both rows receive `payload_purged_at`. Newer calls and nonterminal runs or
pending/approved/resume-claimed approvals remain intact and do not block an
independent eligible terminal subset. A malformed eligible graph, missing
completion time, lookahead overflow, or CAS race fails closed without partial
scrubbing.

The same redacted IDs, hashes, states, authorization and egress evidence,
approval decision/use metadata, application audit references, timestamps, and
CAS versions documented for 0.40.0 remain. Provider adapters do not persist raw
HTTP response envelopes; they normalize bounded results. Protected provider
state inside coordinator checkpoints has a separate dependency lifecycle and
is not removed by this age-based tool phase.

`AiSessionRetentionReport` adds `expired_tool_payloads_purged`,
`expired_approval_payloads_purged`, and `raw_payload_purges_blocked`.
Downstream exhaustive struct literals must initialize the new fields or use
`..Default::default()`. This is a pre-1.0 public Rust API, schema-module
semantic, and retention-behavior change. It changes no public GraphQL SDL,
Cargo feature/default, private entity shape/count, append-only policy, or
consumer schema. Run complete bounded scan cycles after migration; one report
is not an erasure certificate.

## Unreleased: deleting-session tool and approval tombstones (crate 0.39.0 to 0.40.0; schema 0.37.0 to 0.38.0)

Apply AI schema module `0.38.0` while session, run, tool, approval, proposal,
attachment, coordinator, retention, and restore workers are closed. Do not run
0.40.0 code against a module still registered as 0.37.0. The generated
migration keeps the existing 39 private entities, adds nullable
`payload_purged_at` columns to the tool-call and approval tables, makes tool
`protected_arguments` nullable, and makes approval
`protected_resource_bindings`/`protected_action_preview` nullable. Tool
`protected_result` was already nullable. Use only the generated `graphql-orm`
migration; no application SQL, consumer-table change, blob-key rewrite, or
application-authored data copy is required. Existing protected values remain
unchanged until retention proves them eligible.

At and after the exact current deleting-session cutoff, repeated bounded scan
cycles now order protected content as context summaries, proposal payloads,
tool/approval payloads, attachment cleanup, message bodies, and terminal
coordinator checkpoints. A tool pass first proves the complete session run,
tool-call, and approval sets under lookahead bounds. Every run must be terminal;
each call must have a matching finished application-tool step and one of the
closed terminal outcomes; and each referenced approval must be exact,
one-shot, terminal, and state-compatible with that call. Pending, approved,
resume-claimed, nonterminal, recovery-required, over-bound, malformed, or
inconsistently tombstoned state retains every tool/approval payload and blocks
later content cleanup.

For an eligible whole-session set, one transaction clears approval resource
bindings and canonical previews before clearing tool arguments/results, writes
both tombstone timestamps, and appends redacted audit. IDs, provider/tool
references, canonical hashes, risk, authorization and egress decisions,
application audit references, approval decision/use state, timestamps, and CAS
versions remain. A later checkpoint-purge transaction independently re-proves
the bounded terminal graph and complete tombstone shape before deleting any
append-only checkpoint. Ordinary approval, checkpoint, and consequential-tool
paths treat missing protected payload as unusable and fail closed.

The existing `AiSessionRetentionLimits` constructors remain source-compatible
and derive tool/approval defaults from the message limit. Call
`with_tool_payload_limits` for independent `1..=5_000` tool-call and approval
bounds; new getters return both. `AiSessionRetentionReport` adds
`deleting_session_tool_payloads_purged`,
`deleting_session_approval_payloads_purged`, and
`tool_payload_purges_blocked`. Downstream exhaustive struct literals must
initialize the new fields or use `..Default::default()`.

This is a pre-1.0 public Rust API, private persistent-shape,
backup/schema-fingerprint, and retention-behavior change. It changes no public
GraphQL SDL, Cargo feature/default, table/entity count, append-only policy, or
consumer schema. Deploy the generated schema migration, then run retention in
complete repeated cycles. One pass is not an erasure certificate; tool and
approval metadata, provider raw payloads, proposal metadata, attachment
artifacts, session shells, and immutable audit/usage/history facts remain.

## Unreleased: deleting-session proposal tombstones (crate 0.38.0 to 0.39.0; schema 0.36.0 to 0.37.0)

Apply AI schema module `0.37.0` while session, proposal, retention, attachment,
coordinator, and restore workers are closed. Do not run 0.39.0 code against a
module still registered as 0.36.0. The generated migration keeps the existing
39 private entities, adds nullable `payload_purged_at` to the proposal table,
and makes proposal `protected_payload`/`source_references` plus proposal-item
`protected_suggested_value`/`source_references` nullable. Use only the generated
`graphql-orm` migration; no application SQL, consumer-table change, blob-key
rewrite, or application-authored data copy is required. Existing non-null
payloads and sources remain unchanged until retention proves them eligible.

At and after the exact current deleting-session cutoff, repeated bounded scan
cycles now order protected content as context summaries, proposal payloads,
attachment cleanup, message bodies, and terminal coordinator checkpoints. A
proposal pass first proves the complete session proposal/item set is within its
lookahead bounds. It retains every accepted or accepted-edited proposal because
the ordinary application mutation or authoritative outcome recorder may still
be pending. It may tombstone only rejected, applied, expired, or expired
pending-review proposals whose owning run is terminal. The same transaction
clears all protected item values/rationales/sources/review values, clears the
parent protected payload/sources, writes `payload_purged_at`, changes an expired
pending review to `expired`, and appends redacted audit. Identity, schema,
logical item count, review decisions, creator/reviewer, applied resource and
application-audit references, timestamps, state, and CAS versions remain.

The existing `AiSessionRetentionLimits` constructors remain source-compatible
and derive proposal/item defaults from message/block limits. Call
`with_proposal_limits` for independent `1..=5_000` proposal and `1..=20_000`
item bounds; new getters return both. `AiSessionRetentionReport` adds
`deleting_session_proposal_payloads_purged` and
`proposal_payload_purges_blocked`. Downstream exhaustive struct literals must
initialize the new fields or use `..Default::default()`.

This is a pre-1.0 public Rust API, private persistent-shape,
backup/schema-fingerprint, and retention-behavior change. It changes no public
GraphQL SDL, Cargo feature/default, table/entity count, append-only policy, or
consumer schema. Deploy the generated schema migration, then run retention in
complete repeated cycles. One pass is not an erasure certificate; tool calls,
approvals, provider raw payloads, proposal metadata, attachment artifacts,
session shells, and immutable audit/usage/history facts remain.

## Unreleased: verified deleting-session attachment cleanup (crate 0.37.0 to 0.38.0; schema 0.35.0 to 0.36.0)

Apply AI schema module `0.36.0` while session writers, attachment upload and
cleanup workers, retention workers, provider attachment reopeners, and restore
callbacks are closed. Do not run 0.38.0 code against a module still registered
as 0.35.0. The generated migration adds no table, column, index, constraint,
or entity and rewrites no row data. It advances the module because existing
attachment `quarantine_state`/`processing_state` values now include a private
`deleting`/`retention_cleanup_required` transition whose meaning is bound to
the exact current session-deletion cutoff. No consumer table, protected-payload
rewrite, blob-key rewrite, or application SQL is required.

Hosts must schedule both maintenance services. After
`deleted_at + deleted_content_purge_seconds`,
`OrmAiSessionRetentionService` proves the current exact scope policy and a
whole-session attachment lookahead bound. Artifact-free rows that still own or
may own storage enter the retention cleanup state by CAS; no reference is
cleared at this step. `OrmAiAttachmentService::cleanup_once` then reloads the
session and policy in its claim transaction, re-proves the cutoff, claims one
generation, deletes only the stored opaque final/quarantine references, and
verifies absence. Storage errors or ambiguous absence checks preserve the
references in bounded backoff. A later retention pass may physically delete an
ordinary attachment row only when it has no artifacts, both blob references
and its upload capability hash are absent, its cleanup is complete, and its
deleted timestamp plus a positive cleanup generation are present. Linked
message scrubbing can proceed only after that metadata deletion.

Attachment artifacts—including provider-file references, derivative blobs,
or protected artifact content—remain blockers. This release does not infer
provider deletion, clear an artifact, or weaken any append-only fact. Runs,
attempt history, non-checkpoint immutable facts, tool/proposal payloads, and
session shells remain. One report is not an erasure certificate.

The existing `AiSessionRetentionLimits` constructors remain source-compatible
and use their message bound as the default attachment bound. Call
`with_attachment_limit` to set an independent `1..=5_000` whole-session proof
bound; `maximum_attachments_per_session` returns it.
`AiSessionRetentionReport` adds
`deleting_session_attachment_cleanups_requested`,
`deleting_session_attachments_deleted`, and `attachment_cleanups_blocked`.
Downstream exhaustive struct literals must initialize the new public fields or
use `..Default::default()`.

This is a pre-1.0 public Rust API and persistent lifecycle-behavior change. It
changes no GraphQL SDL, Cargo feature/default, entity shape, append-only policy,
or protected row representation. No row-data migration is needed, but hosts
must deploy the new module version and run cleanup plus retention repeatedly;
running retention without cleanup intentionally leaves attachments blocked.

## Unreleased: terminal run-checkpoint purge (crate 0.36.0 to 0.37.0; schema 0.34.0 to 0.35.0)

Update the exact `graphql-orm` pin from 0.7.0 to 0.9.0 at
`f996cdbe2ef1867dea029ec3ff16e051dbe7566e`, refresh one dependency lockfile,
and apply AI schema module `0.35.0` while session writers, coordinator workers,
retention workers, and restore callbacks are closed. Do not run 0.37.0 code
against a module still registered as 0.34.0. The generated migration adds no
table, column, index, constraint, or entity and rewrites no row data. It does
change managed append-only enforcement for
`graphql_orm_ai_run_checkpoints`: SQLite and PostgreSQL gain the reviewed
transaction-scoped retention-delete path while ordinary update/delete remains
prohibited. Checkpoint IDs also become privately sortable so bounded retention
pages use stable `created_at ASC, id ASC` ordering. PostgreSQL enforcement
objects and managed row-security integration are regenerated by `graphql-orm`;
hosts must not reproduce them with application SQL.

At and after the existing deleting-session cutoff, repeat bounded complete
scan cycles. The worker still removes protected events and context summaries
before eligible message content. Only after those sources are exhausted and
the configured run page proves every run terminal does an ordinary
state-machine transaction validate each current checkpoint and clear
`latest_checkpoint_id`. A separate retention transaction then reloads the
session and policy, repeats the empty-source and terminal-run proofs, requires
all pointers to be absent, purges one exact bounded checkpoint ID set, and
appends a redacted audit in the same commit. A crash between transactions
leaves an orphan checkpoint for a later pass rather than a dangling pointer.

CAS conflicts during ordinary pruning now roll back the entire per-session
transaction before incrementing `sessions_conflicted`. Hosts may safely retry a
later scan cycle; no earlier deletion or pointer change from the conflicted
transaction is committed without its audit.

Only run checkpoints opt in. Immutable run attempts/outcomes, pricing and
skill versions, usage, egress, and audit facts remain non-purgeable. Runs,
messages, session shells, attachments/external objects, tool/proposal payloads,
and unsafe dependencies also remain. MSSQL remains schema-only/read-only and
does not opt the entity into retention purge. One report is not an erasure
certificate.

The existing `AiSessionRetentionLimits` constructors remain source-compatible.
They derive run and checkpoint bounds from their existing message/context
bounds; call `with_run_checkpoint_limits` to set independent values. New public
getters expose those values. `AiSessionRetentionReport` adds
`deleting_session_run_checkpoint_references_cleared`,
`deleting_session_run_checkpoints_deleted`, and
`run_checkpoint_purges_blocked`; exhaustive downstream struct literals must
initialize them or use `..Default::default()`.

Constructing `OrmAiSessionRetentionService` installs the exact
`graphql_orm_ai.run_checkpoint.retention_purge` entity-policy grant on the
service's cloned database handle while delegating every other access surface
to the handle's existing policy. This construction is the host's explicit
enablement of trusted maintenance; it grants no GraphQL or arbitrary purge
surface. Existing row policy may still deny a selected checkpoint.

This is a pre-1.0 public Rust, dependency, database-enforcement, authorization,
backup/schema-fingerprint, and runtime-behavior change. It changes no GraphQL
SDL, Cargo feature/default, entity shape, or protected row representation. No
AI row-data migration, consumer-table migration, or protected-payload rewrite
is required.

## Unreleased: context-first deleting-session retention (crate 0.35.0 to 0.36.0; schema 0.33.0 to 0.34.0)

Apply AI schema module `0.34.0` while session writers, context workers,
retention workers, subscriptions, and restore callbacks are closed. Do not run
0.36.0 code against a module still registered as 0.33.0. The generated
migration adds no table, column, index, constraint, or entity and still owns 39
private records. It advances the module because existing session,
context-checkpoint, message/block, retention-policy, and audit records now have
a context-before-content deletion meaning. No data copy, protected-payload
rewrite, consumer table, or application SQL is required.

After the existing deleting-session cutoff, each
`OrmAiSessionRetentionService` transaction now loads at most the configured
context-checkpoint bound. A nonempty page is validated and deleted atomically,
and all message scrubbing is deferred for that session until a later pass.
Hosts must repeat complete bounded scan cycles until all protected context
summaries are gone; only then can eligible terminal unattached message content
be scrubbed. Protected event deletion may progress in the same earlier passes.
This order prevents a retained summary from outliving message content it may
cover.

The existing four-argument `AiSessionRetentionLimits::new` remains compatible
and uses `maximum_messages_per_session` as the context-checkpoint bound. Hosts
that need an independent limit should migrate to
`new_with_context_checkpoints`. `AiSessionRetentionReport` adds the public
`deleting_session_context_checkpoints_deleted` field; downstream exhaustive
struct literals must initialize it or use `..Default::default()`.

This slice deletes only deleting-session context-summary rows. Ordinary
message-retention invalidation, context-summary production/selection, run and
coordinator checkpoints, tool/proposal payloads, attachment/external content,
append-only facts, and final session-shell deletion remain closed. The context
producer must stay disabled until exact source coverage and ordinary-retention
invalidation are implemented. This is a pre-1.0 public Rust API and persistent
behavior change with no GraphQL SDL or data migration.

## Unreleased: deleting-session content cutoff (crate 0.34.0 to 0.35.0; schema 0.32.0 to 0.33.0)

Apply AI schema module `0.33.0` while session writers, retention workers,
subscriptions, and restore callbacks are closed. Do not run 0.35.0 code
against a module still registered as 0.32.0. The generated migration adds no
table, column, index, constraint, or entity and still owns 39 private records.
It advances the module because existing session, protected event,
message/block, run, attachment, retention-policy, and audit records now have a
deleting-session content-cutoff meaning. No data copy, protected-payload
rewrite, consumer table, or application SQL is required.

Hosts should continue scheduling `OrmAiSessionRetentionService` as bounded
keyset scan cycles. For an exact `deleting` session with a valid `deleted_at`,
the worker now compares that timestamp plus the current scope policy's
`deleted_content_purge_seconds` to its trusted clock. Before the cutoff, the
existing ordinary live-delta/message-retention rules apply. At and after the
cutoff, each bounded session transaction may delete every protected session
event kind and scrub eligible terminal unattached message previews and blocks
even when `message_retention_seconds` is absent. Repeat complete scan cycles
until operational telemetry shows no further eligible rows; one report is not
an erasure certificate.

The worker preserves session/message metadata, unsafe message content linked
to nonterminal runs or attachments, attachments/blobs, provider-persistent
files, raw provider/tool payloads outside these rows, checkpoints, proposals,
approvals, usage, egress decisions, audit facts, fencing, and restore evidence.
It appends a redacted `session_deletion_retention_expired` audit in the same
transaction as each changed session. Those retained dependencies require
separately ordered workers; this slice begins but does not complete the
`deleting` lifecycle. Append-only retention remains closed until the reusable
generated-ORM deletion primitive is reviewed upstream.

`AiSessionRetentionReport` adds the public
`deleting_session_events_deleted` field. Downstream exhaustive struct literals
must initialize it or use `..Default::default()`. This is a pre-1.0 public Rust
API and persistent-behavior change with no GraphQL SDL or data migration.

## Unreleased: live approval-wait reconciliation (crate 0.33.0 to 0.34.0; schema 0.31.0 to 0.32.0)

Apply AI schema module `0.32.0` while run claimers, approval workers, generic
expired-lease recovery, and restore callbacks are closed. Do not run 0.34.0
code against a module still registered as 0.31.0. The generated migration adds
no table, column, index, constraint, or entity and still owns 39 private
records. It advances the module because existing approval, tool-call, run-step,
provider-turn checkpoint, run, event, audit, and attempt-outcome records now
have a live approval-wait reconciliation meaning. No data copy, protected
payload rewrite, consumer table, or application SQL is required.

Hosts should construct `OrmAiApprovalWaitReconciliationService` with the same
generated ORM database, run service, current-principal resolver, content
protection boundaries, clock, and a current
`AiApprovalWaitReconciliationPolicy`. Configure positive principal/wait
durations and a bounded `1..=256` candidate scan through
`AiApprovalWaitReconciliationLimits`. The policy's decision may only leave an
exact pending/approved wait parked or cancel it; it is not approval, resolver,
provider, egress, or replay authority.

Run `reconcile_waits` before `OrmAiRunService::recover_expired_leases` in each
live worker cycle. The reconciler does not heartbeat or poll a human wait.
Generic expired-lease recovery no longer selects `WaitingApproval`; the
dedicated worker owns its decision, policy, expiry, and deployment-cutoff
transition. `WaitingTool` and other externally ambiguous expired states remain
conservative recovery cases.
Denied, revoked, expired, deployment-cutoff, deleted-session, and current-policy
cancellations atomically close the run/call/step fence and append protected,
redacted, and immutable outcome facts. Valid pending/approved waits remain
unchanged. Exact CAS races are reported and can be reconsidered by the next
bounded cycle. Malformed or unprovable checkpoint/budget/call/step/approval
linkage moves the run to `RecoveryRequired` without changing the linked
approval or call.

Approved work still uses only `claim_next_approved`, fresh preview/policy/rule
validation, one-shot consumption, and the ordinary authenticated GraphQL
resolver path. The reconciler never claims or resumes it. During snapshot
restore, keep all workers closed: restored `WaitingApproval` and `WaitingTool`
states continue to become `RecoveryRequired` through restore reconciliation
and are deliberately not eligible for this live worker. This is a behavioral
and public Rust API change with no GraphQL SDL or data migration.

## Unreleased: bounded sequential supervised coordinator (crate 0.32.0 to 0.33.0; schema 0.30.0 to 0.31.0)

Apply AI schema module `0.31.0` while workers, provider calls, approval waits,
and restore callbacks are closed. Do not run 0.33.0 code against a module still
registered as 0.30.0. The generated migration adds no table, column, index,
constraint, or entity and still owns 39 private records. It advances the module
because the existing provider-turn, approval-wait, supervised-result
checkpoint, and run states now have a top-level sequential orchestration
meaning. No data copy, consumer table, application SQL, or protected-payload
rewrite is required.

Hosts may now construct `AiSupervisedAgentCoordinator` from their existing
fenced run control, provider executor, protected output/checkpoint services,
consequential approval service, supervised resume service, current-rule
resolver, trusted clock, and a new `AiSupervisedAgentTurnPlanner`. Route normal
queue claims to `execute_claimed`; route one-owner
`OrmAiRunService::claim_next_approved` results to `execute_approved_claim`.
Never call both entry points concurrently for one run fence.

Every `AiSupervisedAgentTurnPlan` must contain only exact registered
`SupervisedWrite`/`OneShot` definitions, use provider-retained continuation,
match the resolved-rule scope/fingerprint, and carry a current server-selected
result-egress route. Initial plans have no continuation. Continuation plans
must use `AiProviderCallPlan::new_supervised_continuation_with_tools` with the
opaque result supplied by the coordinator; do not reconstruct call IDs,
provider response IDs, or model-visible result blocks.

The coordinator checkpoints each accepted provider result before staging one
canonical-preview approval and then returns `WaitingApproval` without
heartbeating through the human wait. After approval, the existing resume
service reopens the exact provider checkpoint, consumes the approval once,
executes the ordinary authenticated GraphQL resolver, and protects its result.
The coordinator re-adopts and consumes that result checkpoint immediately
before a freshly planned provider turn. A later turn may request another
single mutation, producing a new independent approval. Parallel/mixed tool
batches, stateless supervised continuation, autonomous writes, model-authored
GraphQL, and mutation replay remain rejected.

`AiSupervisedResumeOutcome::RecoveryRequired` now includes `provider_turns`
and `total_tool_calls`, exposes matching getters, and the enum is
`#[non_exhaustive]`. Downstream matches must use `..`; downstream code must not
construct this outcome as authority. This is a pre-1.0 source-breaking API
change. The new top-level outcome and planner/stager/checkpoint/resume traits
are re-exported from the crate prelude.

Read-only and supervised coordinators now check remaining provider-turn
capacity before consuming an exact continuation checkpoint. The supervised
coordinator also refuses to stage an approval on the final allowed provider
turn, because no permitted turn would remain to disclose the mutation result.
This is a fail-closed behavior change and needs no data migration.

Denied, revoked, never-approved, and expired human decisions still require the
host's bounded waiting-run reconciliation worker; `execute_approved_claim`
accepts only an exact approved claim. Do not poll or heartbeat a pending human
wait through this coordinator. Multi-call and stateless supervised resumption
(including Ollama/local-harness mutation waits) remain closed.

## Unreleased: cross-generation supervised checkpoint adoption (crate 0.31.0 to 0.32.0; schema 0.29.0 to 0.30.0)

Apply AI schema module `0.30.0` while workers, provider calls, restore
callbacks, and approval execution are closed. Do not run 0.32.0 code against a
module still registered as 0.29.0. The generated migration adds no table,
column, index, constraint, or entity and still owns 39 private records. It
advances the module because the existing protected supervised-checkpoint kind
gains a stricter approval-binding payload and becomes eligible for
cross-generation adoption. No data migration, consumer table, application
SQL, or payload rewrite is required.

After an exact `supervised_tool_batch_persisted` checkpoint loses its worker
lease, expired-lease recovery may now requeue it under a new attempt and lease
generation. The recovery transaction requires one completed write-risk tool,
its exact consumed one-use approval, complete step/result/egress state, a
committed reconciled provider budget, and the checkpoint hash. It never
executes or retries the consequential resolver.

`OrmAiCoordinatorCheckpointService::adopt_supervised_tool_batch` then reopens
the old generation's protected checkpoint and every protected argument,
result, approval-resource, and canonical-preview envelope. It verifies the
exact provider response/budget/tool/approval/egress rows, approval binding,
preview hash, policy/auth-state evidence, current principal/scope/protection
policy, and current hierarchical rules before returning the opaque
`AiAdoptedSupervisedToolBatch`. The provider-retained continuation remains
private. `consume_supervised_before_provider` accepts that proof and clears the
exact latest-checkpoint link through the current row-version fence; it must run
before the next provider transport and succeeds only once.

Trusted backup/restore fact producers must populate the new
`AiRestoredRun::coordinator_checkpoint` field using
`AiRestoredCoordinatorCheckpoint`. A confirmed external effect is eligible for
`RequeueWithNewAttempt` only when the snapshot state is `Running`, the linked
checkpoint was fully validated as `SupervisedToolBatch`, and a provider
continuation exists. `WaitingApproval`, `WaitingTool`, uncertain effects,
uncheckpointed confirmed mutations, invalid coordinator counts, and malformed
adoption evidence remain `RecoveryRequired` or fatal. This new required field
is a public Rust construction API change; update snapshot adapters before
upgrading. Legacy serialized facts without the field deserialize as `None` and
therefore fail closed rather than acquiring adoption eligibility.

The supported supervised checkpoint is still exactly one mutation with a
provider-retained response ID. Multi-call, partial-batch, and stateless
supervised adoption (including Ollama/local-harness approval waits) remain
closed. The top-level supervised provider coordinator is a later gate. Existing
read-only checkpoint adoption remains strictly read-only.

## Unreleased: protected supervised continuation handoff (crate 0.30.0 to 0.31.0; schema 0.28.0 to 0.29.0)

Apply AI schema module `0.29.0` while workers, provider calls, human approval
waits, and restore callbacks are closed. Do not run 0.31.0 code against a
module still registered as 0.28.0. The generated migration adds no table,
column, index, constraint, or entity and still owns 39 private records. It
advances the module because existing private checkpoint records gain a new
authorization-sensitive kind and stricter interpretation. No consumer table,
application SQL, or data copy is introduced.

`OrmAiSupervisedResumeService::execute_claimed` accepts the exact
`AiApprovedRunClaim`. It reopens the linked `provider_turn_persisted`
checkpoint, committed provider budget, single staged tool, and
`resume_claimed` approval under current principal, scope/session, protection,
and hierarchical-rule authority. It then uses the normal consequential tool
service to rebuild the canonical preview, consume approval once, and execute
the ordinary authenticated GraphQL mutation. It never calls the provider.

An unambiguous model-visible result is protected as
`supervised_tool_batch_persisted`, with the exact consumed approval, result
egress manifest, provider-retained response continuation, rule fingerprint,
and cumulative provider/tool usage. `AiSupervisedResumeOutcome` returns either
that opaque checkpoint or a durable recovery-required tool ID. If resolver or
post-mutation persistence is ambiguous, no approval or mutation is replayed.

This first resume contract accepts exactly one supervised mutation and a
provider-retained response ID. Multi-call batches and stateless continuation
(including Ollama and local-harness turns) remain closed at this handoff until
their complete ordering/history evidence is implemented. Existing provider
and local-harness support is unchanged outside approved-wait resumption.

Trusted supervised planners should call the new public
`AiProviderCallPlan::project_supervised_rule_usage` with the exact freshly
resolved hierarchy before provider execution. Plans now retain private
plan-time fingerprint/maturity/approval bindings: safe reads must remain
approval-free, and supervised mutations must remain one-shot. The method also
checks provider capabilities, classification, retention/BYOK, and estimated
usage, but does not replace atomic budget reservation, egress, tool policy, or
resolver authorization.

Read-only `tool_batch_persisted` append and adoption now reject every tool row
whose risk is not `read_only` or whose approval ID is present, including all
stateless history. The new supervised kind requires one allowed write-risk row
and its exact consumed, one-use approval. Finish or reconcile active 0.30.0
coordinator checkpoints before upgrading; legacy ambiguous/misclassified
records are not adopted. No data migration is required.

Live expired-lease and snapshot restore do not yet adopt a supervised
continuation across a new attempt/generation. A process loss before or after
the supervised checkpoint therefore remains `RecoveryRequired`; do not relink
or replay it manually. Cross-generation adoption and the top-level supervised
provider loop are later gates.

## Unreleased: fenced approved-wait handoff (crate 0.29.0 to 0.30.0; schema 0.27.0 to 0.28.0)

Apply AI schema module `0.28.0` while workers, approval decisions, backups,
and restore callbacks are closed. Do not run 0.30.0 code against a module
still registered as 0.27.0. The generated migration adds no table, column,
index, constraint, or entity and still owns 39 private records. It advances
the module because existing approval/run records gain strict durable handoff
semantics. No consumer table, application SQL, or data copy is introduced.

Workers that resume human-approved actions should call
`OrmAiRunService::claim_next_approved`. The returned `AiApprovedRunClaim`
contains private, non-forgeable approval/tool IDs and the sole current lease.
The transaction preserves the existing attempt and lease generation so the
staged approval, provider usage, and tool call retain their exact bindings,
but rotates owner, expiry, heartbeat, and row version. It also changes the
approval from `approved` to `resume_claimed`, moves the run from
`WaitingApproval` to `WaitingTool`, and appends a redacted audit fact. Exactly
one concurrent worker succeeds; the old waiting lease becomes stale.
Expired `approved` rows encountered in a bounded claim scan are changed to
`expired` with a redacted audit fact before the scan continues, preventing an
old block of approvals from permanently starving newer eligible work.

`AiApprovalState` adds the pre-1.0 `ResumeClaimed` variant. Approval views may
return `resume_claimed`. Consumption accepts either the original direct
`approved` path or the claimed path, always rehydrates and rebuilds the exact
binding, then atomically clears the internal run marker while moving to
`Running`. Revocation accepts both unconsumed states. A claim remains neither
approval consumption nor resolver/rule/egress authority.

This is the durable queue-handoff foundation, not yet the complete top-level
supervised coordinator. Consumers must not reconstruct provider continuations
or replay a mutation after a resumed worker crash. Full protected provider-turn
adoption will build on this proof. Existing 0.29.0 approvals require no data
migration; finish or reconcile active waits before upgrading so their state is
not interpreted across versions.

Restore snapshot producers must include pending, approved, and
`resume_claimed` unconsumed rows in `pending_approval_count`. The pure restore
planner now classifies both `WaitingApproval` and `WaitingTool` as
`RecoveryRequired` regardless of the coarse external-effect flag; a restored
snapshot cannot use the live same-attempt handoff or infer replay authority.

## Unreleased: rule-bound coordinator checkpoints (crate 0.28.0 to 0.29.0; schema 0.26.0 to 0.27.0)

Apply AI schema module `0.27.0` while workers, provider calls, backups, and
restore callbacks are closed. Do not run 0.29.0 code against a module still
registered as 0.26.0. The generated migration adds no table, column, index,
constraint, or entity and still owns 39 private records. It advances the
module because existing protected run-checkpoint fields now require strict v2
rule fingerprint and cumulative usage semantics. No consumer table or raw SQL
is introduced.

Every `AiReadOnlyAgentTurnPlan::new` call now supplies an exact
`AiResolvedRuleSet` and a trusted planner-derived `uses_byok` flag. Construct
`OrmAiCurrentRuleResolver` from the durable current-principal resolver, the
same `Arc<dyn AiRulePolicyService>` used for GraphQL rule management, a trusted
clock, and bounded principal freshness. Install it as the new
`AiAgentRuleResolver` argument on both `AiReadOnlyAgentCoordinator` and
`OrmAiCoordinatorCheckpointService`; normally one shared instance is used at
both boundaries.

Checkpoint-writer implementations receive the exact rules and
`AiRuleRunUsage`. Protected format v2 binds the target/fingerprint and
cumulative provider calls, provider/application-tool steps, trusted start
time, output tokens, cost, tool units, and image units. The coordinator checks
estimated capacity before provider egress and replaces it with authoritative
committed usage after return. It re-resolves the current hierarchy before
transport, after transport, before each resolver tool, around checkpoint
protection, and during adoption. A pre-egress mismatch fails safely; a
post-egress mismatch or actual-usage overrun becomes `RecoveryRequired` rather
than replaying or exposing the result.

The new checks only narrow. They do not replace atomic budget reservations,
authoritative pricing, egress manifests, provider-profile authorization,
current tool policy, ordinary GraphQL resolver authorization, or approval.
`uses_byok` is a server-owned planning assertion checked against the rule set,
not proof that a credential exists or is usable. A turn exposing any custom
application tool also requires both the `CustomTools` and
`ParallelToolCalls` rule capabilities: even one advertised tool definition can
be selected more than once in a provider turn.

Legacy protected coordinator checkpoint v1 does not contain enough evidence
for safe adoption and is deliberately rejected by 0.29.0. Before upgrade,
finish or reconcile active 0.28.0 runs. If an old checkpoint remains after a
crash/restore, keep the runtime closed and classify the run for privileged
manual recovery; do not rewrite protected checkpoint JSON or counters with
application SQL.

Restore snapshot producers must populate the new
`AiRestoreSnapshotFacts::invalid_coordinator_checkpoint_count`. Count legacy
format, malformed protected state, rule fingerprint/current-lineage mismatch,
invalid cumulative usage, or fence/scope mismatch. Any nonzero value emits
fatal `AI_RESTORE_COORDINATOR_CHECKPOINT_INVALID` evidence. This field and the
additional constructor/trait arguments are pre-1.0 source-breaking changes.
No consumer-data migration is required.

## Unreleased: hierarchical rule narrowing (crate 0.27.0 to 0.28.0; schema 0.25.0 to 0.26.0)

Apply AI schema module `0.26.0` while workers, rule/configuration mutations,
backups, and restore callbacks are closed. Do not run 0.28.0 code against a
module still registered as 0.25.0. The generated migration adds no table,
column, index, constraint, or entity and still owns 39 private records. It
advances the module because the existing private scope-policy record now has a
strict deterministic ID, deny-unknown-fields v1 hierarchical-rule payload,
scope-bound checksum, and restore meaning. No application raw SQL or
consumer-owned data access is introduced.

New public Rust APIs include `AiRuleConstraints`, budget/provider/approval
constraint types, `AiRuleDeploymentLimits`, `AiResolvedRuleSet`, access and
hierarchy traits, `AiRulePolicyService`, the redacted GraphQL input/view and
roots, and `OrmAiRulePolicyService`. Compose the rule roots separately and
install one `Arc<dyn AiRulePolicyService>`. Writes require a current principal,
exact `Manage` authorization, recent MFA, immutable deployment-limit
validation, and CAS. Reads and run resolution have independent authorization
actions.

Implement `AiRuleHierarchyResolver` from authoritative application state and
the current principal. It must return the complete broadest-to-target lineage.
Every participating layer must have an explicit policy; a missing, duplicate,
over-depth, wrong-target, cross-tenant, unauthorized, corrupt, or deployment-
widening layer fails closed. GraphQL scope kinds remain opaque strings and do
not add any product entity or tenant hierarchy to this crate.

Resolve the hierarchy before trusted run planning and carry its canonical
fingerprint and exact row versions into host orchestration. Apply the effective
tool approval floor and budget ceilings at their real execution boundaries.
`AiResolvedRuleSet` is only negative constraint evidence: a positive helper
result is not tool enablement, resolver authorization, disclosure approval,
provider routing, egress authorization, spend reservation, BYOK permission, or
one-shot approval consumption.

The GraphQL SDL adds `aiRulePolicy`/`AiRulePolicy` and
`setAiRulePolicy`/`SetAiRulePolicy`, plus their inputs and enums, following the
selected camelCase/PascalCase feature without aliases. Secret classification
and autonomous-write maturity are absent. An absent allowlist/budget value
inherits the effective broader constraint; an empty allowlist or zero budget
explicitly denies that dimension.

Restore snapshot producers must populate the new
`AiRestoreSnapshotFacts::invalid_rule_policy_count`. Any nonzero value emits
fatal `AI_RESTORE_RULE_POLICY_INVALID` evidence and keeps the runtime closed.
This added public struct field is a pre-1.0 source-breaking change for
struct-literal producers.

The public service did not exist in 0.27.0, so a normal deployment has no
service-created policy rows and needs no row rewrite or consumer-data
migration. If a private integration pre-seeded `AiScopePolicyRecord`, treat
those rows as unsupported legacy data: keep the runtime closed and replace
them through the authenticated `setAiRulePolicy` mutation as part of a
controlled migration. Do not expose generic CRUD roots or repair private JSON
with application SQL.

## Unreleased: durable validated UI-intent suggestions (crate 0.26.0 to 0.27.0; schema 0.24.0 to 0.25.0)

Apply AI schema module `0.25.0` while AI workers, subscriptions, backups, and
restore callbacks are closed. Do not run 0.27.0 code against a module still
registered as 0.24.0. The generated migration adds no table, column, index,
constraint, or entity and still owns 39 private records. It advances the
module because existing session/inbox event rows now have a strict protected
`ui_intent_suggested` semantic bound to an exact provider result, descriptor,
committed budget reservation, owner/scope, audit fact, and run fence. No raw
SQL or consumer-owned data access is introduced.

New public Rust APIs are `AiPersistedUiIntent`,
`AiUiIntentDeliveryService`, `AiUiIntentDeliveryLimits`, and
`OrmAiUiIntentDeliveryService`. Construct the ORM service with the same fenced
run service, current-principal resolver, session/scope access policy,
content-protection policy/protector, trusted clock, and immutable UI-intent
catalog used by the worker. Delivery consumes an exact
`AiUiIntentTypeBinding`; catalog registration alone is not enablement or
authorization.

For a provider turn that returns a UI-intent envelope, persist the ordinary
protected assistant output first, pass its renewed lease to UI-intent delivery,
then pass the delivery result's renewed lease to the next fenced write or run
completion. The provider-visible text must be exactly one camelCase object:
`{"formatVersion":1,"intentType":"…","payload":{…}}`. The normalized
event stream must contain one ordered start, usage, and completion; hidden
reasoning, tool calls, built-ins, citations, unknown events, extra envelope
fields, stale fingerprints, schema-invalid payloads, mismatched response/usage
evidence, or absent committed budget proof fail closed. Exact retries return
the existing event without advancing either stream or fence twice.

Restore snapshot producers must populate the new
`AiRestoreSnapshotFacts::invalid_ui_intent_event_count`. Validate protected
session/inbox event pairs, deterministic source and descriptor evidence,
owner/scope linkage, the matching committed budget fact, and redacted audit.
Any nonzero value emits fatal `AI_RESTORE_UI_INTENT_EVENT_INVALID` evidence and
keeps the runtime closed. This additional public struct field is a pre-1.0
source-breaking change for struct-literal producers.

Existing 0.26.0 deployments have no crate-created UI-intent events because the
delivery service did not exist. They need no row rewrite or consumer-data
migration. If a private integration previously reused the
`ui_intent_suggested` event name, treat those rows as unsupported legacy data
and keep the runtime closed until its controlled restore/migration process has
removed or replaced them; do not repair protected event payloads with
application raw SQL. GraphQL SDL is unchanged.

## Unreleased: protected skills and typed UI intents (crate 0.25.0 to 0.26.0; schema 0.23.0 to 0.24.0)

Apply AI schema module `0.24.0` while AI workers, backups, and restore callbacks
are closed. Do not run 0.26.0 code against a module still registered as
0.23.0. The generated migration adds no table, column, index, constraint, or
entity and still owns 39 private records. It advances the module because the
existing skill/version fields now have strict v1 protected-instruction,
policy, checksum, provenance, and restore semantics. Skill scope fields also
participate in generated exact-scope filters, and skill-version IDs are
assigned by the catalog before protection so their row identity can be bound
into the protected envelope. Neither change introduces application-written
SQL.

The new separately composable GraphQL SDL exports `AiSkillQueryRoot` and
`AiSkillMutationRoot` with bounded redacted list, safe metadata upsert,
immutable version publication, and enable/disable operations. Names follow the
selected camelCase or PascalCase feature with no aliases. Install exactly one
`Arc<dyn AiSkillCatalogService>` in schema data. The concrete ORM service also
requires an `AiSkillAccessPolicy`, current `AuthPrincipal`, ready exact-scope
content-protection resolver/protector, recent-MFA policy, and trusted clock.

The service was not publicly available before 0.26.0 and private generated
skill CRUD roots have never been exported, so a normal 0.25.0 deployment has no
catalog-created skill rows and needs no row rewrite. If an early deployment
privately pre-seeded these tables, treat those rows as unsupported legacy data:
keep the runtime closed, inventory them through the deployment's controlled
backup/migration process, and publish a replacement current version through
the authenticated skill GraphQL mutation using the known skill ID and CAS
version. Do not repair policy JSON with application raw SQL. Unknown fields,
legacy empty objects, malformed protected content, or a legacy current version
fail closed until replaced. Consumer-owned data is unaffected.

New public Rust APIs include the skill inputs/views/service/access policy and
ORM service, plus `AiUiIntentTypeId`, `AiUiIntentTypeDescriptor`, exact
bindings, draft/validated values, and `AiUiIntentCatalog`. UI-intent schemas
must explicitly declare JSON Schema 2020-12. A skill stores the descriptor
fingerprint, not only its logical name. Consumers must re-register the exact
descriptor on startup and validate model drafts with `validate_bound` before
delivery. A validated intent remains a suggestion: the consumer must recheck
current resource authorization and map the logical type to frontend behavior.
No route or navigation is performed by this crate.

Restore snapshot producers must populate the new
`AiRestoreSnapshotFacts::invalid_skill_catalog_count`. Count any malformed
skill/current-version relationship, protected envelope, strict policy object,
provenance, or checksum. Any nonzero value produces fatal
`AI_RESTORE_SKILL_CATALOG_INVALID` evidence and must keep the runtime closed.

This is a pre-1.0 additive Rust API and GraphQL SDL change plus a persistent
semantic, authorization, content-protection, audit, backup, and restore
contract change. No database DDL or consumer-data migration is required for
deployments that did not privately pre-seed skill rows.

## Unreleased: owned PostgreSQL parity harness (crate 0.25.0; schema 0.23.0)

CI now runs the PostgreSQL parity test through a container created by the test
itself on the local Docker socket. The harness generates its own user,
password, database, container identity, ownership label, and Docker-assigned
IPv4 loopback port. It never reads or accepts a database URL and verifies its
ownership label before removing the container. Local runs skip only when the
local Docker socket is unavailable; CI fails closed instead. The 0.26.0
harness additionally exercises protected skill publication/resolution through
generated ORM operations.

This changes test and release-gate behavior only. It adds no public Rust API,
GraphQL SDL, entity, index, constraint, persistent semantic, authorization,
backup, or restore change. `AI_SCHEMA_MODULE_VERSION` remains `0.23.0`; no
database or consumer-data migration is required.

## Unreleased: profiled OpenAI-compatible adapter (crate 0.24.0 to 0.25.0; schema 0.22.0 to 0.23.0)

Apply AI schema module `0.23.0` while configuration/provider workers, backups,
and restore callbacks are closed. Do not run 0.25.0 code against a module still
registered as 0.22.0. The generated migration adds no table, column, index,
constraint, or entity and still owns 39 private records. The module advances
because `AiProviderProfileRecord.data_policy` now stores a strict version-1
OpenAI-compatible capability and retention contract.

The GraphQL `UpsertAiProviderProfileInput` SDL adds nullable
`openaiCompatible`/`OpenaiCompatible` input according to the selected naming
feature, and `AiProviderProfileView` adds the corresponding redacted view.
Creating or updating an `OpenAiCompatible` profile requires this nested value;
all other provider kinds reject it. The retention label is bounded and the
parallel-tool flag requires custom tools. Updating a profile remains
recent-MFA-, host-policy-, endpoint-policy-, CAS-, and audit-gated.

Existing compatible profiles whose `data_policy` is the legacy empty object
remain readable with no compatible contract, but they cannot construct the new
adapter. Re-save each intended profile through the authenticated GraphQL
mutation with an explicitly reviewed endpoint, retention label, and minimal
capability set before enabling routing. Unexpected or malformed nonempty
policy data fails closed. Existing native-provider profiles need no rewrite.
No consumer-owned or chat data is migrated.

Enable `provider-openai-compatible` to export
`OpenAiCompatibleProviderConfig`, `OpenAiCompatibleCapabilities`, and
`OpenAiCompatibleProvider`. The adapter expects a Responses-compatible SSE
endpoint—not the older Chat Completions surface—and never discovers
capabilities. Build from the redacted profile plus its separately loaded
`SecretRef`, then pass the same deployment endpoint policy and secret store to
the provider constructor. Every call needs exact egress and atomic budget
proofs matching the profile ID, normalized destination, provider/model, and
retention declaration.

This is a pre-1.0 additive Rust API/Cargo-feature change and an additive
GraphQL SDL plus persistent semantic change. It changes provider routing,
configuration, egress, retention, and restore validation contracts. No
database-row or consumer-data rewrite is required beyond the administrator
re-save needed to activate a legacy compatible profile.

## Unreleased: native xAI adapter (crate 0.23.0 to 0.24.0)

Enable `provider-xai` to activate the native xAI Responses/SSE adapter and its
optional HTTP dependencies. The feature now exports `XAiProviderConfig` and
`XAiProvider`. Construct configuration from a secret-store `SecretRef`, then
supply an `Arc<dyn AiSecretStore>` to `XAiProvider::new`. The production URL is
fixed to xAI's official Responses endpoint; GraphQL, provider profiles, and
model input cannot select a URL, header, or plaintext credential.

`require_zero_data_retention` defaults to true and requires the exact xAI
response attestation before any streamed output is accepted. Hosts without
xAI enterprise ZDR must explicitly set it false and separately ensure the
egress policy describes and permits xAI's documented ordinary retention.
`store_responses` remains false by default. It cannot be combined with required
ZDR verification and still needs an exact provider-response retention proof on
every call when enabled. Existing OpenAI provider configuration is unchanged.

The initial adapter supports bounded text/JSON, JSON-schema structured output,
and strict custom/parallel application tools. Every request requires an output
token ceiling. Attachments, xAI server tools, stateless/encrypted-reasoning
continuation, and arbitrary endpoints fail closed. The shared Responses
normalizer now rejects a non-SSE response and any built-in event whose exact
kind was not in the server-authored request. It also requires exact model,
response ID, completed status, usage, bounded event/text/tool-call state, and
an unambiguous terminal event. This tightens malformed, truncated, swapped, or
unsolicited OpenAI responses as well.

This is a pre-1.0 additive Rust API, feature, dependency, provider transport,
retention, egress, and behavioral contract change. It adds no GraphQL SDL,
persistent entity, index, constraint, backup/restore behavior, or data semantic
change. `AI_SCHEMA_MODULE_VERSION` remains `0.22.0`; no database or consumer
data migration is required.

## Unreleased: native Anthropic adapter (crate 0.22.0 to 0.23.0)

Enable `provider-anthropic` to activate the native Anthropic Messages/SSE
adapter and its optional `reqwest` dependency. The feature now exports
`AnthropicProviderConfig` and `AnthropicProvider`; code that treated the
previous empty feature as a marker should update its feature expectations.
Construct configuration with a secret-store `SecretRef`, optionally narrow
the bounded timeout, then supply an `Arc<dyn AiSecretStore>` to
`AnthropicProvider::new`. The endpoint and `anthropic-version` header are
adapter-owned and cannot be selected through GraphQL or model input.

Requests require an explicit `maximum_output_tokens`, exact Anthropic egress
proof, and atomic provider-call budget proof. Supported inputs are bounded
text/JSON, strict application tools with protected stateless continuation,
and JSON-schema structured output. Attachments, provider built-ins,
provider-retained continuation, extended thinking, and prompt-cache creation
are rejected. Cache-read usage is reported as a subset of checked total input;
nonzero cache creation fails closed because the generic authoritative pricing
catalog does not yet represent Anthropic's separate cache-write price class.

This is a pre-1.0 additive Rust API, feature, dependency, provider transport,
egress, and accounting behavior change. It adds no GraphQL field or SDL
change, persistent entity, index, constraint, backup/restore behavior, or data
semantic change. `AI_SCHEMA_MODULE_VERSION` therefore remains `0.22.0`; no
database or consumer-data migration is required.

## Unreleased: stateless checkpoint adoption (crate/schema 0.21.0 to 0.22.0)

Apply AI schema module `0.22.0` while provider/coordinator workers, backups,
and restore callbacks are closed. Do not run 0.22.0 code against a module
registered as 0.21.0. The generated migration has no table, column, index, or
consumer-data rewrite and the module still owns 39 private entities. The
module version advances because the protected checkpoint and restore semantic
contract now permits a fully proven stateless tool history to cross a lease
generation.

Expired-run recovery may now requeue an exact completed stateless
`tool_batch_persisted` checkpoint instead of classifying lease loss as
`RecoveryRequired`. The replacement worker must use
`AiAgentCheckpointAdopter`; it cannot read or reconstruct checkpoint JSON
itself. Adoption rehydrates current authority, opens the protected payload,
and validates every historical and current tool against its original
attempt/generation, committed budget reservation, finished run step,
canonical arguments, protected result, disclosure classification, immutable
allow audit, and unique tool-result manifest. It then rechecks authority and
the protection policy. No application resolver or previous provider turn is
rerun. The linked checkpoint is still atomically consumed through the new
fence before the next provider transport.

Existing 0.21.0 stateless version-2 checkpoints need no rewrite or backfill.
They become eligible only when all durable evidence satisfies the stricter
adopter; missing provider-name metadata, stale policy, malformed history,
tampering, duplicate identities, incomplete work, or denied egress fails
closed. Existing provider-retained checkpoint behavior is unchanged. No
consumer-owned application/domain data is read or changed by migration.

There is no new public Rust item, feature/default change, or GraphQL SDL
change. This is a pre-1.0 persistence-semantic, restore, and behavioral
contract change. Hosts that intentionally treated every stateless lease loss
as permanently non-resumable should update operator runbooks: exact completed
batches can now be safely requeued, while provider-turn, partial-batch,
consequential, and otherwise ambiguous checkpoints remain
`RecoveryRequired`.

## Unreleased: stateless local tool continuation (crate/schema 0.20.0 to 0.21.0)

Apply AI schema module `0.21.0` while provider workers, coordinator workers,
backups, and restore callbacks are closed. Do not run 0.21.0 code against a
module registered as 0.20.0. The module still owns 39 private entities and the
generated migration has no table, column, index, or consumer-data rewrite.
The module version advances because the protected coordinator-checkpoint
semantic contract now accepts version-2 stateless conversation payloads.
Existing version-1 provider-retained checkpoints remain readable.

Public Rust API changes are pre-1.0 breaking for exhaustive matches and struct
literals:

- `ModelRequest` adds required `continuation_mode`; use
  `ProviderRetained` for the existing OpenAI response-ID path and
  `StatelessReplay` only for an adapter that advertises it.
- `ModelContinuation` adds `StatelessConversation`, and the new
  `ModelConversationMessage`/`ModelConversationToolCall` types retain exact
  bounded visible history. Update exhaustive matches.
- `ProviderCapabilities` adds `provider_retained_continuation` and
  `stateless_continuation`. Providers and registries must state these
  independently; neither is inferred from `custom_tools`.
- Provider plans now reject duplicate manifest hashes and permit at most 288
  transfers so a bounded stateless replay can carry one distinct proof for
  each of up to 256 tool results. Every `ToolResult` manifest must contain one
  `application_tool_result` source and may cover exactly one replayed result.

The native Ollama adapter now accepts only server-authored custom-tool plans in
`StatelessReplay` mode. It maps the full protected text/JSON, assistant-call,
and tool-result history into `/api/chat`, rejects hidden thinking and
provider-retained continuation, and normalizes only offered function names
back to local tool IDs. Existing text/image/structured requests in
`ProviderRetained` mode continue unchanged.

The installed-harness framing contract changes from
`graphql-orm-ai/local-harness-jsonl/v1` to `/v2`. Update reviewed harnesses to
accept `continuation_mode`, `continuation`, and `tools` in the single request
frame. A tool-capable registration must set `custom_tools = true` and
`stateless_continuation = true` together; `parallel_tool_calls` is optional.
The driver accepts only exact offered tool IDs in bounded
start/delta/complete order. Text-only harnesses may keep both capabilities
false but must still implement v2 framing.

Stateless tool batches are protected and checkpointed through generated
`graphql-orm` repositories and transactions; no raw SQL is introduced. The
same fenced generation consumes the checkpoint before its next provider call.
If that lease expires, restore validates the durable budget/tool/step/hash
evidence but moves the run to `RecoveryRequired`; it does not replay a local
model or application resolver. Cross-generation adoption remains limited to
provider-retained response-ID checkpoints. No existing data backfill is
needed, and no consumer-owned application/domain data is changed.

This is a pre-1.0 Rust API, provider capability, local-harness protocol,
persistence-semantic, restore, and behavioral contract change. It adds no
GraphQL field or root and therefore does not change the public GraphQL SDL.

## Unreleased: bounded session content retention (crate/schema 0.19.0 to 0.20.0)

Apply AI schema module `0.20.0` while session/provider workers, subscriptions,
backups, and restore callbacks are closed. Do not run 0.20.0 code against
module 0.19.0. The module still owns 39 private entities and changes only
AI-owned storage:

- make `graphql_orm_ai_messages.protected_preview` nullable;
- add nullable `content_purged_at` and required CAS `row_version` to messages;
  and
- add a non-unique lookup index on
  `graphql_orm_ai_attachments.message_id`.

For every existing message, preserve its protected preview, set
`content_purged_at` to null, and initialize `row_version` to zero using the
dependency-generated migration/default. Do not synthesize a tombstone or
delete a block during migration. Validate that each unpurged complete message
still has a protected preview and exactly `block_count` ordered block rows.
There is no consumer-owned application/domain data migration.

After migration, schedule `OrmAiSessionRetentionService` as a trusted host
worker. Begin each scan cycle with no cursor, pass each returned opaque
`next_session_cursor` into the next call, and begin a later cycle only after it
returns absent. Deployment limits bound every scan, event query, message query,
and block deletion. The worker uses generated ORM repositories and
state-machine transactions only; do not replace it with application SQL or
expose it as a user GraphQL operation.

The current exact scope retention policy is reloaded and validated inside each
session transaction. Missing/legacy policy, corrupt rows, CAS conflicts,
nonterminal runs, linked attachments, and block-count mismatch retain content
and fail closed or are reported. A successful pass may:

- delete expired provisional `provider_live_delta` session-event rows;
- clear the protected preview and delete blocks for an expired, finalized,
  terminal-run message with no attachment;
- retain the message shell with `content_purged_at`, `block_count = 0`, and a
  fixed server-authored tombstone in authenticated reads; and
- append one redacted audit fact in the same transaction.

This is not complete erasure. It never deletes a session, message metadata,
attachments or blob objects, runs, tool/proposal/approval payloads, raw
provider payloads, provider-persistent files, usage, egress, audit, fence, or
restore evidence. Continue to treat those retention fields and deleting-
session workflows as separate obligations.

`AiMessageView` gains required GraphQL/Rust field `content_purged` (or
`ContentPurged` under the PascalCase feature). A purged authorized message has
the fixed preview `Content removed by retention policy`, reports zero blocks,
and returns an empty authorized block window. Clients must not infer that the
message metadata or linked external artifacts were erased.

Selective live-delta deletion can leave durable sequence gaps without reusing
or rewinding sequence values. `AiSessionService::session_event_page` now
returns an empty page with `reset_required = true` when the requested replay
window crosses such a gap. Subscription and virtualized clients must discard
provisional state and reload bounded authoritative message/session windows.

Restore fact collectors must populate
`AiRestoreSnapshotFacts::invalid_session_retention_count`. Report nonzero for
inconsistent purged/unpurged message shapes, retained blocks behind a
tombstone, or an event gap that cannot be classified as expected retention.
Any nonzero value adds fatal `AI_RESTORE_SESSION_RETENTION_INVALID` and keeps
runtime readiness closed. Expected, validated retention gaps remain represented
through reset semantics; duplicate sequence values remain independently fatal.

This is a pre-1.0 public Rust API, GraphQL SDL, persistence, migration,
backup/restore, and behavioral contract change.

## Unreleased: immutable pricing catalog (crate/schema 0.18.0 to 0.19.0)

Apply AI schema module `0.19.0` while configuration writes, provider workers,
budget reservations, backups, and restore callbacks are closed. Do not run
0.19.0 code against module 0.18.0. The module adds its 39th private entity,
append-only `graphql_orm_ai_pricing_policies`, with a required globally unique
`version_reference`, exact deterministic scope key and scope fields, exact
provider/model, integer-only fixed/input/cached-input/output rates, creator
principal identity, and creation time. No consumer-owned application/domain
data migration is needed.

Pricing versions are immutable and immediately eligible for explicit
selection. There is no update, delete, activation, or implicit “latest” API.
Create a new version to change a rate, then bind its exact returned reference
into new budget reservations. Existing reservations and uncertain calls retain
their original reference and must never be repriced under a newer version.

Hosts composing `AiConfigurationQueryRoot`/`AiConfigurationMutationRoot` gain
`aiPricingPolicies` and `createAiPricingPolicy` (or coherent PascalCase names).
Install a separate `Arc<dyn AiPricingCatalogService>` in GraphQL context;
existing `AiConfigurationService` implementations do not gain methods. Add
the new exhaustive `AiConfigurationAction::ReadPricingCatalog` and
`ManagePricingCatalog` cases to every host policy. Reads are exact-route and
bounded to 100 versions. Creation requires a user principal with recent MFA,
the exact host write decision, deployment `AiPricingCatalogManagementLimits`,
per-route capacity, and an atomic redacted audit append.

`OrmAiPricingService` also implements `AiPricingQuoteService` and
`AiProviderUsageAccounting`. Quotes bind exact scope, provider, model, and
version and conservatively price all estimated input at the non-cached rate.
Settlement prices authoritative total/cached input and output under the same
version. `AiProviderUsageObservation::scope` exposes the exact application
scope copied from the bound budget plan so custom accountants can enforce the
same cross-scope rejection. Rates and totals use checked integer microunit arithmetic with
per-dimension ceiling division; cached rate cannot exceed ordinary input rate.
Version/provider/model swaps, corrupt rows, negative rates, and overflow fail
closed. The initial concrete accountant rejects provider built-ins because a
requested tool is not authoritative provider-billed usage; deployments using
built-ins must retain a custom complete accounting implementation until exact
billable-unit catalogs land.

Restore fact collectors must populate the new
`AiRestoreSnapshotFacts::invalid_pricing_policy_count`. Validate unique
references, deterministic scope-key equality, exact supported provider/model,
non-negative rates, cached rate no greater than input, creator identity, and
the corresponding creation audit before reporting zero. Any nonzero value adds
fatal `AI_RESTORE_PRICING_POLICY_INVALID` and keeps runtime readiness closed.

This is a pre-1.0 public Rust API, GraphQL SDL, authorization, configuration,
persistence, migration, backup/restore, and behavioral contract change.

## Unreleased: authenticated budget-policy management (crate/schema 0.17.0 to 0.18.0)

Apply AI schema module `0.18.0` while provider workers, configuration writes,
budget reservations, backups, and restore callbacks are closed. Do not run
0.18.0 code against module 0.17.0. The managed schema adds required indexed
`scope_key` to `graphql_orm_ai_budget_policies`; the module still owns 38
private entities.

Backfill every existing budget policy with `ai_scope_key` computed from its
stored `scope_kind`, `scope_id`, and optional `tenant_id`. This is a
deterministic non-secret lookup identity, not authorization. Reject or repair
rows with invalid scopes, unpaired principal kind/subject, unknown intervals,
negative/no ceilings, duplicate/corrupt IDs, or a key that does not exactly
match those stored scope fields. Do not infer a tenant or principal. No
consumer-owned application/domain data migration is needed.
The helper is now exported for every backend, including schema-only MSSQL
builds; its availability does not imply MSSQL write-service parity.

`AiConfigurationAction` gains `ReadBudgetPolicies` and
`ManageBudgetPolicies`. `AiConfigurationService` gains `budget_policies` and
`upsert_budget_policy`; every custom implementation must add both methods.
Composed configuration GraphQL schemas gain `aiBudgetPolicies` and
`upsertAiBudgetPolicy` (or coherent PascalCase names), the
`AiBudgetIntervalInput` enum, input, and redacted view.

The ORM configuration service leaves mutations closed until the host calls
`with_budget_policy_management(AiBudgetPolicyManagementLimits)`. These
deployment bounds cap every GraphQL-configurable token/tool/image/cost/run
ceiling and allow at most 100 policies per exact scope. They do not grant
configuration authority and do not replace the independent per-call
`AiBudgetServiceLimits`. Choose the per-scope management bound together with
the budget service's maximum-applicable-policy bound so exact plus
tenant-wildcard policy sets remain executable.

Reads require the host's exact-scope `ReadBudgetPolicies` decision and return
at most 100 records. Mutations require a user principal with recent MFA, the
host's `ManageBudgetPolicies` decision, validated deployment ceilings, a
create/update identity pairing, and exact CAS. Create accepts an optional exact
principal kind/subject pair. On update the scope, tenant, principal pair, and
interval are immutable; create a replacement and disable the old policy to
change those bindings. There is no delete operation. Each successful mutation
appends a redacted audit event in the same state-machine transaction.

The reservation service now selects policies by the exact deterministic scope
key plus the matching tenant-wildcard key, then verifies the stored key and
scope fields before applying principal filters. Missing, excessive, corrupt,
or ceiling-free effective policies remain fail-closed. Existing counters keep
their committed/reserved values across ceiling changes; new reservations use
the current policy row version and a disabled policy no longer participates in
new reservations.

Restore fact collectors must populate the new
`AiRestoreSnapshotFacts::invalid_budget_policy_count`. Any nonzero value adds
fatal `AI_RESTORE_BUDGET_POLICY_INVALID` and keeps readiness closed. Validate
scope-key integrity, principal pairing, interval, non-negative bounded
ceilings, and policy/counter version relationships before reporting zero.

This is a pre-1.0 public Rust API, GraphQL SDL, authorization, configuration,
persistence, migration, backup/restore, and behavioral contract change.

The crate root no longer glob-reexports macro-generated types from the private
persistence module. These types were an accidental compile-visible leak and
were never a supported application CRUD surface. Replace any use with
`AiSchemaModule` for migrations and the authenticated configuration, budget,
usage, session, run, proposal, approval, attachment, or worker service traits.
`AiSchemaModule`, `AI_SCHEMA_MODULE_ID`, `AI_SCHEMA_MODULE_VERSION`, and
`AI_TABLE_NAMESPACE` remain public.

## Unreleased: authoritative usage ledger and reporting (crate/schema 0.16.0 to 0.17.0)

Apply AI schema module `0.17.0` while provider workers, budget reconciliation,
usage readers, backups, and restore callbacks are closed. Do not run 0.17.0
code against module 0.16.0. The managed migration:

- adds nullable `actual_cached_input_tokens` to
  `graphql_orm_ai_budget_reservations`;
- adds required, unique `budget_reservation_id` and required
  `principal_kind` to append-only `graphql_orm_ai_usage_entries`;
- adds generated query indexes for exact scope kind/ID/tenant, principal
  kind/subject, provider kind/model, run, creation time, and reservation; and
- advances `AI_SCHEMA_MODULE_VERSION` from `0.16.0` to `0.17.0` without adding
  an entity (the module still owns 38 private records).

The 0.16.0 usage entity was reserved private storage with no supported writer
or reader and should be empty. If a deployment wrote private usage rows, do not
invent a reservation, principal kind, tenant, or authority. While the runtime
is closed, a dependency-owned migration must prove each row's exact committed
budget reservation and matching run/session/scope/principal/provider/model,
reject duplicates, validate cached input is no greater than total input, and
then backfill it; otherwise remove those unsupported rows. Never expose an
unproven legacy row through the new service.

Existing committed reservations are not silently converted into historical
usage facts. If historical reporting is required, backfill only from complete
authoritative provider and committed-reservation evidence with a unique
one-to-one reservation binding. An absence remains an explicit historical gap;
estimated values must never be relabeled as actual usage. No consumer-owned
application/domain data migration is needed.

`AiBudgetReconciliation` gains `cached_input_tokens`. A committed result must
supply `Some(value)` and prove it is no greater than total `actual.input_tokens`;
an unused release must supply `None`; an uncertain result may carry an
observation but creates no usage fact. `AiBudgetAmounts::input_tokens` and
`AiProviderUsageObservation::input_tokens()` now explicitly mean total input,
with cached input recorded as a subset rather than added to the total. Update
every struct initializer and pricing implementation accordingly.

On authoritative commit, the ORM budget service now appends exactly one usage
fact in the counter/reservation transaction. Its unique reservation ID is the
idempotency boundary. A replay must match the original actual and cached usage
and returns the prior reconciliation; it never appends another fact. Release
and uncertain outcomes append none.

Hosts composing `AiQueryRoot` gain `aiUsage` (or `AiUsage` under
`graphql-case-pascal`). Install `Arc<dyn AiUsageService>` in GraphQL context.
`OrmAiUsageService` additionally requires an `AiUsageAccessPolicy`; return
`OwnPrincipal` for personal reporting, `WholeScope` only for independently
authorized scope administrators, and `Denied` otherwise. The policy result is
read authority only and grants no provider, budget-management, transcript, or
tool authority. Default pages contain at most 50 rows and the hard maximum is
200. Time filtering requires both bounds, is limited to a 366-day interval,
and uses the current generated GraphQL integer range.

Backups and restores must preserve the usage table as immutable facts and
validate: one usage fact per reservation; referenced reservations are
committed; exact scope/principal/provider/model fields agree; numeric usage is
non-negative; cached input does not exceed total input; and no report opens
until restore reconciliation succeeds. Retention or correction of usage facts
is not introduced by this release. Restore fact collectors must populate the
new `AiRestoreSnapshotFacts::invalid_usage_fact_count`; any nonzero value adds
the fatal `AI_RESTORE_USAGE_FACT_INVALID` issue and keeps readiness closed.

This is a pre-1.0 public Rust API, GraphQL SDL, persistence, migration,
backup/restore, reporting, budget-reconciliation, and behavioral contract
change.

Host egress planners should call the new public
`ModelRequest::conservative_egress_bytes()` when constructing the inference
manifest. It exposes the exact conservative calculation enforced by the
provider context; callers must not reproduce the older input-only estimate.

## Unreleased: bounded complete provider request metadata (crate 0.15.0 to 0.16.0)

`ModelRequest::validate` now rejects oversized instructions, text/JSON blocks,
output schemas, custom-tool schemas/fingerprints, zero or excessively large
output-token ceilings, more than 16 provider built-ins, duplicate built-in
kinds, duplicate/invalid web domains or file-store IDs, and invalid built-in
result limits. Serialized non-attachment request metadata has a 64-MiB hard
aggregate ceiling. Web-domain filters accept normalized DNS names and an optional
leading `*.` only; schemes, paths, whitespace, empty labels, and invalid label
characters are rejected.

Provider egress validation now estimates the complete serialized
`ModelRequest`, including model, instructions, tool definitions, schemas,
built-in configuration, continuation and tool-result metadata, then adds the
exact Base64 expansion of attachment bytes. Existing egress planners must use
the request's current conservative estimate rather than reproducing the older
input-only calculation. A previously accepted manifest whose
`estimated_bytes` omitted tool/schema/built-in metadata now correctly fails
before transport and must be reauthorized with the complete ceiling.

This is a pre-1.0 provider validation, egress, and behavioral contract change.
It adds no Rust type, GraphQL SDL, Cargo feature/default, entity, field, index,
constraint, persistent semantic, or backup/restore change.
`AI_SCHEMA_MODULE_VERSION` remains `0.16.0`; no AI-owned or
application-domain data migration is needed.

## Unreleased: installed local-harness foundation (crate 0.14.0 to 0.15.0)

`ProviderKind` and GraphQL `AiProviderKindInput` now include `LocalHarness`
with stable persistence/configuration value `local_harness`. Exhaustive Rust
matches must add that variant. Composed configuration GraphQL schemas gain the
corresponding enum value (`LOCAL_HARNESS` by default or `LocalHarness` with
`graphql-case-pascal`). A local-harness provider profile accepts no `base_url`:
GraphQL may enable, disable, scope, and route a logical profile, but cannot
create or alter executable, arguments, digest, working directory, sandbox,
environment, network, or resource authority.
Credential set/rotation is rejected for these profiles, and a credentialed
profile cannot be changed to `LocalHarness` until its provider credential is
removed through the ordinary audited mutation.

The opt-in `local-harness` feature exports `AiLocalHarnessRegistration`, its
immutable registry and limits, `AiLocalHarnessProvider`, the bounded
`AiJsonLinesLocalHarnessDriver`, and trusted process launcher/session traits.
Registrations require a normalized absolute executable and working directory,
fixed arguments, lowercase executable SHA-256, reviewed version, sandbox
profile, identical narrow capabilities, and hard protocol/process ceilings.
The initial registration has no environment, credential, mount, network, file,
image, built-in, tool, continuation, reasoning, background, embedding, or code
authority.

The crate does not include a generic unsandboxed child-process launcher. A host
implementation of `AiLocalHarnessProcessLauncher` must atomically verify and
execute the registered image without a shell, clear the complete inherited
environment, enforce the reviewed OS/container profile and denied network,
contain descendants, apply memory/CPU/wall/output limits, and synchronously
initiate process-tree termination on drop. Construction of the registration is
syntactic validation, not proof that those deployment controls were applied.

Every installed harness turn still enters through `AiProviderCallExecutor` as
`ProviderKind::LocalHarness`, with current-principal reauthorization, exact
egress audit, atomic budget reservation, fencing, bounded normalized output,
usage reconciliation, and protected persistence. A logical local destination
does not bypass disclosure or spend policy. The JSON-lines v1 protocol accepts
only response-started, visible-text, bounded-usage, and response-completed
events without response IDs; unsupported process events terminate the session
and fail closed.

This is a pre-1.0 public Rust API, Cargo feature, GraphQL SDL, configuration,
provider, security, and operational contract change. Default features do not
change. It adds no entity, field, index, constraint, persistent semantic, or
backup/restore change. `AI_SCHEMA_MODULE_VERSION` remains `0.16.0`; no
AI-owned or application-domain data migration is needed.

## Unreleased: native Ollama adapter (crate 0.13.0 to 0.14.0)

Enabling `provider-ollama` now compiles the native HTTP adapter and its optional
Base64/Reqwest dependencies. Construct `OllamaProvider` with
`OllamaProviderConfig` and a deployment-owned `AiProviderEndpointPolicy`. The
configured value must be a root `http` or `https` origin without URL
credentials, query, fragment, or path. Redirects are disabled. Endpoint policy
is still responsible for exact host/port allowlisting, DNS rebinding defenses,
and network-zone isolation; the configuration value is not an SSRF proof.

The adapter supports bounded native `/api/chat` NDJSON text streaming,
ephemeral inline PNG/JPEG/WebP inputs, JSON-schema structured output, and
reported prompt/evaluation token usage. Every call still requires a matching
model-inference egress proof and atomic budget proof. Each image additionally
requires its exact image-analysis transfer and freshly reopened attachment
bytes. A local destination does not imply disclosure authorization.

Custom tools, provider built-ins, non-image files, provider-response
continuation, and model thinking output are not supported by this adapter.
They fail closed rather than silently losing conversation state or persisting
hidden reasoning. Native Ollama tool calling remains gated until the runtime
can durably checkpoint and reconstruct a provider-independent stateless
conversation. No API key is required by this adapter; if a deployment places
authentication in front of Ollama, it must use a separately reviewed fixed
transport boundary rather than URL credentials.

This is an additive pre-1.0 public Rust API, feature, dependency, and provider
behavior change. Default features and GraphQL SDL do not change. It adds no
entity, field, index, constraint, persistent semantic, or backup/restore
change. `AI_SCHEMA_MODULE_VERSION` remains `0.16.0`; no AI-owned or
application-domain data migration is needed.

## Unreleased: schema module 0.15.0 to 0.16.0 and principal inbox (crate 0.12.0 to 0.13.0)

Apply AI schema module `0.16.0` while session writes, provider-output commits,
subscriptions, pruning workers, and restore callbacks are closed. Do not start
0.13.0 code against the 0.15.0 module: session creation, message queueing,
archive/restore/delete, and final assistant-output persistence now append a
principal-inbox event in the same state-machine transaction.

The managed schema changes are:

- add private `graphql_orm_ai_inbox_streams`, with deterministic ID, exact
  principal kind/subject, never-rewound `stream_head`,
  `minimum_retained_sequence`, last-event time, row-version fence, and a unique
  principal kind/subject index;
- add a unique principal kind/subject/sequence constraint to
  `graphql_orm_ai_inbox_events`;
- add required captured `scope_key`, `scope_kind`, `scope_id`, and optional
  `tenant_id` to new inbox events; and
- add nullable `scope_key`, `inbox_event_retention_seconds`, and
  `inbox_minimum_events` to retention policies, plus a unique index on
  non-null scope keys.

The inbox-event entity existed as reserved private schema, but 0.12.0 exposed
no writer, query, or subscription for it. A normal deployment should therefore
find it empty. If an early consumer wrote private rows anyway, do not infer
authority or silently assign scopes. A dependency-owned migration must either
prove each row's exact owner/session/scope, backfill captured scope fields,
validate unique contiguous per-principal sequences, and construct the matching
stream head, or remove the unsupported rows while the runtime is closed. No
client cursor existed in the public 0.12.0 GraphQL contract.

Legacy retention rows remain stored but are not effective for inbox pruning
until all three nullable migration fields are populated and valid. Supported
write-backend migration diagnostics may use `ai_scope_key` to reproduce the
stable non-secret scope identity; that value is not authorization. Prefer the
new recent-MFA-protected `setAiRetentionPolicy` mutation to create the current
scope policy. Resolve duplicate logical legacy policies explicitly before
adding/currently relying on the unique keyed policy. Never invent a retention
period or treat absence as permission to delete.

Host `AiConfigurationService` implementations must add `retention_policy` and
`set_retention_policy`. Host `AiConfigurationAccessPolicy` implementations
must handle `ReadRetention` and `ManageRetention`. Compose the corresponding
configuration query/mutation fields if GraphQL management is enabled. The
mutation is CAS-bound, requires current recent MFA in the ORM service, and
audits in the same transaction.

Hosts composing `AiQueryRoot`/`AiSubscriptionRoot` gain
`aiInboxEventPage`/`aiInboxEvents` (or coherent PascalCase names). Install an
explicit `Arc<dyn AiInboxService>`; missing registration fails closed. Schedule
`OrmAiInboxPruningService` only as a trusted host worker after all required
scope policies are current. It deletes only a bounded expired prefix, keeps the
configured recent-event floor, and atomically advances the retained cursor.
Do not expose pruning as an ordinary user mutation or manually renumber rows.

This is a pre-1.0 Rust API, GraphQL SDL, persistence, index/constraint,
authorization, backup/restore, and behavioral contract change. Cargo features
and defaults do not change. Backups must include the new stream entity and
captured inbox scope fields. Restore reconciliation must validate stream
bounds and retained-prefix continuity before reopening. No application-domain
table or data migration is required.

## Unreleased: exact provider attachment reopening (crate 0.11.0 to 0.12.0)

Provider turns containing `ModelInputBlock::Attachment` now require exact
freshly reopened bytes in addition to the attachment egress proof introduced
in 0.10.0. Configure `AiProviderCallExecutor::with_attachment_resolver` with a
trusted `AiProviderAttachmentResolver` and validated
`AiProviderAttachmentResolutionLimits`. SQLite/PostgreSQL hosts can use the
same `OrmAiAttachmentService` that owns intake. A missing resolver fails before
provider transport; do not replace it with a signed URL, raw storage key, or
model-selected object lookup.

The new public `AiProviderAttachmentRequest` and
`AiResolvedProviderAttachment` values bind opaque ID, scanner-detected MIME,
raw byte count, lowercase SHA-256, sanitized filename, and content. The
resolved type validates length/hash but is not authorization proof. Resolver
implementations must use the supplied fresh `ResolvedPrincipal`, recheck the
current session/scope/owner and released/clean/message-linked state, read only
the exact durable object, and fail if either object facts or the row changes.
`ProviderRequestContext::with_resolved_attachments` requires one-to-one exact
coverage; provider adapters retrieve content with `resolved_attachment`.

`ModelRequest::validate` now rejects duplicate attachment IDs. Its estimated
payload includes conservative Base64 expansion, so existing inference and
image/file manifests may need larger `estimated_bytes` values. The exact
capability manifest is still separate and must carry the canonical attachment
source. Deployment limits may only narrow the model/request hard limits.

With `provider-openai`, supported PNG/JPEG/WEBP/GIF inputs are sent as
ephemeral Responses `input_image` data URLs. Host scanning/acceptance must
reject animated GIFs because OpenAI accepts only non-animated GIF input. Other
host-accepted files are sent as inline `input_file` data under the adapter's
less-than-50-MiB
per-file and 50-MiB combined raw-file bounds. This path creates no provider
file ID and therefore no provider-file deletion lifecycle. Hosts remain
responsible for MIME acceptance and must separately authorize `ImageAnalysis`
or `ProviderFile`; provider rejection remains a normal uncertain transport
failure. The feature now enables an optional Base64 dependency; default
features are unchanged.

This pre-1.0 release adds public Rust APIs and changes provider behavior and
payload estimates. It adds no GraphQL SDL, entity, field, index, constraint,
backup/restore, or persistent semantic change. `AI_SCHEMA_MODULE_VERSION`
remains `0.15.0`; no AI-owned or application-domain data migration is needed.

## Unreleased: schema module 0.14.0 to 0.15.0 and attachment cleanup (crate 0.10.0 to 0.11.0)

Apply AI schema module `0.15.0` before starting the new worker. The existing
`graphql_orm_ai_attachments` table gains nullable `processing_expires_at`,
`cleanup_generation`, `cleanup_lease_expires_at`, `cleanup_retry_count`, and
`cleanup_next_attempt_at` columns. Lifecycle state fields become private
maintenance filters. No entity is added or removed; blob references retain
their backup-redaction contract.

Hosts should schedule `AiAttachmentCleanupService::cleanup_once` through a
trusted singleton or distributed worker scheduler. The service itself permits
safe concurrency: every row gets a monotonic generation, expiring CAS claim,
and redacted audit outcome. Do not expose it through GraphQL, delete storage
prefixes, or clear blob references manually. Configure upload-processing time
longer than maximum upload plus full-object scanner latency. Storage ambiguity
enters capped retry backoff rather than being reported as deletion.

`AiAttachmentServiceLimits::new` remains source-compatible and defaults the new
processing lifetime to one hour; `with_upload_processing_ttl` can narrow or
widen it within the documented hard bound. `OrmAiAttachmentService` adds
`with_cleanup_limits`. Public `AiAttachmentCleanupLimits`,
`AiAttachmentCleanupReport`, and `AiAttachmentCleanupService` are new additive
Rust APIs. GraphQL SDL, Cargo features/defaults, and application authorization
contracts do not change.

Existing pending tickets need no rewrite. Legacy interrupted `uploading` rows
with no processing deadline fall back to their ticket expiry; legacy
`deleting` rows with no deadline are reclaimable. After restore, keep the
runtime start gate closed until the module migration and normal restore
reconciliation have completed, then run cleanup. This is an AI-owned metadata
migration only; there is no application-domain data migration.

## Unreleased: exact attachment egress binding (crate 0.9.0 to 0.10.0)

Every `ModelInputBlock::Attachment` constructor must now supply the exact
verified `byte_count` and lowercase `sha256` from the released attachment. Its
separate `ImageAnalysis` or `ProviderFile` egress manifest must include a
source with `kind: "attachment"`, `trust: UserProvided`, and canonical
`reference` returned by `ModelInputBlock::attachment_egress_reference`. The
versioned value binds ID, byte count, detected MIME, and SHA-256. The manifest
byte/count limits must cover the full request including attachment bytes.
Changed content or metadata requires a new manifest decision and audit; never
copy a proof between attachments.

This is a pre-1.0 breaking Rust API and provider behavior change. It adds no
GraphQL SDL, Cargo feature/default, entity, field, index, constraint,
backup/restore, or data semantic change. `AI_SCHEMA_MODULE_VERSION` remains
`0.14.0`; no AI or application-domain data migration is required.

## Unreleased: schema module 0.13.0 to 0.14.0 and attachment intake (crate 0.8.0 to 0.9.0)

This pre-1.0 release adds the owner-isolated attachment service and composable
attachment GraphQL roots. Add the exact pinned `graphql-orm-storage` 0.5.0
dependency universe from this manifest. Construct `OrmAiAttachmentService`
with the ordinary session/scope access policy, content-protection boundaries,
a provider-neutral `BlobStore`, a complete-object `AiAttachmentScanner`, a
separate fail-closed `AiAttachmentAcceptancePolicy`, and a trusted clock.

The GraphQL `createAiAttachmentUpload` mutation returns a one-time token. A
host-owned authenticated streaming endpoint passes that token as
`SecretString` plus `StorageByteStream` to `AiAttachmentUploadService::upload`;
do not put it in a URL, log, database, or GraphQL file body. The current owner
must still authenticate. After clean scanning and policy acceptance, call
`finalizeAiAttachmentUpload`; only released/clean attachment IDs can enter the
ordinary `sendAiMessage` mutation. Existing applications must compose the new
roots explicitly; no fields are silently added.

Apply `AiSchemaModule` `0.14.0` while provider starts, uploads, subscriptions,
and restore callbacks are closed. `graphql_orm_ai_attachments` changes as
follows:

- `blob_reference`, `detected_mime`, `byte_count`, and `sha256` become nullable
  so a durable pending ticket never invents final-object facts;
- nullable `quarantine_blob_reference` keeps cleanup work addressable without
  exposing or overloading the final object reference;
- nullable `expected_byte_count`, `upload_token_hash`, and
  `upload_expires_at` bind new uploads without making legacy finalized rows
  invalid; and
- nullable scanner version, acceptance-policy version, and redacted rejection
  code record lifecycle evidence.

For existing finalized rows, no content rewrite is required. Optionally
backfill `expected_byte_count` from `byte_count`; leave upload token/expiry null.
Do not fabricate a token or move a legacy final object into quarantine. Any
legacy row whose object, checksum, owner, session, clean scan, or release state
cannot be verified must remain unavailable and be reported by restore
reconciliation. The managed migration must change column nullability/add the
new columns and record module `0.14.0`; never relabel an applied `0.13.0`
module.

This changes public Rust APIs and GraphQL SDL, but no Cargo feature/default.
Provider adapter file/image resolution, provider-side file retention/deletion,
derivative artifacts, expired-ticket/orphan pruning, scope quota configuration,
and bulk session purge remain explicit gates. No application-domain data
migration is required.

## Unreleased: schema module 0.12.0 to 0.13.0 and protected live output (crate 0.7.0 to 0.8.0)

This pre-1.0 release adds an optional durable provisional-output boundary to
`AiProviderCallExecutor`. Existing construction is unchanged and emits no
provisional events. To enable it, construct `OrmAiLiveDeltaService` with the
same run service, runtime, trusted clock, and validated protection/freshness
limits, then pass it to `with_live_delta_sink` together with validated
coalescing limits.

The new public `AiLiveDeltaSink` receives only bounded visible text or visible
reasoning-summary batches plus an immutable private-field context. A conforming
sink must rehydrate current authority, protect content, and validate the exact
session/run/attempt/generation/provider/model/budget binding. The built-in sink
does this for every batch, rechecks policy after protection, and commits a
protected `provider_live_delta` session event before the ordinary commit-only
subscription wakeup. Sink failure occurs after provider transport and therefore
leaves usage uncertain; do not automatically replay the provider call.

Clients must treat these events as provisional progress. The authoritative
`assistant_message_completed` event and windowed message blocks remain the
final transcript. A provisional event from an attempt later classified
`RecoveryRequired` remains partial history and must not be presented as a
completed assistant answer. Event payload format version 1 binds the run,
attempt, generation, provider/model/optional response, budget reservation,
batch sequence, visible kind, text, and byte count.

Advance `AiSchemaModule` to `0.13.0` through the managed `graphql-orm` schema
manager while provider starts and subscriptions remain closed. No entity,
field, index, constraint, public GraphQL root, client SDL, Cargo feature, or
default changes. No existing AI row or application-domain data rewrite is
required. The managed migration may be structurally empty, but the module bump
is mandatory because `graphql_orm_ai_session_events` gains the persistent
semantic contract for `provider_live_delta`; never relabel an applied `0.12.0`
module. Restore reconciliation must validate module `0.13.0` before reopening.

This slice does not add delta retention/purge. Existing retention policy fields
remain configuration only until the bounded pruning worker lands; do not delete
event rows manually or break session cursor monotonicity.

## Unreleased: schema module 0.11.0 to 0.12.0 and exact tool-batch adoption (crate 0.6.0 to 0.7.0)

This pre-1.0 release makes the read-only coordinator constructor deliberately
breaking: `AiReadOnlyAgentCoordinator::new` now requires an
`AiAgentCheckpointAdopter` in addition to `AiAgentCheckpointWriter`. Use the
same `OrmAiCoordinatorCheckpointService` value for both boundaries unless a
conforming wrapper preserves its current-principal, protection, durable-record,
and one-shot consumption checks.

Expired `Running` attempts are now requeued only when their linked checkpoint
is an exact `tool_batch_persisted` record with a valid protected-envelope hash,
committed/reconciled provider budget, and complete fenced tool/step rows. The
replacement claim retains that one checkpoint ID. Before planning or transport,
the adopter:

- rehydrates the current principal and rechecks session/scope access;
- reopens the protected checkpoint, tool arguments, and tool results under an
  unchanged ready protection policy;
- validates the original attempt/generation, provider response, budget,
  ordered call IDs, descriptor fingerprints, canonical arguments, disclosure
  outputs, egress manifests and immutable allow audits;
- reconstructs the loop counters and exact opaque continuation under the new
  fence; and
- atomically clears the linked checkpoint before the next provider call.

If a worker dies before consumption, the exact checkpoint can be considered by
bounded recovery again. If it dies after consumption or while the next provider
call may have started, ordinary conservative recovery applies. Provider-turn
checkpoints, partial tool batches, supervised mutations, exhausted adoption
retries, and malformed or missing records are never adopted automatically.
`AiRunRecoveryReport` adds the public `checkpoint_requeued` counter; update
exhaustive struct construction and report handling.

Advance `AiSchemaModule` to `0.12.0` through the managed `graphql-orm` schema
manager while workers and provider starts remain closed. There is no physical
entity, field, index, or constraint change, but the module version must record
the new persistent meaning of `latest_checkpoint_id`, the
`checkpoint_adoption_ready` retry marker, and one-shot checkpoint consumption.
The managed migration may therefore be structurally empty; do not skip its
module-version/readiness record or relabel an applied `0.11.0` module.

This adds no GraphQL root/SDL or Cargo feature/default. Existing completed
history needs no rewrite, and existing active provider-turn/partial/malformed
checkpoints remain closed. No AI row rewrite or application-domain data
migration is required. Restore reconciliation must validate module `0.12.0`
before reopening workers.

## Unreleased: schema module 0.10.0 to 0.11.0

Apply `AiSchemaModule` through the managed `graphql-orm` schema manager with
workers, provider starts, subscriptions, and restore callbacks closed. This
revision adds nullable private `protected_state` to append-only
`graphql_orm_ai_run_checkpoints`. It stores exact protected normalized provider
turns and completed model-visible read-only tool batches. Final assistant-output
checkpoints continue to prove their content through message/block rows and keep
this field null.

The coordinator now requires an `AiAgentCheckpointWriter`. Install
`OrmAiCoordinatorCheckpointService` with the same run service, current-principal
resolver, access policy, content-protection resolver/protector, trusted clock,
and deployment byte/freshness limits. Provider turns are checkpointed only
after authoritative budget reconciliation; tool-batch checkpoints additionally
verify every protected result, egress decision/manifest hash, run step, provider
response, and fence in the same transaction. A checkpoint failure after
external execution becomes `RecoveryRequired`.

This schema revision alone does not permit cross-generation adoption. Adoption
is supplied by the later `0.7.0` runtime contract above and is restricted to
exact complete read-only tool batches. Existing active pre-`0.11.0` runs and
malformed/absent checkpoints must continue through privileged recovery; never
infer provider output or tool results, replay a provider call, or backfill
protected state manually. Historical completed runs and final-output
checkpoints need no rewrite. No application-domain data migration is required.

This adds no public GraphQL root or client-visible SDL and changes no Cargo
feature/default. The AI schema migration is nullable/additive, but the runtime
gate must remain closed until managed validation and restore reconciliation
report module `0.11.0` ready. The new public service/trait and constructor
change advance the pre-1.0 crate from `0.5.0` to `0.6.0`; update the reviewed
Git revision and package expectation together.

## Unreleased: remote authenticated GraphQL execution (0.4.0 to 0.5.0)

This pre-1.0 Rust API boundary adds the project-agnostic private remote
execution adapter and deliberately changes
`GraphqlRequestContextFactory::build` to receive `&ToolGraphqlRequest` instead
of `&GraphqlInvocationContext`. Update every factory implementation to accept
the complete request. Local factories may continue to construct the same
ordinary application context; remote factories should use the additional
operation and variable bindings rather than discard them.

For private routed or direct targets, construct
`AiRemoteAuthenticatedGraphqlAdapter` and use the same cloned adapter value as
both `GraphqlRequestContextFactory` and `AuthenticatedGraphqlExecutor`. Supply:

- an `AiRemoteGraphqlAuthorityIssuer` that mints one audience/resource/
  operation-bound, short-lived credential while preserving the human actor;
- an `AiRemoteGraphqlTransport` that maps only deployment-registered logical
  target IDs to fixed private allowlisted destinations and propagates the
  correlation/causation audit chain; and
- validated authority-lifetime and freshly resolved principal-age limits.

Do not pass or persist the user's bearer token. Do not serialize, log, retain,
or reuse `AiRemoteGraphqlAuthority`. A direct-service transport must never
grant more authority than the equivalent routed request. The issuer and
transport remain trusted deployment boundaries: the crate binds and verifies
the redacted request but cannot inspect proprietary delegated-token claims or
prove private network configuration.

This change adds no GraphQL root or client-visible SDL, changes no Cargo
feature/default, and changes no persistent entity, index, constraint, backup,
restore, or authorization-policy data. `AI_SCHEMA_MODULE_VERSION` remains
`0.10.0`; no AI or application data migration is required. Update the package
expectation and reviewed Git revision together.

## Unreleased: schema module 0.9.0 to 0.10.0

Apply `AiSchemaModule` through the managed `graphql-orm` schema manager while
provider starts, workers, subscriptions, and approval callbacks remain closed.
This revision adds nullable restart/audit bindings to
`graphql_orm_ai_tool_calls`:

- `provider_kind`, `provider_model`, and `provider_response_id`;
- `budget_reservation_id`;
- `correlation_id` and `causation_id`; and
- `delegation_reference`.

New tool calls always populate the applicable fields. Supervised execution
requires provider/model, budget, correlation, and causation bindings and proves
that the referenced budget reservation is committed, reconciled, and matches
the exact session/run/attempt/fencing generation/provider/model before
consuming approval. Historical completed rows need no rewrite. A pending or
approved consequential row created before module `0.10.0` lacks authoritative
restart bindings and must fail closed for privileged reconciliation; do not
invent values or update private tables manually.

No application-domain data migration is required. The AI schema migration is
nullable/additive, but the runtime start gate must remain closed until managed
schema validation and restore reconciliation report module `0.10.0` ready.

### Rust API and behavior changes

- Use `AiProviderCallPlan::new_with_supervised_tools` and
  `new_supervised_continuation_with_tools` only when the deployment/scope
  policy explicitly enables exact supervised descriptors. The read-only plan
  constructors remain restricted to read-only queries.
- Implement `AiCanonicalActionPreviewBuilder` with trusted current application
  state. Returned resource versions and preview content are approval authority;
  model-written prose must never be used.
- Construct `OrmAiConsequentialToolCallService`, call `request_approval`, let a
  human decide through the existing approval GraphQL lifecycle, then call
  `execute_approved` with the exact waiting lease and current result-egress
  route. Replace the lease only when the returned persisted outcome contains a
  renewed one.
- `AiRuntime::execute_tool` now rejects descriptors whose approval rule is not
  `None`. Direct callers that previously passed a one-shot descriptor must use
  the supervised lifecycle; there is no compatibility bypass.
- `AiToolPreauthorization` proves only a fresh host tool-policy decision.
  `execute_approved_tool` recomputes and compares that policy version/state
  before resolver invocation; ordinary resolver authorization remains final.
- A post-consumption resolver or handoff ambiguity returns
  `AiConsequentialToolCallOutcome::RecoveryRequired` and terminally closes the
  run. Never retry that mutation or reuse its consumed approval.

This adds no public GraphQL field or root and changes no client SDL. Approval
query/mutation roots are unchanged. The new public APIs and deliberately
stricter runtime behavior advance the pre-1.0 crate from `0.3.0` to `0.4.0`;
consumers must update the package expectation and reviewed Git revision
together.

## Unreleased: schema module 0.8.0 to 0.9.0

This public/security/persistence slice advances the pre-1.0 crate version from
`0.2.0` to `0.3.0`. Update the Git revision and package expectation together;
the intentional initial-turn constructor restriction and new recovery-report
field are a pre-1.0 breaking API/behavior boundary.

Apply `AiSchemaModule` through the managed `graphql-orm` schema manager while
provider starts, workers, subscriptions, and callbacks remain closed. This
revision adds:

- nullable `graphql_orm_ai_runs.latest_checkpoint_id`; and
- append-only `graphql_orm_ai_run_checkpoints`, bound to the exact run,
  attempt, fencing generation, provider response reference, settled budget
  reservation, final assistant message, and a stable redacted checkpoint hash.

The protected assistant message/blocks, session event, renewed run fence, and
`assistant_output_persisted` checkpoint now commit in one state-machine
transaction. If the worker dies after that transaction but before terminal run
finalization, expired-lease reconciliation verifies the complete checkpoint,
hash, attempt/generation, and finalized assistant message before committing
`Completed`. Missing, swapped, malformed, or any other active checkpoint still
fails closed or becomes `RecoveryRequired`; it is never replay authority.

The module version is `0.9.0`; never apply these semantics under an earlier
version. Existing completed/history rows require no rewrite and may keep a null
checkpoint reference. Reconcile any existing active pre-release run through
the prior owning service before reopening workers. Do not invent checkpoint
rows or update the private tables with manual SQL. No application-domain data
migration is required. Keep the runtime start gate closed until managed schema
validation and restore reconciliation report module `0.9.0` ready.

### Rust API and behavior changes

- `AiReadOnlyAgentCoordinator` now owns the bounded top-level read-only loop.
  Hosts implement `AiReadOnlyAgentTurnPlanner` and supply proof-preserving run,
  provider-turn, tool, and output services. Replace the lease after every
  fenced operation and configure a heartbeat interval comfortably shorter than
  the run-service lease TTL.
- Provider/tool/output ambiguity is durably classified as
  `AiReadOnlyAgentRunOutcome::RecoveryRequired` when the current fence can
  commit it. A lost heartbeat fence returns an error without attempting a
  terminal write.
- `AiProviderCallPlan::new_with_tools` is now for initial turns only. Code that
  previously supplied `ModelContinuation` or `ModelInputBlock::ToolResult`
  directly must retain the exact `AiAgentContinuation` and call
  `new_continuation_with_tools`.
- `AiRunRecoveryReport` has a new public `completed` counter.
- In that release, `AiLiveDeltaCoalescer` and related public types provided
  synchronous bounded batching only. A raw batch remains neither authorization
  nor a durability proof. The later crate `0.8.0` contract at the top of this
  guide supplies the optional protected ORM sink.

This adds no public GraphQL root or client-visible SDL. The new generated ORM
records remain private implementation entities.

## Unreleased: multi-repository ownership workflow

Development now assigns one owning agent and isolated branch/worktree to each
repository. The `graphql-orm-ai` agent may inspect `agql-auth` and
`graphql-orm` read-only but sends requested changes to their owners instead of
mutating sibling worktrees. Upstream crates merge first and report final commit
SHAs; this crate then repins and verifies the reviewed dependency universe.

This is a contributor workflow change only. It changes no consumer Rust API,
GraphQL SDL, feature/default, configuration, authorization behavior, schema
module, backup/restore contract, or persisted data. No consumer or data
migration is required.

## Unreleased: documentation and release enforcement

CI and the documented release gate now deny missing public Rust documentation
in addition to ordinary Rustdoc warnings. The release-policy check also
requires `README.md`, `CHANGELOG.md`, and `MIGRATION.md` to move together with
public Rust/runtime changes. Contributors must add useful Rustdoc for every new
public item, including `# Errors` sections for fallible APIs and explicit proof
boundaries for security-sensitive types. This changes no consumer Rust API,
GraphQL SDL, feature/default, runtime behavior, schema module, or persisted
data; no consumer or data migration is required.

## Unreleased: upstream dependency alignment

The public manifest now resolves one exact Git dependency universe:

- `graphql-orm` 0.9.0 at
  `f996cdbe2ef1867dea029ec3ff16e051dbe7566e`; and
- `agql-auth` 0.10.0 at the peeled `v0.10.0` target
  `c92dcb441237bbe308499b26525945f60ffa394a`.

Remove host patches or path overrides to older sibling versions. Hosts that
also depend directly on either crate must use these exact revisions so Cargo
resolves one source/type universe. This changes no `graphql-orm-ai` GraphQL
SDL, AI schema-module version, persisted AI data, or application authorization
policy; no AI data migration is required.

`agql-auth` 0.10.0 separately adds a nullable `authorization_policy` field to
OAuth state storage. Hosts using its OIDC authorization-state persistence must
apply the auth crate's 0.10.0 migration: legacy absence remains an ordinary
login, while a flow requiring a bound policy fails closed when that binding is
absent. Hosts that do not use that OIDC storage need no auth data migration.
`graphql-orm` 0.9.0 adds retention metadata to its public schema, backup,
runtime, and migration models. Hosts constructing those ORM metadata types
manually must follow the upstream 0.9.0 migration notes. This AI crate uses the
derive-generated metadata and opts only run checkpoints into purge.

## Unreleased: schema module 0.7.0 to 0.8.0

Apply `AiSchemaModule` with provider starts and workers closed. This revision
adds the persistent semantics required by authenticated proposal and approval
lifecycles:

- `graphql_orm_ai_proposals.item_count` stores the schema-validated logical
  review-item count;
- proposal, proposal-item, and approval IDs are assigned by the owning service
  before content protection so envelope associated data binds the real row ID;
- proposal and approval tables expose stable dependency-owned keyset metadata;
  and
- approval `session_id` is an internal filterable field for bounded,
  scope-authorized repository windows (it is not exposed as generic CRUD).

The module version is `0.8.0`; do not apply these semantics under an earlier
module version. Existing pre-release proposal rows have no authoritative item
count, and existing protected proposal/approval envelopes may not have been
bound to a service-assigned row ID. Reconcile and remove unfinished rows through
their owning pre-release service, or recreate a disposable environment. Never
invent item counts, decrypt/rewrite envelopes with manual SQL, or treat a prior
pending/approved row as consumable. Completed chat history and provider usage
need no data rewrite.

Keep the runtime gate closed until managed migration validation and restore
reconciliation report module `0.8.0` ready.

### Rust, GraphQL, and behavior changes

- `OrmAiProposalService::persist_validated` stages a catalog-validated proposal
  through the current `AiRunLease` and returns a renewed lease. The previous
  lease is stale.
- `AiProposalService` provides bounded reads and CAS review. `AcceptEdited`
  requires a replacement payload and item count and revalidates both against
  the current exact registered schema version.
- `AiProposalOutcomeRecorder::record_applied_outcome` now takes an
  `&AuthPrincipal`. Call it only after the ordinary application mutation has
  committed; the service rehydrates/authorizes and links the authoritative
  application audit reference without performing the mutation.
- `OrmAiApprovalService::request_approval` requires a server-generated
  canonical preview and complete binding, then atomically parks the exact run
  and tool call in `WaitingApproval`. Its constructor now also requires the
  current `AiToolCatalog`; request and consumption reject catalog/descriptor/
  GraphQL-contract drift.
- `decide_approval` and `revoke_approval` are CAS-bound authenticated GraphQL
  operations. Recent MFA is enforced when the durable request requires it.
- `consume_approval` requires the current waiting lease plus a freshly rebuilt
  binding/preview, rehydrates the original actor, atomically consumes exactly
  once, and returns a renewed `Running` lease and `ConsumedAiApproval`. That
  proof does not replace the fresh resolver authorization/resource-version
  check that must immediately follow.
- Hosts may compose `AiProposalQueryRoot`, `AiProposalMutationRoot`,
  `AiApprovalQueryRoot`, and `AiApprovalMutationRoot`. This adds public GraphQL
  SDL when those roots are composed; regenerate affected client documents.
  Default names are camelCase and the `graphql-case-pascal` feature changes the
  entire new contract coherently without aliases.
- `AiProposalCatalog::descriptor` is new, and registration now rejects zero or
  excessive payload/source/item limits.

No application-domain data migration is required. No proposal review or
approval decision grants domain write authority by itself.

## Unreleased: schema module 0.6.0 to 0.7.0

Apply `AiSchemaModule` through the managed `graphql-orm` schema manager with
workers and provider starts closed. This revision extends
`graphql_orm_ai_tool_calls` with:

- a unique run/provider-call key plus the opaque provider call ID;
- provider-turn and within-turn ordering;
- current authorization policy and authorization-state bindings;
- static disclosure fingerprint and result classification;
- exact result-egress decision ID and manifest hash; and
- the ordinary application audit reference.

The module version is `0.7.0`; never apply these persistent semantics under a
previous module version. Existing pre-release active tool-call rows cannot
safely infer provider call identity, current authorization, disclosure, or
egress proofs. Stop/reconcile them through their owning pre-release service and
classify ambiguity as recovery-required. Recreate a disposable environment if
that service path is unavailable. Do not backfill invented values or use manual
SQL. Conversational messages and completed provider usage need no data rewrite.

Keep the runtime gate closed until managed schema validation and restore
reconciliation report module `0.7.0` ready. This change adds no public GraphQL
root or SDL field, so GraphQL clients need no document regeneration.

### Rust API and behavior changes

- `ModelRequest` requires a `continuation: Option<ModelContinuation>` field.
  Existing request literals should set it to `None`.
- `ModelInputBlock` adds `ToolResult`. A request containing tool results must
  carry an exact previous-response continuation, and a continuation without a
  tool result is rejected by this initial contract.
- `AiProviderCallLimits::with_maximum_tool_calls` sets a per-turn hard limit.
- `AiProviderCallPlan::new` still rejects custom tools. Use `new_with_tools`
  only with an exact current `AiToolPolicySet`; it accepts registered,
  fingerprint-matching, explicitly enabled, idempotent read-only application
  queries with no approval requirement. Use `new_continuation_with_tools` for
  subsequent turns so result blocks and exact manifests cannot be swapped.
- `AiProviderCallResult::tool_calls` exposes normalized unforgeable call
  requests, and `continuation` returns the exact prior-response identity.
- `OrmAiApplicationToolCallService` and `AiApplicationToolCallLimits` own
  protected/fenced read-only resolver execution and result egress. Replace the
  lease after every returned outcome.
- `AiAgentLoopGuard` enforces provider-turn/tool-call bounds and exact call/
  result/continuation ordering. Reconstructing a guard is not a recovery
  mechanism; uncertain work remains closed for restore/operator review.
- `OrmAiProviderOutputService::persist` now rejects results with pending custom
  tool calls.
- OpenAI stateful continuation requires `store_responses = true` and every
  exact transfer manifest must use
  `AI_EGRESS_RETENTION_PROVIDER_RESPONSE` (`provider_response`). The default
  remains false. No provider retention is silently enabled by migration.

The public API additions and `ModelRequest` field are deliberate pre-1.0
changes within the unreleased `0.2.0` line. This `0.7.0` slice did not change
approval semantics; the later `0.8.0` section defines canonical-preview and
one-shot approval persistence. Consequential execution remains unavailable
until the separately gated executor performs fresh resolver authorization.

## Unreleased: schema module 0.5.0 to 0.6.0

The crate version moves from `0.1.0` to `0.2.0` because this revision changes
public pre-1.0 Rust contracts as well as persistence.

Apply `AiSchemaModule` through the managed `graphql-orm` schema manager. This
revision changes budget persistence and must not reuse a previously applied
`0.5.0` module version.

Budget policy rows gain optional principal-kind matching and tool/image unit
ceilings. Budget counter rows gain:

- a stable `period_key` and unique `(budget_policy_id, period_key)` boundary;
- reserved and committed tool/image units; and
- an upsert identity for safe concurrent counter creation.

Budget reservation rows gain principal-kind binding, unique
`(principal_kind, principal_subject, idempotency_key)` enforcement, and an
`actual_runs` field so every reserved dimension reconciles completely.

Run-attempt completion/retry/recovery is now stored in the new append-only
`graphql_orm_ai_run_attempt_outcomes` table, uniquely keyed by `attempt_id`.
The existing append-only claim row remains immutable. Egress event IDs are now
caller-supplied exact `AiEgressDecisionId` values rather than unrelated
generated UUIDs.

The legacy optional completion columns on the pre-release attempt-claim table
remain physically present for non-destructive migration compatibility, but new
workers leave them null. The separate outcome table is the source of truth;
do not disable append-only enforcement to populate the legacy columns.

The `0.5.0` pre-release counter and reservation shapes cannot safely infer
principal kind or a stable interval key. Before applying this migration in an
early test deployment, stop workers and provider starts, classify every
in-flight call, preserve required audit/usage facts, and remove the old active
counter/reservation rows through the owning pre-release service. Do not invent
bindings or run manual SQL. Recreate disposable environments when that safe
service path is unavailable. Keep the runtime start gate closed until schema
validation and reconciliation report module `0.6.0` ready.

### Rust API and behavior changes

- `AiBudgetService::reconcile` returns `AiBudgetReconciliationResult` with
  committed, released, and still-held amounts.
- `AiError::BudgetDenied` reports missing/exhausted applicable capacity.
- `AiBudgetReservation` and `AiBudgetReconciliationResult` are no longer Serde
  deserializable. Obtain reservations from `AiBudgetService`; do not reconstruct
  proof-bearing values from persisted or client-controlled JSON.
- `OrmAiBudgetService` requires validated deployment-owned
  `AiBudgetServiceLimits` and a trusted `agql-auth::Clock`.
- A request now requires exactly one run unit, a fresh `ResolvedPrincipal`, a
  current running lease/attempt/fencing generation, an active persisted
  session with matching owner/tenant/exact scope, an expiry no later than the
  lease, and at least one applicable policy.
- Ordinary reconciliation may move an uncertain reservation to committed when
  authoritative actual usage arrives, but cannot optimistically release it.
- Orchestration must persist `MarkUncertain` immediately before handing the
  authorized reservation proof to provider transport. `ReleaseUnused` is only
  available while the durable reservation is still `Reserved`.
- `OrmAiRunService` and `AiRunServiceLimits` now own queue claims, heartbeats,
  retries, completion, and expired-lease recovery. Callers must replace the
  lease value after every successful fenced write; an older row-version proof
  is deliberately rejected.
- `AiProviderCallExecutor` requires a durable `AiEgressDecisionAudit`, budget
  service, `AiProviderUsageAccounting`, trusted clock, and bounded
  `AiProviderCallLimits`. Accounting implementations must resolve the exact
  `pricing_policy_version`, return provider-observed input/output tokens and
  one run exactly, and authoritatively compute cost/tool/image units. It
  supports one security-ordered provider turn. Tool-free construction remains
  the default; the later `0.7.0` section documents the separately gated
  read-only application-tool path.
- A successful provider result must be passed to
  `OrmAiProviderOutputService::persist` before terminal completion. Use the
  renewed lease returned by that service for `OrmAiRunService::finish`.
- `AiProviderCallResult` is bound to session/run/attempt/generation/provider/
  model and is not a transferable authorization proof.

This release adds no GraphQL budget configuration fields yet. Existing host
GraphQL SDL is unchanged, so no client document regeneration is needed for
this worker/provider slice. Hosts that previously implemented the budget trait
must update the reconciliation return type and preserve the same atomic,
fenced, idempotent semantics. Existing pre-release run claims have no outcome
fact to backfill unless an authoritative final state is known; never infer a
provider outcome. Keep ambiguous active work closed for recovery review.

## Unreleased: schema module 0.4.0 to 0.5.0

Apply the dependency-owned `AiSchemaModule` through the normal
`graphql-orm` schema manager using a new managed migration version. Do not copy
SQL or create AI tables manually.

The additive schema change creates:

- `graphql_orm_ai_budget_counters`
- `graphql_orm_ai_budget_reservations`

It also adds exact target/schema/document/projection/disclosure, principal/
delegation, resource precondition, policy/auth-state, canonical preview, and
one-shot consumption columns to `graphql_orm_ai_approvals`.

No existing conversational content needs rewriting. Existing pre-release
approval rows cannot safely manufacture the new bindings: expire/revoke them
during restore/startup reconciliation and require a fresh approval. Existing
unaccounted provider work must complete or be classified as uncertain before
enabling hard budgets.

Back up a disposable environment before rehearsing the migration. Runtime
workers, subscriptions, webhooks, and schedules remain closed until managed
schema validation and restore reconciliation report module `0.5.0` ready.

### Rust API changes

- `ProviderRequestContext::new` requires an `AuthorizedBudgetReservation`.
- `AiRuntimeBuilder` requires `graphql_targets(...)`.
- `GraphqlRequestContextFactory::build` receives the validated
  `GraphqlExecutionTarget`.
- `ToolGraphqlRequest` carries an exact `GraphqlOperationContract`.
- `GraphqlInvocationContext` carries explicit causation and optional safe
  delegation references plus the exact application scope.
- Application GraphQL tools use
  `AiToolCatalog::register_with_disclosure`; `register` is reserved for
  internal proposal-staging tools.
- `AiRuntimeBuilder` requires `tool_authorization_policy(...)` so current
  principal/scope/descriptor/arguments are authorized on every call.
- `AiRuntime::execute_tool` requires the registered `AiToolId` and returns an
  `AiToolExecutionResult` after argument, output-limit, and disclosure checks.
- Tool argument schemas must explicitly declare JSON Schema 2020-12.

These are deliberate pre-1.0 breaking changes. Update host construction and
mock fixtures together; do not create permissive placeholder targets,
disclosure schemas, or budget grants.

### Provider error classification

The OpenAI adapter now maps HTTP 401 to `ProviderError::CredentialUnavailable`
instead of `ProviderError::Rejected`. Hosts matching public error categories
should handle the credential category as a redacted configuration/rotation
failure. No data migration is required.

### GraphQL naming

The default SDL remains async-graphql camelCase. Hosts requiring PascalCase
enable:

```toml
graphql-orm-ai = {
  version = "0.1.0",
  features = ["sqlite", "graphql-case-pascal"]
}
```

This changes resolver, argument, input, output, subscription, and generated ORM
field names as one compile-time schema contract. There are no lowercase aliases.
Regenerate client documents and compare SDL before rollout. No database
migration is caused solely by the naming feature.

## Initial adoption

New deployments compose `AiSchemaModule`, apply its managed schema, configure
content protection and immutable deployment boundaries, and keep the runtime
start gate closed until readiness succeeds. PostgreSQL/MSSQL rehearsal must use
a disposable Docker-owned database; never point migration commands at a live
machine database.
