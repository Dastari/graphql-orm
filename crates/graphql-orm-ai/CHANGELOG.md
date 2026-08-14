---
title: "Changelog"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-14
review_by: 2027-02-01
supersedes: []
---

# Changelog

All notable user-visible changes are recorded here. The crate follows
Semantic Versioning and keeps migration instructions in [MIGRATION.md](MIGRATION.md).

Historical development entries below retain their original dependency and
checkpoint facts. For the current workspace baseline and active gates, use the
[implementation status](docs/implementation-status.md) and the central
[AI production-readiness plan](../../docs/plans/active/ai-production-readiness/README.md).

## [0.78.4] - 2026-08-14

### Fixed

- Codex app-server projection now admits an omitted object `required` keyword
  as the JSON Schema empty set and emits `"required": []` for otherwise closed
  objects. Canonical generated query relationship-selection and empty scalar
  field/relationship objects therefore survive `start_persistent_empty_thread`
  without changing the registered argument schema or its validation.

### Security

- Projection still rejects non-array, duplicate, or unknown `required` names,
  unknown keywords, unbounded `additionalProperties`, recursive/generic schema
  passthrough, mutations, and subscriptions. The projected representation is
  bound into the existing Codex schema-projection fingerprint. Previously
  admitted static definitions keep the same projected wire shape.

### Schema

- AI schema module remains `0.59.0`. This release changes no persistence
  entity, stored semantic meaning, table, column, index or constraint.

## [0.78.3] - 2026-08-14

### Added

- Remote GraphQL authority issuance now receives a crate-authored
  `AiRemoteGraphqlCapabilityBinding`. Static reads carry their exact registered
  descriptor identity; generated queries additionally carry the active target,
  finished-schema, semantic-catalogue/root and offered capability identities.
  The binding is read-only to hosts and participates in delegation request
  serialization, equality, stable hashing and transport matching.

### Security

- Dynamic `AiQuery_*` operation names grant nothing. Generated-query bindings
  arise only after exact capability compilation and generated-target policy,
  while static reads retain ordinary static policy. Kind substitution, stale
  capability data, changed request bindings, mutations and subscriptions fail
  before private authority issuance. Fresh principal, current host policy,
  ordinary resolver authorization and short-lived exact authority remain
  mandatory.

### Schema

- AI schema module remains `0.59.0`. This release changes no persistence
  entity, stored semantic meaning, table, column, index or constraint.

## [0.78.2] - 2026-08-13

### Added

- `AiProviderCallPlan::new_with_read_capabilities` and
  `new_continuation_with_read_capabilities` admit one provider-visible set of
  exact static read descriptors and automatic generated query capabilities.
  The registered `AiToolCatalog` alone classifies each stable ID; static and
  generated entries retain their independent exact policy validation.

### Security

- Mixed admission rejects unknown or colliding IDs, mutations,
  subscriptions, stale definitions and stale target/schema/semantic-catalogue
  bindings. A denial in one catalogue kind cannot fall through to the other.
  Execution still uses fresh principal, host policy, resolver authorization,
  disclosure, egress, budget and durable coordinator fences.

### Schema

- AI schema module remains `0.59.0`. This release changes no persistence
  entity, table, column, index, constraint or stored semantic meaning.

## [0.78.1] - 2026-08-13

### Fixed

- Retained approval waits now atomically persist the exact protected
  `approval_wait_parked` checkpoint, close the source attempt as nonterminal,
  and release its ordinary run lease in the same transaction as the one-shot
  approval. Confirmation can be repaired only from that exact graph; an
  approved wait receives a fresh fenced attempt only after confirmation.

### Schema

- AI schema module `0.59.0` records the new retained-approval checkpoint and
  nonterminal attempt semantics. It changes no table, column, index, or
  constraint and requires no row rewrite or backfill.

## [0.78.0] - 2026-08-13

### Added

- Durable bounded subscription waits now bind canonical `ReplayThenLive`
  capabilities to authenticated source registrations, protected replay
  cursors, exact current-principal/rule/target checks, separately authorized
  provider egress, and the existing coordinator queue. One source item is
  examined per short-lived fenced claim; event, timeout and event-limit
  adoption commits cursor progress and the queued continuation atomically.
- `AiSubscriptionCheckpointAdopter` composes wait adoption with the existing
  checkpoint adopter so resumption rechecks the source event and disclosure
  before consuming the one-shot continuation.
- `AiProviderSessionState::ParkedWait` and the default-deny
  `AiProviderSessionService::{park_for_wait,confirm_parked_wait,reclaim_after_wait}`
  contract let approval and bounded subscription coordinators suspend an
  exact completed provider-retained continuation without holding a provider
  process or ordinary run lease. The park request is crate-issued from the
  exact provider claim, durable provider-turn checkpoint and complete
  continuation fingerprint; the later reclaim authority is derived only from
  the matching one-shot durable adoption graph.
- `AiToolCatalog::register_query_capability_catalog`, provider-definition
  projection and `AiRuntime::execute_query_capability` connect the canonical
  finished-schema semantic graph to closed automatic GraphQL read plans.
  Nested relationships and opt-in aggregate roots compile into exact
  server-authored documents, variables, disclosure contracts and drift
  fingerprints without model-authored GraphQL or static per-query profiles.
- `AiGeneratedGraphqlTargetPolicySet` and
  `AiGeneratedGraphqlAuthorizationPolicy` admit generated queries by one exact
  logical target plus active finished-SDL/semantic-catalogue binding. Ordinary
  generated reads no longer require a per-capability `AiToolPolicySet` entry.
- Classified generated mutations now compile as `Automatic`,
  `ApprovalRequired`, or `Prohibited`. Prohibited mutations are not offered;
  approval-required mutations reuse the existing exact one-shot approval
  lifecycle; automatic mutations use a distinct protected post-effect
  checkpoint and one-shot continuation proof.

### Security

- Parking rejects stateless replay, unfinished streams and provider-native
  dynamic-tool turns with synchronous responders. Confirmation requires a
  cleared run lease, the exact `WaitingApproval`/`WaitingSubscription` state,
  matching wait row and parked checkpoint. A stale, expired, restored,
  cancelled, terminal, swapped or already-reclaimed binding cannot resume;
  unconfirmed crash gaps and abandoned waits converge through the existing
  exact provider-deletion lifecycle.
- Automatic capability registration is discovery only. Execution still
  requires readiness, a freshly rehydrated principal, current host target/tool
  policy, ordinary resolver authorization, exact offered capability and plan
  fingerprints, bounded output and selected-field disclosure validation.
  Secret and `NeverExport` fields are absent from provider schemas.
- Static and automatic GraphQL disclosure contracts prove their checked
  worst-case total record count against the descriptor ceiling. Runtime
  evaluation independently counts the actual composite result, including
  sibling and nested list expansions, and fails closed above that total.
- Automatic writes require independent target policy, hierarchical
  `AutonomousWrite` policy, fresh principal/tool policy and ordinary resolver
  authorization. Once their pre-effect fence is durable, ambiguity converges
  to `RecoveryRequired`; a result checkpoint can continue but can never replay
  the non-idempotent resolver or be adopted as a read-only/approved result.

### Fixed

- Session-retention completeness proofs now traverse generated ORM pages up
  to the configured retention bound even when the database pagination maximum
  is smaller than that bound. Protected content and session shells are never
  deleted from a clamped partial proof set, while terminal subscription
  waiter/adoption tombstones drain in bounded mutation-safe batches.

### Schema

- AI schema module `0.58.0` adds private subscription waiter and one-shot
  adoption tables. Protected plans, cursors and outcomes are backup-redacted;
  portable restore therefore treats live waits as recovery-required, while an
  unchanged database may reclaim an exact cursor after restart.
- The provider-session binding gains private nullable parked-wait linkage,
  checkpoint/fingerprint, confirmation/expiry/reclaim columns plus one cleanup
  scan index in the same AI schema module `0.58.0`. Existing bindings require
  no data backfill and cannot be interpreted as parked.

## [0.77.1] - 2026-08-13

### Fixed

- Restored independent `provider-xai`, `provider-ollama`, and
  `provider-openai-compatible` builds by gating the OpenAI-only background
  overrides and helpers. Other adapters retain the provider-neutral
  fail-closed default, and runtime background capability remains unavailable
  unless the exact provider profile advertises it.

### Verification

- Added a local provider runner that builds and tests every provider feature
  separately with one explicit persistence backend, including the Codex app
  server and local harness lanes.

## [0.77.0] - 2026-08-13

### Added

- `AiProviderSessionRunDisposition` gives hosts and the provider executor one
  authoritative distinction between a new session, an exact resumable
  binding, an unavailable lifecycle state, and a deleted generation that may
  be rebound after exact provider-absence proof.
- `AiProviderSessionRebindAuthorization` and
  `AiProviderSessionService::rebind_for_run` provide a crate-issued,
  short-lived, run-fenced replacement contract. Consumers cannot construct or
  alter the authorization.
- `AiProviderFailureCategory` and the optional
  `AiProviderFailureDiagnosticSink` expose bounded content-free operational
  diagnosis without changing conservative run outcomes. Bounded Codex
  app-server startup, turn, delete, interrupt, and shutdown timeouts now report
  the closed timeout category instead of looking like owner cancellation.

### Changed

- Successful provider-session cleanup now retains a private `Deleted`
  tombstone containing the exact cleared generation and absence observation.
  A later current-authority run may atomically replace that same row with a
  freshly protected cursor and host-supplied canonical transcript fingerprint.
- `AiProviderCallExecutor` owns fresh-session versus resume versus rebind
  selection. If empty-thread creation succeeds but binding or rebinding loses
  its fence, it invokes the existing exact provider discard boundary.
- Final deleting-session retention removes an absence-proven tombstone only as
  part of the existing session-deletion dependency order.

### Security

- Rebind eligibility is denied for active, claimed, cleanup-required,
  cleanup-in-progress, cleanup-backoff, and restore-quarantined rows. Expiry,
  process loss, transport ambiguity, or cursor redaction is never provider
  absence proof.
- Rebind rehydrates current principal/session authority and atomically binds
  the exact owner, scope, run, attempt, lease generation, deleted row version,
  cleanup generation, provider descriptor, and canonical transcript prefix.
  Old cursor material is cleared and cannot be reopened.
- Failure diagnostics contain only a closed machine category. They cannot
  carry prompts, output, tool data, provider bodies, credentials, cursors, or
  authorization details and cannot authorize retry.

### Schema

- AI schema module `0.57.0` records the durable provider-session tombstone and
  rebind semantics. No entity, table, column, index, constraint, or stored-row
  representation changes; existing rows require no rewrite or backfill.

## [0.76.1] - 2026-08-13

### Changed

- Aligned the complete workspace auth type universe to `agql-auth` 0.15.0 at
  exact revision `e841ffd382082ad7419be259fe957f949b956ff7`.
- Added consumer-level compatibility coverage for
  `VerifiedActiveUserSessionResolver`, `VerifiedActiveUserSession`,
  `SessionBoundDelegationBinding`, and the prepare/issue session-bound
  access-token-only methods. The issued credential retains its exact active
  user session and creates no refresh-store mutation.

### Security

- Existing principal rehydration, scope matching, actor/resource/correlation
  bindings, and resolver authorization remain unchanged. Session-bound
  delegation authority is owned by agql-auth and does not give the AI runtime
  a synthetic session, refresh credential, or generic token-issuance path.

### Schema

- AI schema module remains `0.56.0`. No entity, table, column, index,
  constraint, backup, restore, or stored-row semantics change.

## [0.76.0] - 2026-08-12

### Added

- `AiRunTerminalEvent` provides the closed mapping from authoritative durable
  run states to `run_completed`, `run_failed`, `run_cancelled`, and
  `run_recovery_required` event names.

### Changed

- Every successful fenced terminal/recovery run transition now atomically
  advances the session and owner-inbox watermarks and appends one canonical
  metadata-only terminal event beside the immutable attempt outcome. This
  includes ordinary and supervised coordinators, expired-lease recovery,
  approval-wait reconciliation, and background-provider reconciliation.
- The existing owner-cancellation transaction remains authoritative for
  `run_cancelled`; generalized completion loses its fence after cancellation
  and cannot append a duplicate event.

### Security

- Terminal payloads use an exact server-authored, content-free metadata
  envelope containing only its format and closed run state. Prompts, output,
  tool data, provider references, authorization detail, and error text never
  enter these events. Session and inbox reads retain current owner, scope,
  content-policy, retention, and replay authorization.
- Run transition, attempt outcome, session event, session watermark, inbox
  event, inbox watermark, and commit-only wakeups now succeed or roll back as
  one ORM state-machine transaction. A stale fence, duplicate finish, or event
  persistence failure cannot publish a partial terminal projection.

### Schema

- AI schema module `0.56.0` records the new authoritative durable terminal
  event semantics. No entity, table, column, index, constraint, or stored-row
  representation changes.

## [0.75.1] - 2026-08-12

### Fixed

- Durable session and owner-inbox event pages now derive `HasMore` from the
  last contiguous sequence and the page's captured watermark. Requests at the
  configured ORM maximum therefore drain every replay page instead of losing
  the look-ahead row to database pagination clamping.

### Security

- Session and inbox replay continue to fail closed on missing, noncontiguous,
  wrong-owner, wrong-session, purged, or out-of-watermark rows. The correction
  changes no principal, scope, content-protection, disclosure, or retention
  authorization boundary.

## [0.75.0] - 2026-08-12

### Added

- `AiToolCatalog::read_only_model_definition` constructs the provider-facing
  definition directly from one registered read-only descriptor. Hosts choose
  only a provider-safe alias; stable ID, description, canonical argument
  schema, strictness, and fingerprint cannot drift into a second declaration.
- `AiCodexAppServerBootstrapInstructions` carries bounded compile-time static
  deployment instructions for retained threads. Its protected content
  fingerprint participates in registration identity and is rechecked through
  creation, first activation, and resume.

### Changed

- `AiCodexAppServerRunProcess::create_empty_thread` now receives the typed
  static bootstrap beside the model and exact dynamic definitions. Retained
  `ModelRequest::instructions` must be empty; request-local text cannot enter
  the developer-instruction channel.
- Registration identity version 3 binds the bootstrap fingerprint. Existing
  retained cursors from older registration identities become cleanup-only.
- Provider-neutral session cursor, descriptor, activation, and deletion
  contracts now remain available in the MSSQL compile/schema profile. The ORM
  provider-session runtime remains intentionally limited to SQLite and
  PostgreSQL.

### Fixed

- Canonical GraphQL tool-profile argument schemas are projected through a
  closed Codex-specific JSON Schema subset. Unsupported scalar bound keywords
  are represented in provider-visible property descriptions, while the exact
  canonical schema remains authoritative for every returned dynamic call.
- The retained first turn now receives the same immutable trusted instructions
  that created its empty thread. Tool behavior no longer depends on a
  request-local instruction that cannot be transmitted after durable binding.

### Security

- Generated-profile, tool-bound plan, coordinator, newly-bound, retained
  resume, exact responder, cancellation, process-fence, schema substitution,
  descriptor substitution, instruction substitution, and cursor substitution
  tests share one canonical generated read-only profile with a bounded integer
  input and explicit result projection. The ignored Codex 0.147.0 acceptance
  uses that same profile rather than a hand-authored model definition.

## [0.74.0] - 2026-08-12

### Added

- `AiCodexAppServerLaunchProfile` now supplies a closed, fingerprinted
  experimental dynamic-tools-only process and thread profile. It exposes the
  exact reviewed Codex CLI arguments, requires an isolated configuration home,
  disables native execution, browser, hosted-search, connector, image,
  collaboration, plugin, and interactive surfaces, and makes every dynamic
  thread and turn environment-free.
- `AiCodexAppServerModelToolMode` records the reviewed executable/model
  catalogue mode. The dynamic-tools-only profile accepts only `Direct` models;
  Code Mode and Code Mode-only models fail configuration rather than silently
  advertising unusable custom tools.
- `AiCodexAppServerRunProcessFactory::supports_launch_profile` makes runtime
  readiness explicit. Existing factories default to text-only support, and
  `ProviderCapabilities::custom_tools` remains false until the factory attests
  the exact dynamic-only profile.

### Changed

- Dynamic Codex registrations now use `with_launch_profile` instead of the
  former boolean `with_experimental_dynamic_tools` switch. Registration
  identity version 2 includes the closed launch profile, so stale retained
  cursors are cleanup-only after adoption.

### Fixed

- The strict protocol actor now accepts JSON-RPC server-request identifier
  zero for an otherwise exact, lifecycle-correlated `item/tool/call`. Codex
  0.147.0 uses this valid identifier for its first dynamic request; duplicate,
  malformed, uncorrelated, unknown, or schema-invalid requests still fail
  closed.

### Security

- The live Codex 0.147.0 acceptance now proves one exact dynamic tool call on
  both a newly bound thread and a later retained resume while native shell,
  unified execution, Code Mode, filesystem environments, MCP, browser,
  hosted web, images, plugins, collaboration, and generic JSON-RPC remain
  unavailable. It also proves provider-owned interruption and terminal close
  for an active dynamic turn.

## [0.73.4] - 2026-08-12

### Fixed

- Both strict Codex app-server initialization paths now send one
  library-owned `optOutNotificationMethods` profile for unused thread status,
  thread settings, cleared goals, MCP startup status, and account rate-limit
  notifications. Authoritative thread starts, turns, item lifecycles,
  assistant deltas, dynamic-tool requests, in-turn usage, and completion are
  never suppressed. A server that emits any opted-out method still fails
  closed at the actor.
- Retained-thread deletion now uses only the exact empty successful
  `thread/delete` response as provider-absence evidence. The actor no longer
  admits a deletion-bound `thread/status/changed` exception.
- Codex 0.147.0 empty reasoning item pairs are admitted only through the new
  content-free `AiCodexAppServerInbound::ReasoningLifecycle` value while every
  `turn/start` explicitly sets `summary: "none"`. Non-empty content or summary,
  malformed pairs, and raw reasoning deltas remain rejected and never cross
  the provider boundary.
- A cumulative `thread/tokenUsage/updated` value replayed while loading a
  retained thread is validated against the complete nonnegative generated
  shape and exposed only as content-free
  `AiCodexAppServerInbound::RetainedResumeUsageSnapshot`. It is never charged
  to the next run. On Codex 0.147.0, the exact snapshot can complete only a
  typed resume after its correlated response is observed when that version
  omits `thread/started`; it cannot complete new thread creation.

### Security

- The live Codex 0.147.0 gate now covers a persistent empty thread, an initial
  direct dynamic-tool turn, process restart, retained resume, a second
  dynamic-tool turn, and response-authoritative deletion while shell, files,
  MCP, browser, hosted web, remote control, and generic JSON-RPC stay closed.

## [0.73.3] - 2026-08-12

### Fixed

- The strict Codex app-server actor now admits the documented generic
  `warning` notification only during an exact correlated turn. Its positive
  timestamp, closed parameter shape, optional thread binding, bounded
  control-free message, per-turn count, and cumulative bytes are validated;
  the message, timestamp, and thread reference are then discarded behind the
  content-free `AiCodexAppServerInbound::RuntimeWarning` variant.
- Warnings before `turn/start`, after terminal completion, for another thread,
  or with malformed, extra, oversized, control-bearing, or flooding payloads
  remain rejected. No generic notification, remote-control, shell, file, MCP,
  browser, hosted-web, or JSON-RPC capability is added.

## [0.73.2] - 2026-08-12

### Fixed

- Durable provider-session activation now distinguishes a newly created empty
  thread from a previously committed retained thread. The first turn consumes
  a crate-owned, run-fenced activation and starts directly on the exact
  process and cursor that created the empty thread; later runs continue to use
  the strict `thread/resume` lifecycle.
- The Codex app-server run pool freezes the empty thread's cursor and reviewed
  dynamic-tool definitions, rejects process, run, cursor, registration,
  model, or tool swaps, and consumes initial activation once. Bind/open
  failure, cancellation, lease loss, and abandoned streams retain exact
  deletion and process-tree cleanup behavior.

### Added

- `AiCodexAppServerRunProcess` has typed `start_bound_turn` and
  `start_bound_dynamic_turn` operations for the post-bind first turn. They do
  not expose activation state or a reset flag and must not issue
  `thread/resume`.

## [0.73.1] - 2026-08-12

### Fixed

- The strict Codex app-server actor now begins a new internal lifecycle
  observation phase for each typed `thread/start` or `thread/resume` request.
  One actor can create an empty retained thread, accept its response and
  `thread/started` notification, resume the same protected cursor, accept the
  new pair in either order, and start a turn. Incomplete, duplicate,
  mismatched, late, or concurrently active lifecycle cycles remain rejected.
- Retained model and experimental dynamic-tool definitions remain frozen
  across terminal turns and repeated resume cycles. A later resume or
  `turn/start` cannot replace the tool set, while ephemeral thread definitions
  are still cleared after their terminal turn.

## [0.73.0] - 2026-08-11

This development line advances the pre-1.0 crate version to `0.73.0` and AI
schema module `0.55.0`. It begins the applied-restore implementation with
bounded database-derived facts, aligns the reviewed dependency universe,
integrates generated resolver-operation metadata, completes the durable OpenAI
background terminal-reconciliation runtime, and closes raw provider
file-search IDs behind the reviewed persistent-file design.

### Fixed

- Codex app-server lifecycle notifications now use one strict generated-wire
  envelope requiring a positive signed `emittedAtMs`. The actor validates and
  discards that timestamp, rejects duplicate, missing, malformed, overflowing,
  extra-field, mismatched, duplicate-lifecycle, and late frames, and preserves
  response/notification ordering for thread and turn starts.
- The exact `thread/status/changed` transition to `notLoaded` is admitted only
  for the same thread already under a correlated delete request. This permits
  a complete Codex CLI 0.147.0 persistent create/delete handshake without
  admitting general status traffic or any new provider capability.
- The strict Codex app-server actor now admits only the complete bounded
  `remoteControl/status/changed` notification whose status is exactly
  `disabled`, without exposing any remote-control method or identity. Codex
  CLI 0.147.0 readiness no longer fails on that content-free initialization
  signal; connecting, connected, errored, malformed, repeated, late, and
  unknown remote-control traffic remains rejected.
- Codex app-server capabilities now truthfully advertise implemented protected
  provider-session continuation. Every new and resumed thread explicitly fixes
  approval policy to `never` and sandbox mode to `read-only` rather than
  inheriting operator configuration.
- Re-exported GraphQL tool manifest version 2 recursively canonicalizes JSON
  object keys for manifest and nested descriptor fingerprints. Router
  `DescriptorExtension` normalization now round-trips unchanged complex
  projections, disclosure maps, and JSON Schemas without weakening any stale,
  schema, descriptor, or fingerprint validation.
- Provider-session cleanup canonicalizes trusted absence-observation times to
  the durable second-resolution clock before freshness comparison, so a valid
  same-instant proof with subsecond precision is not rejected as future-dated.
- Streaming and background OpenAI citation normalization now share one strict
  HTTPS, bounded-title, exact output-item/content-index/span contract. Unsafe,
  wildcard, user-info-bearing, control-bearing, malformed, oversized, or
  provenance-free citation metadata fails closed; assistant Markdown links
  never become authoritative source records.
- Synchronous and background provider-output persistence now protect assistant
  message previews using the same bounded top-level JSON string already used
  by user messages and required by `AiMessages`. The reader remains compatible
  with the exact `{"text":"..."}` form written by 0.62.0, while rejecting
  malformed, ambiguous, or oversized legacy values. Existing protection
  context, ownership, scope, retention, and content bounds remain enforced.

### Changed

- Durable provider-session watermarks now become reusable only after protected
  assistant output, its `assistant_output_persisted` checkpoint, and canonical
  terminal run completion are durable. If the retention-only commit then
  fails, the already-completed answer remains successful and the opaque cursor
  is quarantined for exact deletion.
- The provider empty-session hook now receives the exact validated
  `ModelRequest` as a read-only binding input. Adapters may use only immutable
  model and reviewed tool definitions required by their thread protocol and
  must not transmit request instructions or user input before the cursor is
  durably protected.

- `ModelRequest` now carries an explicit host-selected visible-reasoning
  summary request, and web search uses an explicit `PublicWeb`, allowed-domain,
  or blocked-domain policy instead of an ambiguous empty filter. Provider
  capabilities and hierarchical rules independently advertise/permit visible
  summaries. These public request, capability, event, and rule-enum changes are
  intentional pre-1.0 source breaks; missing serialized summary fields remain
  backward compatible through a disabled default.
- Final protected provider blocks preserve normalized provider order rather
  than grouping all text, summaries, and structured metadata by kind. The
  optional `AiProviderActivitySink` supersedes the text-only live sink when
  enabled and persists one protected, cursor-addressable order for visible
  text, visible reasoning summaries, hosted-tool lifecycle, and citations.
- Hierarchical rule budgets now include a separately cumulative
  `maximum_web_search_calls` ceiling. Requested maxima are checked before each
  turn and exact normalized completed searches are carried through provider
  results and coordinator checkpoints so a continuation cannot reset the
  per-run limit.
- The canonical GraphQL tool-profile/compiler/manifest surface now lives in
  the database-neutral `graphql-orm-ai-tool-profiles` package and is re-exported
  unchanged by `graphql-orm-ai`. Owning subgraphs can compile reviewed
  generated and custom profiles without selecting or compiling an AI
  persistence backend. Generated resolver metadata similarly lives in the
  backend-neutral `graphql-orm-operation-catalog` package and remains
  re-exported by `graphql-orm`.
- AI persistence no longer acquires `graphql-orm-backup` transitively. Hosts
  that use the independent backup companion declare it directly at the same
  reviewed monorepo revision. This removes an unused backend-unification edge;
  AI schema metadata and restore contracts are unchanged.
- Companion crates with a mutually exclusive host backend can use
  `#[backend_selected_graphql_entity(...)]` so their entity derives bind their
  own `sqlite`, `postgres`, or `mssql` feature even when another workspace root
  unifies additional `graphql-orm` backend features.

- Removed `AiRestorePlan::readiness_report_after_apply`. A pure dry-run plan
  cannot truthfully manufacture evidence that database repairs were applied
  and post-apply validation succeeded. The existing
  `AiRuntimeStartGate::open` report remains a host-attested compatibility seam,
  not applied-restore authority; restored deployments must keep it closed.

### Added

- The strict Codex app-server adapter now supports exact protected persistent
  thread create/resume/interrupt/delete. `AiProviderSessionTurnPlan` couples a
  provider call to the existing owner/scope/profile/model/executable/protocol/
  policy/transcript/run-fenced binding while warm processes remain a separate
  bounded in-memory concern.
- Codex native dynamic tools are available as an explicit experimental,
  default-off registration capability. The protocol actor admits only exact
  reviewed `item/tool/call` requests and returns them through a typed responder
  to the ordinary read-only coordinator. Current cancellation, rules, tool
  registration, GraphQL resolver authorization, disclosure, egress, durable
  tool lifecycle, and budgets remain authoritative; the adapter receives no
  application credential or generic callback.
- AI schema module 0.55.0 records the stricter terminal-before-provider-
  watermark semantic. It adds no table, column, index, or row rewrite; existing
  0.54.0 provider-session rows remain valid and the next migration plan records
  only the new module version.

- The `provider-codex-app-server` feature adds a strict project-neutral Codex
  app-server foundation, initially for fresh text-only turns. It keeps at most one process per
  exact run/attempt/lease generation, freezes executable/profile/model/sandbox/
  protocol identity, applies global and per-owner admission, reuses the process
  across bounded turns, and exposes exact interrupt/close lifecycle hooks.
  The protocol actor has no generic send method and rejects unenabled dynamic tools,
  shell/command/file/MCP/web/image/collaboration traffic, unknown server
  requests, and raw reasoning. A crate-owned launched-process wrapper always
  invokes the deployment's synchronous process-tree kill callback on final
  drop. The 0.70.0 additions above layer exact protected thread retention and
  default-off coordinator-owned experimental dynamic tools onto this closed
  foundation.
- `AiProviderSessionService` and `OrmAiProviderSessionService` add a protected,
  provider-neutral durable opaque-cursor lifecycle. Bind, claim, open,
  heartbeat, commit, invalidate, cleanup, retry, and exact absence proof are
  fenced to session owner/scope, principal reference, run attempt/generation,
  provider profile/model/protocol/registration/policy identity, and canonical
  transcript watermark. Cursor material is private, content-protected,
  backup-redacted, never exposed through GraphQL, and distinct from any
  in-memory warm process. Session deletion waits for authoritative provider
  absence; portable restore remains readiness-blocked while any binding exists.
- `ModelReasoningSummaryRequest`, `ProviderCitation`, explicit web-search
  domain policies, per-kind `AiProviderBuiltinUsage`, and the typed
  `AiProviderActivity` stream provide reusable contracts for visible provider
  summaries, hosted search, source attribution, ordered replay, and exact
  accounting without exposing hidden chain-of-thought or raw hosted-tool
  result bodies. Native OpenAI Responses maps `reasoning.summary=auto`, mixes
  reviewed application tools with hosted search only in provider-retained
  mode, and keeps the existing stateless mixed-tool prohibition closed.
- AI schema module 0.54.0 adds only the private
  `graphql_orm_ai_provider_session_bindings` table and its bounded claim/
  cleanup indexes. Existing rows require no rewrite or backfill. The required
  restore-audit set now includes provider-session bindings and fails closed
  until retained provider state is drained and absence is proven.
- Application tool descriptors and generated/custom profiles can now opt into
  a separately bounded, fingerprinted `AiBrowserResultPreviewPolicy`. The new
  owner-authorized `AiToolCallResultPreview` query rehydrates current
  authority, proves the exact session/run/call and descriptor/disclosure
  bindings, reruns current tool policy, opens protected content only for the
  response, and requires a host `AiToolResultPreviewAuthorizer` to apply
  current row/field policy and return a least-disclosure projection. Missing
  policy, denial, purge, retention expiry, classification excess, malformed
  storage, or bound/schema failure discloses no result content.
- Fenced application-tool execution now appends protected
  `application_tool_started` and `application_tool_completed` events to both
  the session stream and owner inbox. Start is committed only after read-only
  preauthorization or approved consequential-call consumption, never while a
  call is merely proposed, staged, or denied. Lifecycle events contain only
  bounded identifiers and terminal state; arguments, results, errors,
  provider details, and authorization internals remain protected elsewhere.

- `CancelAiRun` and `AiRunCancellationService` add an owner-authorized,
  idempotent cancellation request for one exact session/run pair. The ORM
  service rehydrates current authority, atomically wins the durable run fence,
  closes active tool/approval/background state, records immutable request and
  attempt evidence, and appends protected request/final session and owner-inbox
  events. `AiRunCancellationHub` accelerates worker wakeups without replacing
  authoritative database polling. The read-only coordinator drops an active
  provider future and checks the durable cancellation fence before tools,
  checkpoints, output persistence, continuations, and terminal completion.
- AI schema module 0.53.0 adds nullable cancellation evidence to private run
  rows and the private idempotency record. No generated CRUD root is exposed.

- `RenameAiSession` and `AiSessionService::rename_session` now provide an
  owner-authorized, bounded, revision-fenced, idempotent session-title update.
  The ORM service atomically advances the session stream, appends one protected
  `session_title_changed` event, and appends the corresponding protected owner
  inbox event. `AiSessionView.title_revision` exposes the authoritative CAS
  revision; browser input cannot select the closed persisted actor source.
- `AiSessionTitleWorkService` and `OrmAiSessionTitleWorkService` add one
  provider-neutral durable title job for the first successfully persisted user
  message. Bounded claims, expiring leases, generation/row-version fences,
  current-principal rehydration, owner/scope reauthorization, protected
  first-message opening, retry/failure states, and an idempotent conditional
  completion let a host use a separately reviewed tool-free provider. A manual
  rename, custom or pre-upgrade title, or deletion state wins
  without being overwritten. The contract grants no provider, tool, URL,
  shell, file, screenshot, remote-control, or GraphQL authority.
- AI schema module 0.52.0 adds private title-mutation and title-work records,
  claim/session indexes, and session title revision/source columns. Existing
  rows receive the closed `user` source so migration cannot mistake an unknown
  pre-upgrade custom title for an untouched default title.

- `AiGraphqlToolManifestBuilder` now compiles reviewed generated-resolver and
  handwritten-root profiles against the owning subgraph's finished SDL. A
  profile supplies a closed typed argument adapter, fixed arguments, semantic
  aliases, explicit bounded projection, disclosure schema, and output limits;
  the library generates the GraphQL document, JSON Schema, public-coordinate
  tool identity, exact contracts, and a versioned fingerprinted manifest.
  Multiple least-disclosure profiles may bind one root. Relationships remain
  absent unless explicitly selected with bounded depth and list cardinality.
  Query profiles are read-only; mutations require the separate supervised
  constructor and one-shot approval semantics.
- `AiGraphqlToolManifestSet` rejects active-schema drift, unsupported manifest
  versions, duplicate tool IDs, and a root advertised by multiple subgraphs.
  The optional `graphql-orm-ai.tool-manifest` router-extension payload is
  project-neutral and remains discovery only. Owning subgraphs validate ORM
  generated-operation fingerprints and host classification during manifest
  compilation; schema-validated federated consumers can register the active
  set without importing service crates. Custom roots retain their
  authoritative resolver policies.
- Tool descriptor setters now safely refresh classification, idempotency, and
  complete descriptor fingerprints. Catalog registration rejects a stale
  descriptor fingerprint.

- `AiReadOnlyAgentTurnPlan::new_chat` now binds an initial provider call with
  no application or provider-built-in tools, tool-result input, continuation,
  or tool-result egress route to one exact current rule set. The read-only
  coordinator preserves its ordinary current-principal, rule, provider,
  classification, BYOK, egress, atomic-budget, output-protection, and terminal
  checks while completing this single tool-free turn without a tool checkpoint
  or continuation. Unoffered application-tool events remain rejected by the
  provider normalizer and fail closed before any tool service if a custom
  executor violates that boundary.

- `DeploymentAiCurrentRuleResolver` supplies library-owned, immutable,
  deployment-only rule evidence without artificial ORM policy rows. It
  rehydrates the exact lease principal twice through the same freshness,
  expiry, and reference boundary as `OrmAiCurrentRuleResolver`, binds the
  validated `AiRuleDeploymentLimits` ceiling to the exact requested scope,
  records an empty applied-layer lineage, and computes the canonical
  fingerprint internally. It remains narrowing evidence only and grants no
  provider, tool, resolver, egress, budget, approval, credential, or resource
  authority. The proof constructor remains crate-private.

- `AiRestoreAuditKind::Attachments` is replaced by the non-overlapping
  `AttachmentMetadataGraph` and `AttachmentObjectBytes` restore audits.
  `AiRestoreAttachmentMetadataAuditLimits` and
  `OrmAiRestoreFactCollector::with_attachment_metadata_audit` now perform one
  bounded generated-ORM audit of attachment/artifact lifecycle, ownership,
  session/message parents, provider-reference tuples, and unique local object
  ownership. Complete canonical row and expected-object digests bind that
  database proof to the collected facts. Missing parents, backup-redacted
  transient/provider references, malformed state, duplicate object ownership,
  or a reached row bound remain fatal. The database audit performs no blob I/O;
  `AttachmentObjectBytes` remains explicitly incomplete until a verified
  manifest and the restored target bytes are streamed and rehashed.
- `AiRestorePolicyAuditLimits` and
  `OrmAiRestoreFactCollector::with_policy_audits` now bind host-attested
  deployment ceilings and independent row bounds into database-derived restore
  facts.
  The collector can completely audit budget-policy identity, scope,
  principal, interval, and ceiling integrity plus immutable pricing identity,
  route cardinality, rates, creator linkage, and exact creation-audit facts.
  Omitted deployment ceilings, reached bounds, malformed rows, missing or
  duplicate creation audits, and orphan pricing audits all remain fatal. These
  dry-run inputs do not prove equivalence to live service configuration; the
  future applied validator must bind that exact configuration epoch.
- `OrmAiRestoreFactCollector` now reads runs, approvals, and egress consents
  through generated ORM queries in one bounded transaction. It conservatively
  classifies interrupted effects, validates core durable shapes, reports a
  reached bound as fatal incompleteness, and produces an opaque deterministic
  fact digest without provider, tool, application, or blob I/O.
- `AiRestoreAuditKind`, `AiRestoreAuditStatus`,
  `AiCollectedRestoreFacts`, and `AiCollectedRestorePlan` make audit coverage
  explicit. `AiRestoreReconciler::plan_collected` emits a fatal issue for
  every unimplemented, truncated, or invalid audit and binds the dry-run plan
  to exact fact and plan digests. These types do not prove repairs or open
  readiness; the applier, post-apply validator, recovery epoch, and opaque
  runtime-start proof remain closed.

- SQLite and PostgreSQL builds now resolve the exact reviewed
  `graphql-orm-backup` 0.7.0 package alongside `graphql-orm` 0.16.0 and
  `graphql-orm-storage` 0.6.0 in one workspace source/type universe. This is a
  dependency and backup-schema compatibility checkpoint; the AI-specific
  empty-target restore collector, repair applier, validation, recovery epoch,
  and readiness gate remain deliberately closed.
- Schema module 0.51.0 includes finalized local attachment and derived-artifact
  `blob_reference` values in confidential database backups so restored rows can
  reconnect to the separately restored object bytes. Quarantine references,
  upload-token hashes, provider-file references, credentials, and secrets
  remain redacted. Snapshots made with the prior redacted linkage are not
  eligible for a complete local-object restore and must be replaced with a new
  full snapshot.
- `GraphqlOperationContract::with_generated_operation`,
  `GraphqlGeneratedOperationBinding`, and
  `AiToolCatalog::register_generated_with_disclosure` bind one
  server-authored GraphQL document to one currently exposed derive-generated
  resolver and the exact immutable `graphql-orm` operation catalog. Admission
  rechecks catalog/operation fingerprints, operation kind, exact unaliased
  single-root selection, ordinary disclosure bounds, and an explicit
  host-supplied `AiGeneratedGraphqlOperationPolicy`. The default policy denies
  all generated operations. Metadata remains discovery/drift evidence, not
  enablement, application-domain classification, authorization, finished host
  SDL validation, projection, disclosure, or runtime limits. Custom roots
  retain the explicit reviewed contract/scanner path.
- `docs/provider-files.md` now defines the complete default-deny capability,
  durable identity, state-machine, quota, pricing, egress, cleanup, retention,
  restore, and conformance contract for future provider-persistent upload and
  search. The design records why current provider-assigned create identities
  and storage-time billing prevent a safe partial implementation; no upstream
  ORM or auth change is required at this checkpoint.
- `docs/ordering-history.md` fixes canonical effect coordinates and phase
  order, capacity checks before approval/checkpoint consumption, the
  cross-generation adoption matrix, provider-family stateless transcript
  rules, crash-window outcomes, and negative-test obligations. It confirms the
  existing complete read-only and single supervised retained-result adoption
  paths while keeping partial, mixed, stateless supervised, and parallel
  execution closed; generic parallel consequential execution is explicitly
  unsupported.
- `docs/recovery-and-restore.md` classifies current recovery, retention,
  append-only, backup, and restore states and fixes the evidence contract for a
  future privileged uncertain-effect service. The follow-up
  `graphql-orm-backup` 0.6.0 merge aligns that hardening with this crate's
  reviewed ORM 0.16.0 universe. No incompatible pin, duplicate dependency
  universe, or downstream workaround is accepted.
- `docs/coordination-gates.md` applies the ordering proof to every Slice 5
  shape. Existing complete read-only adoption and sequential single-mutation
  provider-retained turns are confirmed; provider-turn-only, partial, mixed,
  parallel, and stateless supervised paths remain closed. Per-item proposal
  review remains gated behind applied restore, and generic parallel
  consequential execution is permanently unsupported.
- `docs/control-plane-production.md` records the Slice 6 production-boundary
  audit. The explicit catalog/disclosure, secret-store, delegated-authority,
  and private-transport seams remain supported, while durable tool-policy
  management stays closed until its stored constraints are enforced and
  applied restore passes. Reviewed `graphql-orm` generated resolver metadata is
  now integrated for exact schema-aware generated-root drift checks; custom
  roots remain explicit and scanner-checked.
- `docs/backend-capability-matrix.md` records the Slice 7 evidence boundary:
  SQLite and PostgreSQL are classified separately from host/consumer proof,
  applied restore remains closed, and MSSQL remains an experimental
  compile/schema profile rather than a production claim. A separate
  copy-ready upstream prompt requests the missing reusable MSSQL write,
  transaction, migration, restore, and concurrency runtime.
- With `provider-openai` plus SQLite/PostgreSQL, exact accepted background
  submissions now reach a complete local terminal graph. Claiming
  deterministically matches at most one signature-verified receipt by native
  provider, logical profile, response ID, and pending state; webhook delivery
  remains optional and never supplies response content or execution authority.
  A new composite receipt index keeps that lookup bounded. Reclaim retains the
  exact match, while later agreeing terminal receipts become
  `duplicate_terminal` and disagreements become `recovery_required`.
- `OrmAiOpenAiBackgroundRetrievalService::retrieve_classified` distinguishes
  bounded retryable timeout/rate-limit/unavailability proofs from
  credential/configuration/rejection recovery proofs without retaining provider
  diagnostics. `handle_retrieval_failure`, `release_nonterminal`, and
  `close_expired` consume exact private generation-bound proofs, apply bounded
  exponential backoff, and atomically close retry/deadline exhaustion or
  non-retryable failures while leaving uncertain budget reserved.
- `OrmAiOpenAiBackgroundTerminalService` rehydrates current principal, access,
  protection, and egress policy, prices authoritative usage, protects reviewed
  completed output, then rechecks authority immediately before one generated-
  ORM state-machine transaction. That transaction settles budget/counters and
  one usage row exactly once, commits the optional assistant message/blocks,
  checkpoint, session and inbox events, closes receipts, appends the immutable
  attempt outcome and audit, and transitions the submission and parked run.
  Failed, incomplete, and cancelled observations settle usage without assistant
  output. Conflicting terminal evidence closes for recovery without releasing
  uncertain capacity. Exact replay validates the durable graph and returns
  `AlreadyReconciled`.
- The reviewed dependency audit now pins
  `graphql-orm`/`graphql-orm-macros` `0.16.0`
  (`dd68a001f47f04178bf3389dd47ee952faa6ecf0`) and keeps `agql-auth`
  `0.12.0` (`3f3b0c5365adfbe436514a681d977b600991b797`).
  They resolve as one package/type universe. The ORM change is additive
  upstream but enables the new downstream generated-operation binding API.
- With `provider-openai` plus SQLite/PostgreSQL,
  `OrmAiOpenAiBackgroundRetrievalService` now revalidates an exact active
  reconciliation claim, freshly rehydrates principal authority, proves
  current scope/session write access and a ready content-protection policy,
  audits a new profile/destination/model/classification-bound egress decision,
  and CAS-binds the allow ID before provider I/O. Public fixed logical route
  and response/normalization/timeout limits contain no URL or credential.
  The native OpenAI adapter accepts only an opaque crate-authored binding and
  issues one exact fixed-endpoint Responses GET with redirects disabled and
  just-in-time credentials. It bounds the full JSON and visible output,
  revalidates every durable response/metadata fact, accepts only reviewed
  statuses and message/reasoning/refusal/citation shapes, rejects tool or
  built-in output, and validates terminal token usage against the submitted
  ceiling. The result remains in-memory and grants no receipt, budget,
  persistence, or run-mutation authority. An expired transport-marked claim
  can be reclaimed only under a higher generation, which clears the stale
  retrieval marker before another attempt.
- Schema module `0.48.0` adds content-free reconciliation owner/generation/
  lease, next-attempt/retry, fixed deadline, current retrieval-egress,
  reconciled-time, and terminal-message fields to OpenAI background
  submissions. Newly accepted rows initialize the bounded scheduling facts and
  capture a deadline that cannot exceed either the provider-creation or local-
  acceptance time plus the deployment-reviewed response window.
- Public `AiOpenAiBackgroundReconciliationWindows` and
  `OrmAiOpenAiBackgroundSubmissionService::with_reconciliation_windows`
  configure those acceptance-time bounds. Defaults are five minutes for
  temporary `store: false` responses and 29 days for stored responses;
  compiled ceilings are ten minutes and 30 days.
- Public `AiOpenAiBackgroundReconciliationLimits`,
  `AiOpenAiBackgroundReconciliationClaim`, and
  `OrmAiOpenAiBackgroundReconciliationService` add a bounded generated-ORM
  claim queue over accepted submissions. Claim and expired-claim reclaim
  validate the complete deterministic submission, lease-free waiting run,
  active session, original attempt without an outcome, uncertain budget, and
  original egress allow before CAS-incrementing a separate reconciliation
  generation. Heartbeat rotates the exact row-version proof without extending
  the immutable response deadline; voluntary release is allowed only before
  provider retrieval and atomically clears ownership, increments the retry
  count, and schedules a bounded later attempt. Racing workers receive at most
  one claim, stale generations fail closed, and migrated rows without a
  deadline remain ineligible. Claims grant no credential, current-authority,
  provider-retrieval, egress, output, budget-settlement, or run-mutation
  authority; the separate retrieval service requires the claim plus fresh
  access/protection/egress proofs, and the terminal service requires a bounded
  exact observation plus a second current-authority check.
- With `provider-openai` plus SQLite/PostgreSQL,
  `OrmAiOpenAiBackgroundSubmissionService` now prepares one exact
  run/attempt/fence/profile/model/request/budget/egress binding, rehydrates
  current scope/session authority, marks the reservation uncertain immediately
  before transport, periodically renews the same fence while awaiting the
  create acknowledgement, and issues exactly one initial tool-free,
  attachment-free OpenAI background Responses request with an explicit
  provider-enforced output-token ceiling. Any failure known to precede
  preparation/transport releases unused reserved capacity. The native adapter
  forces non-streaming background mode, embeds only opaque deterministic
  binding metadata, and accepts only a bounded content-free acknowledgement
  echoing the exact model, output ceiling, storage choice, and opaque binding.
- Accepted submissions atomically bind the provider response and park the run
  in the new lease-free `AiRunState::WaitingProvider`. Transport or
  acknowledgement ambiguity closes the binding and run as
  `RecoveryRequired`; the crate never retries the create request. Public
  `ProviderBackgroundBinding`, `ProviderBackgroundSubmission`,
  `AiOpenAiBackgroundSubmission`, and the default-deny
  `AiProvider::submit_background` adapter seam expose no response content or
  retrieval/run-mutation authority. Recovery closure also appends the exact
  immutable attempt outcome in the same transaction.
- Restore preflight adds the serde-defaulted
  `invalid_provider_background_submission_count`; invalid deterministic
  identities, original fences, provider/profile/request/budget/egress/response
  bindings, output ceilings, storage/retention facts, states, or audit links
  keep runtime readiness closed. Restored `WaitingProvider` runs always require
  recovery review.

- Feature-gated `OpenAiWebhookVerifier` verifies bounded exact raw bodies using
  the profile's just-in-time webhook signing secret, OpenAI's three exact
  delivery headers, HMAC-SHA256, and a bounded replay window before minimally
  parsing JSON. It recognizes only terminal response events, redacts provider/
  profile/response identities from `Debug`, and grants no retrieval, run,
  fence, budget, egress, or completion authority.
- On SQLite/PostgreSQL, `OrmAiProviderWebhookReceiptService` atomically inserts
  one content-free private receipt and one redacted audit through generated ORM
  operations. Concurrent and later exact redeliveries are idempotent; changed
  immutable facts under the same profile/event identity conflict; unsupported
  signed events are durably ignored. Bounded whole-transaction retries absorb
  PostgreSQL serialization races without duplicating the audit. No raw body,
  signature, signing secret, prompt, output, or provider error is persisted.

- Feature-gated `OpenAiFileDeletionService` implements only exact OpenAI Files
  deletion for artifacts selected by the existing fenced attachment cleanup
  worker. It uses the fixed official endpoint with redirects disabled, resolves
  credentials just in time, validates the exact deletion acknowledgement, and
  then requires retrieval of the same file to report not found. An initial
  exact delete not-found is idempotent success; all ambiguous responses retain
  the durable reference for bounded retry.
- `AiProviderFileDeletionRequest` now exposes the validated provider family and
  exact logical provider profile owning the opaque file reference. Both the
  profile and reference remain redacted from `Debug`; the native OpenAI service
  rejects another family, profile, artifact kind, or malformed file ID before
  transport.

- Immutable pricing versions now carry deployment-supplied web-search and
  file-search microunits per completed call. `AiPricingQuoteRequest` binds a
  distinct supported built-in set and one shared provider-enforced call
  ceiling; conservative quotes reserve that many tool units at the greatest
  enabled rate. The crate embeds no provider price and performs no network
  price lookup.
- `AiProviderBuiltinUsage` exposes only authoritative counts derived from exact
  normalized start/completion pairs. Requested-but-unused built-ins cost zero;
  unknown, duplicate, unmatched, over-limit, or incomplete pairs fail closed.
  Concrete settlement charges exact completed web/file-search counts while
  code-interpreter and image-generation units remain unsupported.
- `ModelRequest::maximum_builtin_tool_calls` is required when provider
  built-ins are enabled. The native OpenAI adapter sends it as
  `max_tool_calls`, the executor enforces an independent deployment-owned
  built-in bound, and the opaque budget proof now binds the reserved tool-unit
  ceiling before transport.

- `AiOperationalTelemetrySink`, `AiOperationalTelemetry`, and typed provider,
  durable run, application/internal tool, expired-run recovery, retention, and
  restore observations. The public vocabulary contains only reviewed enums,
  counts, durations, booleans, provider family, and a fresh telemetry-only
  operation ID; it cannot carry prompts, output, tool arguments/results,
  GraphQL documents, principal/durable resource IDs, model/profile names,
  provider response IDs, endpoints, credentials, or arbitrary error text.
- Provider observations expose the stable OpenTelemetry `chat` operation and
  only well-known native provider values. Profiled compatible endpoints,
  Ollama, and local harnesses require an independently reviewed host mapping.
  Retention and restore projections deliberately discard opaque cursors,
  fingerprints, issue/resource text, and durable IDs. The synchronous,
  infallible sink contract makes exporter outage/backpressure nonauthoritative;
  implementations must enqueue promptly and may drop under bounded pressure.

- Deleting-session retention now CAS-tombstones bounded protected principal-
  inbox payloads before message content while retaining their monotonic stream
  rows and reports the exact count. After session events, inbox payloads,
  context summaries, proposal/tool/approval payloads,
  attachment objects/metadata, message content, and immutable coordinator
  checkpoints are independently proved exhausted, a final state-machine
  transaction rechecks the current retention cutoff, complete bounded terminal
  run set, zero current checkpoint pointers, exact retained message tombstones,
  and absence of protected/external rows before redacting the session title and
  transitioning the shell from `deleting` to `deleted` with a redacted audit.
- `AiSessionRetentionLimits::with_inbox_event_limit` supplies an independent
  per-session bound. `AiSessionRetentionReport` adds
  `deleting_session_inbox_payloads_purged` and
  `deleting_sessions_finalized`; neither is a whole-database erasure claim.

- `OrmAiContextCompactionService` prepares only a contiguous, bounded prefix
  segment under a renewed running lease and fresh owner/scope authority. The
  sensitive request binds an optional prior protected summary, exact message
  and block identities, opened content, a domain-separated SHA-256 source
  hash, fixed `Restricted` provenance, and a configured recent-message tail.
  `AiPreparedContextCompaction` exposes the exact redacted source set and size
  estimates needed by the ordinary provider-call plan; it grants no egress or
  budget authority.
- Context persistence accepts only the private `AiProviderCallResult` produced
  by the exact prepared request, provider/model, run fence, committed budget,
  and `context_compaction` model-inference manifest. It rejects tools,
  built-ins, reasoning, citations, unknown events, empty/oversized summaries,
  source drift, parent races, and checkpoint lookahead overflow. The final
  state-machine transaction re-proves every parent/message/block before
  protecting and inserting the summary, its direct provenance, and provider
  evidence.
- `load_latest` rehydrates authority, renews the fence, opens only the latest
  valid checkpoint, and validates its exact prefix/parent/provenance envelope.
  Prepared and loaded values redact source/summary content from `Debug`.
- Restore preflight adds `invalid_context_checkpoint_count`; nonzero invalid
  coverage, protection, lineage, provider/budget, or retention state keeps the
  runtime start gate closed.

- Deleting-session retention now proves the complete attachment-artifact set
  under an independent lookahead bound, requests artifact cleanup before its
  parent attachment, and physically removes only fully tombstoned artifact
  metadata. Artifact blobs, protected derivatives, and provider references
  stay intact while cleanup is pending or ambiguous; parent attachment cleanup
  and linked message scrubbing cannot start early.
- `AiProviderFileDeletionService` receives only an exact provider reference
  selected by a current-policy, cutoff-checked, generation-fenced artifact
  claim. `Ok(())` must mean authoritative absence; expiry, an unconfigured
  service, or an ambiguous provider result enters capped retry backoff without
  clearing metadata. `AiProviderFileDeletionRequest` redacts that reference
  from `Debug`.
- `AiAttachmentCleanupReport` now distinguishes artifact candidates, cleaned
  tombstones, races, and failed absence proofs. `AiSessionRetentionLimits` adds
  an independent attachment-artifact bound, while `AiSessionRetentionReport`
  counts artifact cleanup requests and metadata deletion. Cleanup processes
  artifacts before parent attachments and appends only redacted audit facts.

### Changed

- Consolidated into the `graphql-orm` workspace, replaced internal Git pins
  with workspace path dependencies, and aligned storage with
  `graphql-orm-storage` 0.6.0. The crate advances to 0.58.0 for the pre-1.0
  dependency identity boundary; schema 0.51.0 remains unchanged.
- `ModelRequest::validate` now rejects the reserved
  `ModelBuiltinTool::FileSearch` raw `store_ids` shape. A syntactically valid
  provider vector-store ID is not durable creation, ownership, scope,
  retention, cost, or deletion authority. Existing inline attachment input,
  exact provider-file deletion, rule vocabulary, and immutable per-search-call
  pricing remain available independently.
- The dependency universe now resolves internal packages through workspace
  paths and pins external `agql-auth` 0.12.0 at the peeled `v0.12.0` target
  `3f3b0c5365adfbe436514a681d977b600991b797`. ORM 0.16.0 adds the reviewed
  generated resolver-operation descriptor/catalog API while retaining the
  earlier PostgreSQL introspection and bounded-mutation fixes. No AI entity,
  GraphQL SDL, or stored-data migration changes.
- Ordinary CI jobs now resolve the exact upstream revisions from the public
  manifest instead of checking out unused sibling worktrees. The SemVer job
  now resolves the reviewed exact Git dependencies recorded independently by
  the current and baseline manifests; it no longer rewrites a historical
  baseline to hard-coded local path dependencies.
- Repository ownership is now explicit and unconditional: every upstream
  implementation request must be staged as a copy-ready `.handoffs/` prompt
  for a separate owning agent. Agents working in this repository may inspect
  upstream state and consume reviewed final SHAs but never mutate an upstream
  worktree or branch.

- Schema module `0.46.0` adds deterministic receipt/profile/event-kind/time
  bindings to the existing private webhook receipt placeholder and combines
  its deterministic UUID with the existing provider family as private key
  metadata for atomic insert-if-absent. Supported events
  remain `pending_reconciliation`: this release does not submit background
  responses, retrieve provider output, bind receipts to runs, settle usage, or
  mutate run state.
- Restore snapshot facts add `invalid_provider_webhook_receipt_count`; any
  malformed deterministic identity, exact provider/profile/event/response
  binding, signature fact, lifecycle state, or creation-audit linkage is fatal
  to restored-runtime readiness. The field defaults only when decoding legacy
  serialized facts; the changed module fingerprint remains independently
  fail-closed.

- Schema module `0.45.0` adds nullable provider-family and logical-profile
  bindings to private attachment artifacts. A provider reference is valid only
  when both bindings are present, and successful cleanup atomically clears all
  three before metadata can be deleted. Existing artifacts without provider
  references need no row rewrite; legacy provider references without exact
  ownership bindings remain fail-closed until trusted closed-runtime
  reconciliation.

- Schema module `0.44.0` adds defaulted nonnegative web/file-search rate
  columns to the append-only private pricing catalog. Built-in rate management
  is independently disabled until the host calls
  `AiPricingCatalogManagementLimits::with_maximum_builtin_tool_microunits_per_call`.
  Restore validation must treat invalid rates or route/audit bindings as fatal.

- Session queries now filter lifecycle state at the generated ORM boundary so
  `deleting` and finalized `deleted` shells do not consume visible pagination
  windows. Repeated delete requests remain idempotent after finalization.
- Private session-state and inbox-session filters are explicit generated ORM
  metadata. Schema module `0.43.0` makes the private inbox protected payload
  nullable and adds its nullable purge timestamp plus a defaulted CAS version;
  it adds no table, index, constraint, or entity. Existing payloads remain
  retained without an application-authored row rewrite. Eligible payload/title
  redaction and the new terminal `deleted` state occur only through retention.

- Ordinary message retention now physically deletes every context checkpoint
  whose prefix could cover an eligible message before scrubbing that message
  in the same transaction. A one-row lookahead proves the complete checkpoint
  set; over-bound sets block the message without partial invalidation.
  `AiSessionRetentionReport::context_checkpoints_invalidated` reports the exact
  rows removed.
- The private context-checkpoint primary key is now explicitly supplied by the
  trusted writer so its content-protection associated identity is fixed before
  persistence. This changes generated private metadata but adds no table,
  column, index, constraint, or public GraphQL operation.

- `AiAttachmentArtifactRecord` adds nullable cleanup state, generation, lease,
  retry, and backoff fields plus stable created-time/ID keyset ordering. The
  provider reference is now redacted from generated backup descriptors.

- The generated-ORM retention worker now physically deletes bounded,
  age-expired `provider_turn_persisted`, `tool_batch_persisted`, and
  `supervised_tool_batch_persisted` checkpoints only when their run is
  terminal and no current run pointer references them. The database-enforced
  append-only purge transaction re-proves the current scope policy, exact
  attempt/outcome fence, committed budget, and either a durable final assistant
  output or every correlated terminal tombstoned tool/approval dependency.
  Current checkpoints, live/nonterminal runs, missing history, untombstoned
  tool authority, lookahead overflow, and malformed correlations remain closed.
- `AiSessionRetentionReport` now counts expired protected checkpoints and raw
  checkpoint proofs that remained blocked. Deletion is deterministic,
  cardinality-exact, atomically redacted-audited, and never opens protected
  checkpoint state.

- `raw_payload_retention_seconds` now drives age-based tool/approval payload
  tombstones for active, archived, or pre-cutoff deleting sessions. The worker
  selects only calls whose completion time has expired and whose owning run,
  exact application-tool step, and optional one-shot approval are terminal and
  state-compatible. Newer or live runs/calls/approvals remain untouched and do
  not prevent an independent expired terminal subset from being scrubbed.
- `AiSessionRetentionReport` now counts expired tool and approval payloads plus
  sessions whose complete raw-payload lookahead proof exceeded a deployment
  bound or was malformed. Provider adapters continue to normalize responses
  without persisting raw HTTP envelopes. Protected coordinator state remains a
  separate retention dependency and is not claimed by this slice.

- Deleting-session retention now tombstones protected tool arguments/results
  and approval resource bindings/action previews after proposal content and
  before attachment or message cleanup. It first proves the complete bounded
  run/call/approval set, terminal runs and tool states, exact call/step and
  one-shot approval linkage, compatible terminal approval state, and intact
  pre-purge payload shape. Active or uncertain authority remains blocked.
- `AiSessionRetentionLimits::with_tool_payload_limits` and its call/approval
  getters configure independent whole-session proof bounds. The report now
  counts tool and approval payload tombstones plus sessions blocked by bounds,
  nonterminal work, or inconsistent authority. Tool/approval IDs, hashes,
  state, authorization/egress evidence, audit references, use counts,
  timestamps, and row versions remain as non-content metadata. Coordinator
  checkpoint maintenance independently re-proves these tombstones before
  physically purging an append-only checkpoint page.

- Deleting-session retention now tombstones protected proposal and optional
  proposal-item content only after context summaries are exhausted, the whole
  proposal/item set fits configured lookahead bounds, every owning run is
  terminal, and every proposal is rejected, applied, expired, or an expired
  pending review. Expired pending reviews become durably `expired`; accepted or
  accepted-edited proposals remain blocked until a trusted application outcome
  is recorded. Attachment cleanup and message scrubbing wait for a later pass.
- `AiSessionRetentionLimits::with_proposal_limits` and proposal/item getters
  configure independent whole-session proof bounds. The report now counts exact
  proposal payload tombstones and deleting sessions blocked by bounds,
  nonterminal runs, or an unresolved accepted outcome. Proposal identity,
  schema, review decision, creator/reviewer, application outcome/audit links,
  timestamps, and row versions remain as non-content metadata.

- Deleting-session retention now moves bounded, artifact-free attachment rows
  into a dedicated cleanup state only after reloading the exact current scope
  policy and proving `deleted_at + deleted_content_purge_seconds`. The existing
  attachment cleanup worker re-proves that authority in its claim transaction,
  deletes only exact opaque blob references, verifies absence, and retains
  ambiguous operations for bounded retry. A later retention pass physically
  deletes only fully cleaned ordinary metadata before linked message content
  can be scrubbed.
- `AiSessionRetentionLimits::with_attachment_limit` and
  `maximum_attachments_per_session` configure the independent whole-session
  attachment proof bound. `AiSessionRetentionReport` now distinguishes cleanup
  requests, physically deleted metadata, and sessions still blocked on bounds,
  storage ambiguity, in-flight cleanup, or artifact/provider-file lifecycle.
  Any attachment artifact keeps both its parent attachment and linked message
  closed until the separate artifact/provider-file retention contract exists.

- Deleting-session retention now physically purges bounded pages of immutable
  coordinator checkpoints after protected events, context summaries, and
  eligible message content are exhausted and every bounded run is terminal.
  The ordinary state-machine transaction first validates each current
  checkpoint and clears terminal run pointers; a separate generated-ORM
  retention transaction then re-proves the cutoff and dependencies, deletes an
  exact typed ID set, and appends a redacted audit atomically. Checkpoint pages
  use stable created-time/primary-key ordering and a least-privilege projection
  that excludes protected state.
- `AiSessionRetentionLimits::with_run_checkpoint_limits` and its two getters
  configure independent run-proof and checkpoint-page bounds. Existing
  constructors remain source-compatible and conservatively reuse their
  existing message/context limits.
- `AiSessionRetentionReport` reports cleared checkpoint references, physically
  deleted checkpoints, and sessions whose checkpoint purge remains blocked by
  a nonterminal or over-bound run set.

- Deleting-session retention now removes protected context-summary checkpoints
  in independently bounded pages before it can scrub any message content.
  `AiSessionRetentionLimits::new_with_context_checkpoints` configures that
  independent hard bound, while the existing constructor safely reuses its
  message-row bound.
- `AiSessionRetentionReport::deleting_session_context_checkpoints_deleted`
  reports exact summary rows removed by one pass. The cutoff test proves
  context-first ordering across multiple one-row transactions, no early
  message scrub, same-transaction audit, and final replay idempotency.

- `OrmAiSessionRetentionService` now applies the current
  `deleted_content_purge_seconds` policy to exact `deleting` sessions. After
  the cutoff, bounded passes remove every protected session-event kind and
  scrub eligible terminal unattached message previews/blocks even when
  ordinary message retention is disabled. Session/message metadata,
  attachments, external content, and append-only audit/usage/fence facts stay
  durable; the service does not claim complete erasure.
- `AiSessionRetentionReport::deleting_session_events_deleted` distinguishes
  deletion-cutoff event removal from ordinary expired live-delta pruning.
  Focused tests cover the pre-cutoff boundary, repeated bounded scheduling,
  message-retention opt-out, atomic audit, tombstones, and idempotency.

- `OrmAiApprovalWaitReconciliationService` and bounded deployment controls for
  live `WaitingApproval` runs. The worker rehydrates the current principal,
  validates the exact provider-turn/checkpoint/budget/tool/step/approval fence,
  and applies current `AiApprovalWaitReconciliationPolicy` before leaving a
  pending or approved wait parked. Denied, revoked, expired, deployment-cutoff,
  deleted-session, or policy-cancelled waits close atomically with a protected
  session event, redacted audit, immutable attempt outcome, cleared run fence,
  and terminal call/step state. Malformed linkage moves only the run to
  `RecoveryRequired` and does not alter potentially unrelated approval or tool
  authority.
- `AiApprovalWaitPolicyContext`, `AiApprovalWaitPolicyDecision`,
  `AiApprovalWaitReconciliationLimits`, and
  `AiApprovalWaitReconciliationReport`. Policy evidence can only retain or
  cancel a parked wait; it grants no approval consumption, resolver, provider,
  or replay authority.

- `AiSupervisedAgentCoordinator` and its exact planner/service seams. A
  provider-retained turn may finish normally or request exactly one registered
  `SupervisedWrite`/`OneShot` mutation. The provider result is checkpointed
  before a server-previewed approval is staged, the worker stops during the
  human wait, and a one-owner approved claim executes through the ordinary
  resolver before its protected result is consumed once for the next provider
  turn. Sequential approved mutations may repeat within hard loop/rule limits.
- `AiSupervisedAgentCoordinatorLimits`, `AiSupervisedAgentTurnPlan`,
  `AiSupervisedAgentRunOutcome`, `AiSupervisedApprovalWait`, and the
  `AiSupervisedAgentTurnPlanner`, `AiAgentSupervisedApprovalStager`,
  `AiAgentSupervisedCheckpointControl`, and
  `AiAgentSupervisedResumeExecutor` boundaries. Opaque durable proofs remain
  distinct from provider, approval, egress, budget, or resolver authority.

- `OrmAiCoordinatorCheckpointService::adopt_supervised_tool_batch`, the opaque
  `AiAdoptedSupervisedToolBatch`, and proof-consuming
  `consume_supervised_before_provider`. Expired-lease recovery may now requeue
  one exact completed provider-retained supervised mutation under a new
  attempt/generation; adoption never executes the mutation and the checkpoint
  is atomically cleared before any later provider transport.
- `AiRestoredCoordinatorCheckpoint` lets trusted snapshot fact producers
  distinguish no checkpoint, a validated read-only batch, and a validated
  approval-bound supervised batch. A confirmed external mutation is requeued
  only for a running snapshot with the exact supervised classification and a
  provider continuation; uncheckpointed/uncertain effects and human-wait
  states remain recovery-only.
- `OrmAiSupervisedResumeService` now reopens the exact protected provider turn
  behind a one-owner approved-wait claim, revalidates current principal,
  policy, hierarchical rules, provider budget, approval, tool, and route
  bindings, executes one approved resolver through the ordinary GraphQL path,
  and protects its exact provider-retained continuation without making a
  second provider call. The first contract is deliberately one mutation and
  one provider-retained response; stateless/multi-call resume stays closed.
- `AiAdoptedSupervisedProviderTurn`, `AiProtectedSupervisedToolBatch`, and
  `AiSupervisedResumeOutcome` provide opaque proofs for the pre-execution and
  post-mutation handoffs. A failed post-side-effect checkpoint is durably
  classified `RecoveryRequired` and is never replayed.
- `AiProviderCallPlan::project_supervised_rule_usage` binds immutable
  plan-time tool maturity/approval evidence and checks exact current
  hierarchical tool/provider/capability/disclosure/retention/BYOK/budget
  constraints before supervised provider egress. It is narrowing evidence,
  not provider or resolver authority.
- `OrmAiRunService::claim_next_approved` and the opaque
  `AiApprovedRunClaim`. Exactly one worker can atomically adopt an approved,
  unconsumed wait without creating a second provider attempt: approval state,
  run state, worker owner, expiry, heartbeat, row-version fence, and a redacted
  immutable audit fact change together. Current principal, rules, preview,
  approval consumption, resolver authorization, and egress remain mandatory.
  Expired approved rows in the bounded scan are atomically expired and audited
  instead of permanently starving newer eligible handoffs.
- `OrmAiCurrentRuleResolver` and mandatory rule evidence on each read-only
  coordinator plan. The adapter rehydrates the exact lease principal and
  resolves the complete hierarchy twice; the coordinator re-resolves before
  provider egress, after provider return, and before every application tool.
  A changed fingerprint fails before transport when possible and otherwise
  durably requires recovery.
- Protected coordinator checkpoint v2 binds the exact rule fingerprint and
  authoritative cumulative provider-call, provider/tool-step, elapsed-time,
  output-token, cost, tool-unit, and image-unit usage. Estimates are checked
  before egress, actual committed provider usage is checked afterward, and
  adoption reopens the checkpoint and re-resolves the current hierarchy.
- Project-neutral hierarchical rule contracts and a generated-ORM-only
  `OrmAiRulePolicyService`. A host-authored current-principal lineage intersects
  immutable deployment limits and every explicit application-defined scope
  across enabled state, disclosure/maturity, exact tool fingerprints,
  providers/capabilities, approval floors, retention/BYOK, and seven budget
  dimensions. Missing, cross-tenant, corrupt, unauthorized, stale, or widening
  layers fail closed, and the resolved fingerprint grants no ordinary
  authority.
- Separately composable authenticated `AiRuleQueryRoot` and
  `AiRuleMutationRoot` management with exact-scope access decisions, recent
  MFA, compare-and-swap updates, strict v1 checksummed persistence, and atomic
  redacted audit. Restore facts now classify an invalid hierarchical rule as
  fatal start-gate evidence.
- A protected `OrmAiSkillCatalogService` and separately composable GraphQL
  skill roots. Safe metadata, immutable publication, and enablement require
  exact host scope policy, recent MFA, compare-and-swap state, and atomic
  redacted audit. Instructions are protected before persistence; strict v1
  metadata binds exact tool and UI-intent descriptor fingerprints,
  classification/maturity ceilings, schemas, provider capability requests,
  proposal types, activation, and hard per-run limits. Resolution reopens and
  checksum-validates current versions but grants no authority.
- A project-neutral `AiUiIntentCatalog` with bounded logical type IDs, JSON
  Schema 2020-12 payload contracts, exact descriptor fingerprints, bounded
  display metadata, and exact skill bindings. Validated intents are suggestions
  only; the crate contains no route, URL, component, callback, or navigation
  implementation.
- `AiUiIntentDeliveryService` and `OrmAiUiIntentDeliveryService` for durable
  provider-produced logical suggestions. Delivery accepts one strict visible
  JSON envelope from an exact completed tool-free provider result, validates
  its registered type/schema/fingerprint, rehydrates current scope/session
  authority before and after protection, proves exact committed usage, and
  atomically appends protected session and principal-inbox events, redacted
  audit, and a renewed run fence. Exact retries are idempotent and still grant
  no route, resource, or navigation authority.

- A feature-gated, explicitly profiled OpenAI-compatible Responses/SSE
  adapter. Construction fixes one normalized endpoint, provider-profile ID,
  retention declaration, secret reference, timeout, and reviewed capability
  set. Every transfer must reproduce the exact profile, destination, model,
  and retention binding. Redirects, runtime/model-selected URLs, capability
  probing, attachments, built-ins, and undeclared tools/structured output or
  retained continuation fail closed.
- Typed GraphQL-managed OpenAI-compatible provider configuration. A compatible
  profile must declare its retention label and exact tool, parallel-tool,
  structured-output, and retained-continuation capabilities; other provider
  kinds reject that nested contract. The redacted view can construct the safe
  transport configuration only when the profile is enabled and complete.
- A PostgreSQL parity integration test that creates and owns its disposable
  Docker container, random credentials, unique database, and loopback port.
  It applies the generated AI module and exercises atomic session/message/run,
  protected skill publication/resolution, hierarchical rule
  management/resolution, keyset, and stale-fence behavior
  entirely through `graphql-orm`, then verifies the ownership label before
  cleanup. CI never accepts a database URL.

- A feature-gated native xAI/Grok Responses/SSE adapter fixed to the official
  HTTPS endpoint. It resolves Bearer credentials immediately before transport,
  supports bounded text/JSON, JSON-schema output, and strict custom/parallel
  application tools, and requires xAI's zero-data-retention response
  attestation by default. Ordinary retention is an explicit opt-out requiring
  separate egress authorization. Retained response-ID continuation additionally
  requires `store_responses`, the exact provider-retention proof, and ZDR
  verification to be disabled. Attachments, xAI server tools, encrypted
  reasoning replay, and arbitrary endpoints remain closed.

- A feature-gated native Anthropic Messages/SSE adapter fixed to the official
  HTTPS endpoint and API version. It resolves API keys just before transport,
  requires exact egress and atomic budget proofs, and supports bounded
  streaming text/JSON, strict custom and parallel application tools,
  protected stateless continuation, and JSON-schema structured output.
  Provider-retained continuation, attachments, provider built-ins, extended
  thinking, and prompt-cache creation remain fail-closed. Anthropic cache-read
  tokens are included in total input and retained as the cached subset; an
  unexpected cache write is rejected because its distinct billing class is
  not yet represented by the authoritative pricing ledger.

### Security

- Run checkpoints are the only append-only AI entity opted into retention
  purge. Pricing, skill-version, usage, audit, egress, run-attempt, and attempt-
  outcome facts remain non-purgeable. The trusted worker installs an exact
  `RetentionMaintenance` entity policy only on its cloned database handle;
  ordinary update/delete paths remain prohibited and row policy still narrows
  access.
- Pointer clearing commits before physical deletion. A crash between phases
  leaves an unreferenced checkpoint for a later bounded pass, never a dangling
  run pointer. Nonterminal runs, malformed bindings, excessive run sets,
  retained protected sources, or policy/cutoff drift keep checkpoints in
  place.
- Session/message/run CAS conflicts now leave the state-machine transaction by
  error, forcing rollback before the worker converts them into a bounded
  `sessions_conflicted` report. Earlier event/context/content or pointer
  changes can no longer commit without their same-transaction audit.

- A deleting-session message body is not scrubbed while any bounded page of
  protected context summaries remains. The worker validates each checkpoint's
  session/sequence/provider/hash metadata, deletes only exact ORM rows, and
  requires a later transaction to scrub messages, preventing retained
  summaries from outliving content they may cover.

- Deleting-session content pruning reloads and validates the exact current
  scope policy and the `deleting`/`deleted_at` invariant in each state-machine
  transaction. It never opens protected payloads, retains unsafe attached or
  nonterminal message content, uses checked cutoff arithmetic, and records a
  redacted audit in the same transaction as each bounded change.

- Approval-wait reconciliation is a bounded live-runtime pass that must run
  before generic expired-lease recovery. It never polls or heartbeats a human
  wait, infers approval, claims approved work, consumes approval, executes a
  resolver, or invokes a provider. Snapshot-restored `WaitingApproval` and
  `WaitingTool` runs remain recovery-only and are never automatically resumed
  by this service.
- Generic expired-lease recovery no longer selects `WaitingApproval`; leaving
  that state parked is now owned solely by the current-principal/current-policy
  reconciler and its hard deployment cutoff. Other expired waiting states
  remain conservative recovery cases.
- Cancellation is fenced by exact row snapshots and revalidates the immutable
  provider-turn checkpoint hash, committed one-run budget, owner/scope/tenant,
  principal fingerprint, and unique staged call. Concurrent decisions are CAS
  races, while malformed linkage preserves approval/call rows for operator
  evidence and closes the run as `RecoveryRequired`.

- The supervised coordinator accepts only provider-retained, supervised-only
  plans and exactly one mutation request per turn. It re-resolves current
  hierarchical rules before transport, after provider return, before approval,
  before checkpoint consumption, and again after consumption. Provider,
  checkpoint, approval-staging, output, and mutation ambiguity close as
  `RecoveryRequired`; an ambiguous approved resolver is never re-entered.
- Loop capacity is checked before consuming an adopted checkpoint and before
  staging a mutation that would require another provider turn. This preserves
  retry evidence and prevents a human from approving a mutation whose result
  cannot be returned within the configured provider-turn ceiling. The same
  pre-consumption capacity check now protects read-only continuation loops.
- `AiSupervisedResumeOutcome::RecoveryRequired` now retains authoritative
  provider-turn and tool-call counts, and the enum is non-exhaustive. This lets
  the top-level coordinator report durable ambiguity without reconstructing
  counters or provider state.

- Read-only checkpoint append/adoption now structurally requires
  `risk = read_only` and no approval ID for every current and stateless-history
  tool row. Consequential results use the distinct supervised checkpoint kind,
  whose append transaction verifies an exact consumed one-shot approval.
  Neither kind can be substituted for the other. A supervised checkpoint now
  additionally protects the exact approval binding, canonical-preview hash,
  policy version, and authorization-state digest before cross-generation
  adoption; any mismatch fails before continuation consumption.
- Approved-wait handoff preserves the original attempt/generation bindings but
  replaces owner and row-version proof, immediately fencing the staging
  worker. `approved` becomes `resume_claimed` and `WaitingApproval` becomes
  `WaitingTool`, so concurrent or later workers cannot claim the same action.
  A crash before consumption remains externally unexecuted but closes through
  conservative waiting-tool recovery; mutation replay is never inferred.
  Snapshot restore now sends `WaitingApproval` and `WaitingTool` to
  `RecoveryRequired` even when the coarse external-effect flag says none.
- Rule-bound planning rejects provider families/capabilities, disclosure
  classification, retention, BYOK use, tool fingerprints/maturity/approval,
  or cumulative budgets outside the exact resolved intersection. These checks
  are additive: atomic budget, egress, provider-profile, current tool policy,
  resolver authorization, and one-shot approval remain independently required.
  Restore now treats malformed or legacy coordinator rule bindings as fatal.
- Hierarchical rules cannot expose secret classification or autonomous writes,
  cannot widen immutable deployment ceilings, and cannot substitute for fresh
  resolver, tool, egress, provider, budget, or approval authorization. Empty
  allowlists and zero budgets explicitly deny; absent values only inherit
  already-effective bounds. Runtime lineage validation rejects duplicate,
  incomplete, over-depth, cross-tenant, and wrong-target hierarchies.
- Skill discovery, enablement, and resolution cannot widen current tool,
  resolver, egress, provider, budget, proposal, approval, or UI policy.
  Unknown stored fields/formats, duplicate bindings, swapped scope/current
  version/provenance, checksum mismatch, stale UI-intent fingerprints, and
  schema-invalid intent payloads fail closed. Restore facts now classify an
  invalid skill catalog or UI-intent event pair as fatal start-gate evidence.
  UI-intent delivery additionally rejects reasoning, tool/built-in/citation/
  unknown events, malformed event order, mismatched usage/response identity,
  missing budget proof, stale authority, and stale fences.

- Compatible endpoints remain behind the deployment-owned endpoint policy;
  URL syntax validation and redirect denial do not claim DNS-rebinding or
  network-isolation protection. Legacy compatible rows without the new typed
  contract remain visible but cannot construct a compatible adapter until an
  authorized administrator re-saves them with a reviewed contract.

- Responses adapters now require an SSE content type and normalize a built-in
  result only when the exact built-in kind was present in the server-authored
  request. Streams also require exact model/response/status/usage identity,
  bounded event/text/tool-call state, and an unambiguous terminal completion.
  This closes unsolicited built-ins, swapped models, malformed accounting, and
  truncated success for both OpenAI and xAI.

- Provider-independent bounded stateless application-tool continuation through
  `ModelContinuationMode`, protected `StatelessConversation` history, exact
  assistant/tool identity, and domain-separated continuation-chain hashes.
  Every historical and current tool output requires its own unique freshly
  authorized `ToolResult` manifest; hidden thinking, attachments, provider
  built-ins, arbitrary roles, and model-authored instructions cannot enter the
  replay format.
- Native Ollama custom and parallel application-tool calls with exact
  provider-name/local-ID/fingerprint mapping, native message/tool-result
  replay, bounded normalized calls, and no provider-retained response state.
- Installed local-harness JSON-lines protocol v2, including exact stateless
  history/tool definitions and a start/delta/complete tool-event state machine
  restricted to server-offered IDs. Registrations may opt into custom tools
  only together with stateless continuation; filesystem, network, shell,
  credential, built-in, and provider-retained authority remain unavailable.
- Protected stateless tool-batch checkpoints and cross-generation adoption
  using the existing generated ORM entities. A replacement fence can continue
  only after reopening the protected conversation and proving every historical
  and current tool call against its original attempt/generation, committed
  budget, run step, protected arguments/result, disclosure classification,
  immutable egress decision, and unique replay manifest. Resolver calls are
  never rerun during adoption, and any missing, swapped, duplicated, tampered,
  or newly denied evidence remains closed for recovery.

- A host-only `AiSessionRetentionService` and generated-ORM
  `OrmAiSessionRetentionService` for bounded keyset scan cycles. Each session
  transaction reloads its exact current GraphQL-managed scope policy, deletes
  only expired provisional `provider_live_delta` events, and scrubs protected
  preview/block content only from finalized messages whose producing run is
  terminal and which have no linked attachment. Nonterminal, attached,
  corrupt, unconfigured, or concurrently changed state fails closed.
- Explicit retained-message tombstones through `AiMessageView::content_purged`.
  Authorized message windows retain metadata and a fixed server-authored
  preview, while block reads return an empty window without opening protected
  data. Session-event windows now signal `reset_required` when selective delta
  retention creates a sequence gap.
- Same-transaction redacted retention audit, bounded/idempotent SQLite tests,
  and fatal restore evidence for inconsistent message tombstones or retention
  gap classification.

- An append-only, GraphQL-managed immutable pricing catalog with exact
  scope/provider/model bindings, globally unique version references,
  integer-only fixed/input/cached-input/output token rates, deployment hard
  bounds, per-route version caps, recent MFA, separate host read/write
  decisions, and same-transaction redacted audit.
- `OrmAiPricingService` as both a conservative exact-version preflight quote
  service and authoritative token-only provider usage accountant. Cached input
  is priced as a subset, estimates assume non-cached input, arithmetic is
  checked and rounded conservatively, and provider built-ins fail closed until
  authoritative billable-unit catalogs are implemented.
- Authoritative provider usage observations now retain the exact application
  scope from their budget plan, so settlement rejects cross-scope pricing
  references as well as provider/model/version swaps.
- Fatal restore evidence for corrupt immutable pricing references, scope/route
  bindings, rates, or creator-audit linkage.

- Authenticated GraphQL budget-policy reads and recent-MFA-protected CAS
  create/update/enable/disable through the existing configuration service.
  Deployment management limits cap every configurable dimension and policies
  per exact scope; scope/principal/interval bindings are immutable, and every
  mutation appends a redacted audit fact in the same transaction.

- An append-only authoritative usage ledger written exactly once in the same
  state-machine transaction that commits a budget reservation. Each fact has a
  unique reservation binding, exact scope and principal kind/subject,
  provider/model, total and cached input tokens, output/tool/image units, and
  settled cost; idempotent reconciliation cannot duplicate it.
- Authenticated `aiUsage`/`AiUsage` reporting through the ordinary query root
  and a separately composable usage root. Host policy grants either exact
  current-principal or exact-scope visibility, and reads use bounded
  bidirectional keysets with provider/model and bounded time filters without
  exposing prompts, transcripts, tool content, pricing rules, or counter
  internals.
- Public `ModelRequest::conservative_egress_bytes` so host planners can bind
  manifests to the same complete metadata and attachment-encoding ceiling the
  provider boundary enforces.

- An optional installed local-harness foundation with immutable
  deployment-owned logical-model registrations, fixed absolute executable and
  arguments, mandatory executable digest/sandbox/resource contracts, a trusted
  process-launcher seam, and a bounded JSON-lines v2 provider driver. The safe
  protocol supports text/structured output plus opt-in stateless application
  tools, and its deterministic fake process suite covers fixed launch facts, environment/command
  non-injection, framing, stderr/output limits, cancellation cleanup, swapped
  budget/model proofs, secret non-persistence, and unoffered tool events.
- A native feature-gated Ollama `/api/chat` adapter for bounded NDJSON text
  streaming, exact ephemeral PNG/JPEG/WebP image input, JSON-schema structured
  output, and authoritative prompt/evaluation token usage. Deployment endpoint
  policy remains mandatory, redirects and URL credentials are forbidden, and
  local execution does not bypass exact egress or atomic budget proofs.
- Durable exact-principal cross-session inbox sequencing with protected
  lifecycle/message/assistant-output events committed atomically with their
  source state, bounded catch-up pages, receiver-before-replay subscriptions,
  lag recovery, explicit retention-gap reset, and periodic current-principal
  reauthorization.
- GraphQL-managed, recent-MFA/CAS/audit-protected scope retention settings,
  including explicit inbox age and recent-event-floor bounds. The host-only
  `OrmAiInboxPruningService` deletes only an expired contiguous prefix,
  serializes with appends through the stream CAS, never rewinds sequence heads,
  and reports missing policies or concurrent conflicts without unsafe deletion.

- Exact, provider-neutral attachment reopening through
  `AiProviderAttachmentResolver`, private-field request/resolved payload types,
  deployment raw-byte/cardinality limits, and the ORM attachment service. The
  resolver rechecks current owner/session/scope and released/clean/linked state,
  streams only the exact opaque object, verifies metadata/length/SHA-256, and
  detects durable row changes around storage I/O.
- Native OpenAI Responses image/file input using ephemeral inline data URLs.
  Supported image MIME types map to `input_image`; other accepted files map to
  `input_file` under the provider's per-request raw-file limit. No provider file
  ID, storage URL, or provider-side cleanup obligation is created.
- Host-only `AiAttachmentCleanupService` and ORM implementation for bounded
  expired-ticket, interrupted upload/removal, and orphan-reference cleanup.
  Claims use monotonic generations, expiring row-version fences, confirmed
  idempotent blob deletion, durable redacted audit, and bounded retry backoff.
- Configurable upload-processing and cleanup claim lifetimes, plus a redacted
  per-pass cleanup report suitable for deployment telemetry.
- Project-agnostic AI schema module with 39 private persistence entities for
  configuration, sessions, protected history, fenced runs, tools, approvals,
  proposals, budgets, usage, egress, audit, and restore readiness.
- Owner-isolated ORM-backed session/configuration services and resumable
  durable session-event subscriptions for SQLite/PostgreSQL.
- Provider-neutral streaming contracts, deterministic mock provider, and a
  feature-gated OpenAI Responses adapter.
- Explicit egress manifests/proofs, secret-store/content-protection contracts,
  default-deny tools, structured proposals, and restore/start gates.
- Logical local/remote GraphQL execution targets with schema/document/
  projection/disclosure bindings and no model-visible URL.
- Static recursive disclosure schemas that reject unknown, mismatched,
  oversized, secret, and structurally non-exportable result nodes.
- Atomic budget reservation domain contracts and provider-call proofs bound to
  run, attempt, fence, provider, model, output ceiling, pricing version, and
  expiry.
- ORM-backed SQLite/PostgreSQL budget service with multi-policy atomic
  reservation, principal/fence validation, bounded serialization retries,
  content-bound idempotency, exact-once reconciliation, conservative uncertain
  capacity, and in-memory concurrency tests.
- ORM-backed SQLite/PostgreSQL run service with bounded durable queue claims,
  immutable attempt/outcome history, monotonic fencing generations, renewable
  leases, bounded retries, terminal transitions, expired-lease recovery, and
  stale-worker/concurrent-claim tests.
- Append-only ORM egress-decision audit plus a security-ordered provider-turn
  executor that reauthorizes access, reserves budget, records every exact
  allow/deny decision before transport, marks calls uncertain at the transport
  boundary, bounds normalized events, and commits authoritative usage.
- Deployment-owned `AiProviderUsageAccounting` contract so an exact immutable
  pricing version settles cost/tool/image units after provider token usage;
  estimated cost is never mislabeled as authoritative actual usage.
- Fenced provider-output persistence that reauthorizes the current principal,
  resolves current content-protection policy, splits large assistant output
  into windowable protected blocks, and atomically appends the message,
  session event, and renewed run fence.
- Durable read-only application-tool execution for SQLite/PostgreSQL with exact
  registered/policy-bound model definitions, bounded normalized call IDs and
  arguments, protected pre-execution arguments and post-execution results,
  ordinary current-principal GraphQL resolver execution, static disclosure,
  separately audited tool-result egress, session events, run-step history, and
  renewed run fencing.
- `AiAgentLoopGuard` and exact `AiAgentContinuation` sequencing that bind a
  provider response, every requested `call_id`, every protected tool result,
  and its immutable egress manifest under hard provider-turn/tool-call limits.
- `AiReadOnlyAgentCoordinator` with host-owned exact turn planning, periodic
  fenced heartbeats during provider streams, bounded multi-turn/tool
  sequencing, protected final-output persistence, terminal classification, and
  conservative `RecoveryRequired` handling for ambiguous provider, resolver,
  and output handoffs.
- UTF-8-safe `AiLiveDeltaCoalescer` primitives enforcing deployment bounds no
  weaker than 50 ms or 4 KiB while excluding tool arguments and other
  structured provider events from visible live batches.
- Immutable fenced run checkpoints and `latest_checkpoint_id` recovery
  binding. Final protected assistant output and its exact redacted checkpoint
  now commit atomically; expired-lease reconciliation can safely finalize that
  proven crash window instead of misclassifying it as an uncertain replay.
- Explicit provider-response continuation and `ModelInputBlock::ToolResult`;
  the OpenAI adapter maps these to Responses `previous_response_id` and
  `function_call_output` only when provider response storage is deliberately
  enabled.
- ORM-backed protected proposal staging with current-principal/scope policy,
  schema/provenance validation, fenced creation, bounded keyset reads,
  schema-revalidated human edits, CAS review, durable session events, and
  trusted post-domain-mutation application/audit linkage.
- ORM-backed exact approval lifecycle with protected canonical previews and
  resource bindings, fenced `WaitingApproval` parking, authenticated bounded
  GraphQL reads/decisions/revocation, optional recent-MFA decisions, fresh
  original-actor rehydration, atomic one-shot consumption, session events, and
  renewed run fencing. Request and consumption also re-resolve the current
  registered supervised-mutation descriptor and exact GraphQL contract.
- Composable `AiProposalQueryRoot`/`AiProposalMutationRoot` and
  `AiApprovalQueryRoot`/`AiApprovalMutationRoot` with coherent optional
  PascalCase naming and fail-closed authentication.
- Full approval action-envelope types binding resources/versions, policies,
  actor/delegation identity, operation contracts, and server-generated
  canonical previews.
- Fresh principal/scope/descriptor/argument-aware tool authorization inside
  the authenticated bridge, JSON Schema 2020-12 argument validation, and
  disclosure-validated runtime result envelopes.
- Optional `graphql-case-pascal` feature for coherent PascalCase resolvers,
  arguments, inputs, outputs, subscriptions, and forwarded ORM fields.
- Repository governance, documentation index, README/changelog/migration
  release-policy enforcement, warnings- and missing-docs-denied CI Rustdoc
  checks, and SemVer enforcement scaffolding.
- Project-agnostic local execution design covering local HTTP model servers and
  allowlisted native/ACP subprocess harnesses without arbitrary shell,
  environment, filesystem, network, or tool authority.
- Explicit supervised provider-plan constructors accepting only enabled exact
  read-only tools and `SupervisedWrite` application mutations with one-shot
  approval and non-secret consequential risk classes.
- `AiCanonicalActionPreviewBuilder`, `AiToolPreauthorization`, and
  `OrmAiConsequentialToolCallService` for server-owned current-state previews,
  protected approval staging, exact consumption, freshly policy-bound ordinary
  resolver execution, protected results, separate egress, and fenced outcomes.
- Durable consequential tool-call bindings for provider/model/response,
  settled budget reservation, correlation/causation, and safe delegation
  references so approval execution can be rebuilt after an interactive wait.
- A provider-neutral `AiRemoteAuthenticatedGraphqlAdapter` for private routed
  or direct GraphQL targets, plus redacted exact delegation requests,
  deployment authority-issuer and transport seams, logical-route conformance
  fixtures, short-lived secret handling, and current-principal freshness
  limits without embedding a router or federation product.
- `OrmAiCoordinatorCheckpointService` and `AiAgentCheckpointWriter` for
  protected, size-bounded, freshly authorized, fenced provider-turn and exact
  completed-tool-batch checkpoints. The same transaction verifies committed
  provider usage and every protected/egress-audited tool row before advancing
  the run's latest checkpoint.
- `AiAgentCheckpointAdopter` and opaque `AiAdoptedReadOnlyToolBatch` proofs for
  current-authority cross-generation adoption of exact completed read-only
  tool batches. Adoption reopens protected arguments/results, validates the
  original budget, tool, step, disclosure and egress records, reconstructs the
  bounded loop guard, and atomically consumes the checkpoint before transport.
- `AiLiveDeltaSink`, its private-field exact persistence context, and
  `OrmAiLiveDeltaService` for optional protected durable provisional model
  output. The provider executor coalesces only visible text and reasoning
  summaries, applies sequential persistence backpressure, and appends a
  cursor-addressable session event only after fresh authority, scope,
  protection-policy, run-fence, and uncertain-budget validation.
- Owner-isolated attachment intake built on the exact reviewed
  `graphql-orm-storage` `BlobStore`: bounded one-time upload tickets,
  current-owner streaming upload, random scope-bound quarantine/final keys,
  exact byte/hash attestations, trusted full-object scanning, separate
  fail-closed host acceptance policy, conditional promotion, protected durable
  events, bounded metadata reads, finalization, and unlinked-object removal.
- Composable `AiAttachmentQueryRoot`/`AiAttachmentMutationRoot` with coherent
  optional PascalCase naming. Large bytes never pass through GraphQL JSON and
  raw upload tokens, token hashes, blob keys, checksums, and scanner internals
  are not returned in ordinary attachment views.

### Changed

- Budget policies now carry an indexed deterministic non-secret `scope_key`.
  Runtime reservation queries cover the exact tenant scope plus an explicitly
  tenant-wildcard scope, validate every stored binding, and remain default-deny
  for absent, corrupt, or excessive policy sets.
- `ai_scope_key` is now backend-neutral and available to schema-only/MSSQL
  builds as well as SQLite/PostgreSQL migration and configuration code.
- The crate root no longer accidentally reexports macro-generated private ORM
  record inputs, filters, mutations, subscriptions, or repositories. Supported
  public persistence API remains `AiSchemaModule` and its module constants;
  consumers continue through authenticated service/GraphQL contracts.

- Provider-neutral request validation now bounds instruction/text/JSON/output-
  schema/custom-tool-schema metadata, output-token ceilings, and built-in tool
  cardinality/configuration, with a 64-MiB aggregate metadata ceiling. Built-in
  kinds and their domain/store values must be unique and structurally valid.
  Exact egress byte estimates now include
  the complete serialized request plus attachment transfer encoding, so tool,
  schema, continuation, and built-in metadata cannot escape the authorized
  transfer ceiling.
- `ProviderKind` and the GraphQL `AiProviderKindInput` add `LocalHarness` with
  stable value `local_harness`; provider-profile GraphQL may enable and route a
  logical installed profile but cannot configure its process registration.
  The new `local-harness` feature exports the registry, provider, protocol
  driver, process boundary, limits, and non-sensitive transport errors.
- `provider-ollama` enables the optional HTTP/Base64 dependencies and exports
  `OllamaProvider`/`OllamaProviderConfig`. It reports only implemented
  capabilities: native stateless custom tools are supported, while provider
  built-ins, files, provider-retained continuation, and hidden thinking fail
  closed.
- AI schema module `0.16.0` adds principal inbox stream heads, exact
  principal-sequence uniqueness, captured event scope identity, and nullable
  migration-gated inbox fields plus a stable scope key on retention policies.
  Session/provider-output writes now append the applicable inbox event in the
  same transaction.
- `AiConfigurationService` and the configuration GraphQL roots now include
  retention query/mutation contracts. This is a pre-1.0 public Rust and
  GraphQL SDL change requiring host service implementations to add both
  methods and configuration policies to handle `ReadRetention` and
  `ManageRetention`.

- `AiProviderCallExecutor` optionally accepts the exact attachment resolver and
  validated reopening limits. Attachment turns fail before transport when it
  is absent, and current scope/session access is checked again after storage
  I/O before budget state becomes uncertain.
- Provider payload estimation now conservatively includes Base64 expansion for
  attachment bytes, and request validation rejects duplicate attachment IDs.
  Existing server-authored manifests may need larger `estimated_bytes` bounds.
- The OpenAI adapter now advertises image/file input capability when
  `provider-openai` is enabled; that feature also enables its private optional
  Base64 dependency.
- AI schema module version is now `0.15.0`. Attachment rows add nullable
  processing/cleanup deadlines, cleanup generation, retry count and next
  attempt metadata; lifecycle state fields are now privately filterable for
  bounded maintenance queries.
- `ModelInputBlock::Attachment` now requires exact verified `byte_count` and
  lowercase `sha256`. Provider request validation rejects malformed/oversized
  attachment blocks and accounts the full attachment bytes instead of only
  the opaque ID/MIME metadata.
- AI schema module `0.14.0` introduced attachment metadata supporting
  durable pending uploads with nullable object facts, hashed expiring one-time
  capabilities, expected size, scanner/policy versions, and redacted rejection
  state. Existing finalized attachment rows remain representable.
- The dependency universe now pins `graphql-orm-storage` 0.5.0 at its reviewed
  full Git revision with default backend features disabled.
- `AiProviderCallExecutor` can opt into durable live output with
  `with_live_delta_sink`. Visible batches are bounded to no weaker than 50 ms
  or 4 KiB and are committed before a subscription wakeup. The default remains
  no provisional event persistence.
- AI schema module `0.13.0` introduced the persistent meaning of the
  protected `provider_live_delta` session-event type. Entity shape and public
  GraphQL SDL are unchanged.
- `AiReadOnlyAgentCoordinator::new` now requires both an
  `AiAgentCheckpointWriter` and `AiAgentCheckpointAdopter`. Accepted provider
  results are checkpointed before tool/output consumption, and a completed
  tool batch is checkpointed before the next continuation plan. Exact adopted
  batches are consumed before the following provider call. Any checkpoint or
  adoption ambiguity closes the run for recovery.
- AI schema module `0.11.0` added nullable,
  private `protected_state`. Existing final-output checkpoints remain valid
  with no protected state, while older active runs gain no inferred resume
  authority.
- AI schema module `0.12.0` introduced the persistent semantics of
  checkpoint adoption eligibility, cross-generation checkpoint retention, and
  one-shot pre-provider consumption. The entity shape is unchanged.
- `GraphqlRequestContextFactory::build` now receives the complete validated
  `ToolGraphqlRequest` instead of only `GraphqlInvocationContext`, allowing a
  remote issuer to bind delegated authority to the exact server-authored
  operation, canonical variables, projection, disclosure, and audit context.
- `AiRuntime::execute_tool` now rejects every approval-required descriptor.
  One-shot supervised mutations use `execute_approved_tool`, which recomputes
  current host tool policy and compares its version and authorization-state
  digest before building the normal resolver request context.
- The supervised-tool slice introduced AI schema module `0.10.0`. Existing
  tool-call history keeps nullable provider/audit fields; a waiting
  pre-`0.10.0` consequential row cannot be resumed and fails closed for
  reconciliation. The current module is `0.19.0`.
- Approval principal freshness is sampled after asynchronous rehydration,
  avoiding false future-timestamp rejection with sub-second system clocks.

- `AiProviderCallPlan::new_with_tools` now accepts initial turns only and
  rejects pre-populated provider continuation/tool-result input. Exact later
  turns must consume `AiAgentContinuation` through
  `new_continuation_with_tools`.
- `AiRunRecoveryReport` now reports safely finalized output checkpoints in its
  `completed` counter. That checkpoint slice introduced schema module `0.9.0`;
  the current module is `0.19.0`.
- `AiRunRecoveryReport` adds `checkpoint_requeued`. Expired `Running` attempts
  requeue only an exact hash-bound, committed, complete tool-batch checkpoint;
  provider-turn, partial, malformed, consumed, or exhausted adoption attempts
  remain closed as `RecoveryRequired`.

- Multi-repository development now uses one owning agent per repository.
  `graphql-orm-ai` agents treat sibling worktrees as read-only, stage ignored
  handoff prompts for upstream owners, and repin only reviewed final upstream
  commits in dependency order.
- Public Git builds now pin the final `graphql-orm` 0.9.0 merge commit and
  `agql-auth` 0.10.0 annotated-tag target instead of requiring an adjacent
  local sibling checkout or an open-PR revision. CI checks out the same exact
  revisions for baseline compatibility verification. The ORM update supplies
  the generated, database-enforced append-only retention transaction used only
  by the trusted checkpoint worker.
- Crate version is now `0.2.0` because the public budget reconciliation and
  proof-serialization changes are pre-1.0 breaking API changes.
- AI schema module version is now `0.8.0`. In addition to the `0.7.0` tool-call
  changes, proposal rows now persist validated item counts and proposal/
  approval records have deterministic service-owned IDs and stable keyset
  windows required by their authenticated lifecycle services.
- Budget policies/counters now cover
  tool and image units, counters have stable period keys and a unique policy/
  period boundary, and reservations have principal-kind/idempotency uniqueness
  plus complete actual-usage fields.
- Run attempts now receive a separate append-only outcome fact instead of
  relying on mutation of append-only claim history. Egress event IDs are the
  exact policy decision IDs so audit/proof correlation is lossless.
- `AiBudgetService::reconcile` now returns an
  `AiBudgetReconciliationResult`, and `AiError::BudgetDenied` distinguishes
  exhausted capacity from authorization or persistence failures.
- `AiBudgetReservation` and reconciliation results no longer implement Serde
  deserialization; callers obtain validated reservations from a budget service.
- `ProviderRequestContext` now requires an exact `AuthorizedBudgetReservation`
  in addition to egress proofs.
- `AuthenticatedToolBridge` now requires an immutable logical target registry;
  request-context factories receive the validated target, and runtime builders
  require an `AiToolAuthorizationPolicy`.
- `AiRuntime::execute_tool` now requires a registered tool ID and returns an
  `AiToolExecutionResult` only after current policy, resolver, byte/list limit,
  and static disclosure checks succeed.
- `ModelRequest` now has an explicit `continuation` field and its input enum has
  a `ToolResult` variant. Tool results and continuations must occur together.
- `AiProviderCallPlan::new` remains tool-free. The new `new_with_tools` accepts
  only exact explicitly enabled read-only application queries, while
  `new_continuation_with_tools` installs matched result blocks and their exact
  manifests as one unforgeable continuation unit.
- `OrmAiProviderOutputService` rejects a provider turn that still has pending
  custom tool calls instead of prematurely finalizing it as assistant output.
- `AiProposalOutcomeRecorder::record_applied_outcome` now requires the current
  authenticated principal so the ORM service can freshly rehydrate and
  authorize post-mutation linkage. `AiProposalCatalog::descriptor` exposes
  read-only registered metadata and registration now rejects unbounded limits.
- Non-internal tool catalog registration now requires an exact GraphQL
  operation contract and static disclosure schema.
- The opt-in OpenAI smoke-test key file now rejects labels, wrapped values, and
  internal whitespace instead of sending an ambiguous bearer credential.

### Security

- Attachment contents are never resolved from a model-visible storage
  reference. Exact budget and audited egress proofs precede reopening; resolved
  bytes remain redacted from `Debug`, are rebound to request metadata, and are
  accepted by provider context only with complete one-to-one request coverage.
  A missing resolver, changed row/object, owner mismatch, changed checksum, or
  lost authorization releases an unstarted reservation and prevents transport.
- Expired/interrupted attachment cleanup never lists arbitrary storage
  prefixes, reads content, or trusts a stale row. Every candidate is reloaded
  and CAS-claimed; ambiguous deletion retains opaque references and moves to
  bounded backoff, while expired leases are safely reclaimable.
- Every image/file attachment capability proof must contain the exact canonical
  versioned user-provided source reference returned by
  `ModelInputBlock::attachment_egress_reference`. It binds ID, byte count,
  detected MIME and SHA-256; swapping any fact or reusing a broader capability
  manifest is rejected before provider transport.
- Attachment filenames are display-only sanitized metadata and never storage
  paths. Ticket plaintext is returned once, redacted from `Debug`, never
  serialized by Rust APIs, stored only as SHA-256, compared in constant time,
  and insufficient without the current authenticated owner. Scanner or policy
  failure never releases bytes; linked transcript attachments cannot be
  removed through the unlinked-upload mutation.
- Durable live output never receives raw provider frames, tool arguments,
  structured tool events, or hidden reasoning. Every batch is reauthorized and
  protected before a serializable transaction validates the exact active run
  fence and uncertain budget reservation. A sink failure after transport keeps
  provider usage uncertain and must not trigger replay. Provisional events are
  historical progress, not proof of final assistant completion.
- Durable coordinator checkpoints rehydrate the principal and re-resolve an
  unchanged ready protection policy around asynchronous protection. Payloads
  bind the exact attempt/generation, provider result, loop counts, scope,
  result route, completed tool outputs/manifests, and continuation. They are
  persistence proofs only. Only a complete read-only tool batch can become an
  adoption proof, after fresh reopening and durable-record validation; provider
  turns and partial batches remain non-replayable.
- Remote GraphQL execution now rejects local/unregistered targets, stale or
  expired principals, expired delegated authority, changed documents or
  canonical variables, changed operation/projection/disclosure/audit bindings,
  contexts from another adapter, and recursive AI/introspection operations
  before private transport. Incoming bearer tokens and target URLs are not
  accepted by the adapter contract.
- Approval-required descriptors can no longer use the ordinary unapproved
  runtime execution entry point. A consumed proof must match the complete
  rebuilt binding, and fresh policy version/state must still match before the
  resolver is invoked.
- Supervised execution verifies the exact provider turn has a committed,
  reconciled budget reservation before consuming approval. Any resolver
  timeout or post-side-effect persistence/authorization ambiguity terminally
  closes the run as `RecoveryRequired` and is never automatically replayed.

- Tool registration rejects current AI control-plane and GraphQL introspection
  roots, including casing variants, before policy enablement.
- Provider model/output swaps invalidate budget proofs before transport.
- Budget reservation fails closed for stale principal resolutions, tenant
  mismatch, stale/expired run fences, absent policies, invalid counters, and
  partial multi-policy capacity. Uncertain external calls cannot be released by
  the ordinary worker reconciliation path.
- Budget reservation now verifies the active persisted session owner, tenant,
  and exact scope in the same transaction as the run fence and counters.
- A failed egress audit write prevents provider transport and releases only a
  reservation still proven unstarted. An incomplete/erroring provider stream
  retains uncertain budget capacity and is never silently retried.
- Every worker child/terminal write validates run, attempt, generation, owner,
  expiry, state, and row version; pre-provider lease expiry may requeue while
  post-start expiry becomes `RecoveryRequired`.
- Approval changes to resource, policy, schema, document, projection, actor,
  preview, or authorization-state bindings invalidate the grant.
- OpenAI HTTP 401 responses map to the redacted `CredentialUnavailable`
  category instead of a generic provider rejection.
- OpenAI retained-response mode now requires every exact transfer manifest to
  declare `provider_response` retention. The secure default remains
  `store_responses = false`; stateful tool continuation fails closed under that
  default until stateless encrypted continuation is implemented.
- Consequential, proposal, mutation, subscription, approval-required, and
  non-idempotent descriptors cannot enter the implemented read-only loop.
- Proposal acceptance changes only AI-owned staged state and never executes an
  application mutation. Approval consumption proves intent once but still
  requires fresh ordinary resolver authorization and current resource-version
  enforcement before any consequential side effect.

## 0.1.0

Initial release is not yet published. Everything above remains unreleased
until the production gates in `docs/implementation-status.md` are satisfied.
