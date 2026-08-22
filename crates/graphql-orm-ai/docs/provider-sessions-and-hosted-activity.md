---
title: "Provider Sessions, Hosted Search, and Visible Activity"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-15
review_by: 2027-02-11
supersedes: []
---

# Provider Sessions, Hosted Search, and Visible Activity

This guide defines four related but independently enabled contracts:

- an exact run-scoped local app-server process;
- provider-generated visible reasoning summaries;
- provider-hosted web search with authoritative citations; and
- a protected durable provider-thread cursor.

None is application authority. Every model call still needs current-principal
rehydration, current rules, an atomic budget reservation, exact egress
decisions, a registered provider profile, and the current run fence. Every
application tool remains an exact coordinator-owned descriptor and GraphQL
operation whose resolver makes the final authorization decision.

## Capability matrix

| Contract | Default | Durable provider state | Application tools |
| --- | --- | --- | --- |
| JSONL `local-harness` | disabled | none | exact stateless replay supported |
| Codex app-server v2 | disabled | run process plus optional protected thread cursor | default-off experimental dynamic tools through the coordinator |
| Native OpenAI hosted search | absent from each request | Responses continuation only when selected | mixed retained continuation supported |
| Visible reasoning summary | disabled | protected activity/final blocks | authority-neutral |
| Reasoning effort | provider/registration default | exact request, budget, checkpoint, and retained-session fingerprint | authority-neutral |
| `AiProviderSessionService` | not constructed | protected opaque cursor | authority-neutral |

The Codex app-server adapter and durable provider-session service are not
automatically coupled. A process may be warm without a retained provider
thread, and a retained thread may exist while no process is running.

## Closed reasoning-effort negotiation

`ModelReasoningEffort` has seven values. `Unspecified` preserves the active
provider or registration default by omitting the wire override. The six
explicit values are `None`, `Low`, `Medium`, `High`, `XHigh`, and `Max`.
Unknown serialized values fail closed, and explicit `None` is never treated as
`Unspecified`.

The crate has no universal model-to-effort catalogue. A deployment creates one
`ModelReasoningEffortProfile` per exact reviewed model, including the supported
explicit set and its default. Native OpenAI accepts a collection through
`OpenAiProviderConfig::with_reasoning_effort_profiles`; a Codex registration
accepts only the profile for its exact logical model through
`with_reasoning_effort_profile`. `AiRuntime::provider_capabilities` and
`ProviderCapabilities::reasoning_effort_profile` are the server-owned source
for a settings UI. Browser input still has to deserialize into the closed enum
and be present in `profile.supported()`.

For currently reviewed GPT-5.6 registrations, the official
[GPT-5.6 guide](https://developers.openai.com/api/docs/guides/latest-model)
supports this deployment matrix:

| Exact model | Explicit admitted values | Reviewed default |
| --- | --- | --- |
| `gpt-5.6-sol` | `none`, `low`, `medium`, `high`, `xhigh`, `max` | `medium` |
| `gpt-5.6-terra` | `none`, `low`, `medium`, `high`, `xhigh`, `max` | `medium` |
| `gpt-5.6-luna` | `none`, `low`, `medium`, `high`, `xhigh`, `max` | `medium` |

This table is registration input for that reviewed deployment, not a library
assumption. A future provider/model profile may declare a strict subset or a
different default without widening the enum.

The native adapter writes an explicit selection to `reasoning.effort` and
omits the entire effort member for `Unspecified`. It composes independently
with the visible `reasoning.summary` request. Effort never requests or exposes
hidden chain-of-thought.

Codex CLI 0.147.0 generated `v2/TurnStartParams.json` places the optional
non-empty string at `turn/start.params.effort` and describes the override as
applying to the current and subsequent turns. The strict actor narrows that
open schema string to `ModelReasoningEffort` and authors the field name itself.
It does not accept a JSON value or caller-supplied protocol key. The ignored
`generated_codex_0147_schema_places_effort_only_on_turn_start` test regenerates
and checks the schema, while the environment-gated retained live lane sends
the exact effort on the newly bound first turn and the later resumed turn.

Because the app-server value affects subsequent turns, Codex effort is frozen
into the provider-session fingerprint:

```rust
let registration = AiCodexAppServerRegistration::new(
    provider_profile_id,
    logical_model,
    executable_sha256,
    executable_version,
    sandbox_profile,
    AI_CODEX_APP_SERVER_PROTOCOL_V2,
)?
.with_reasoning_effort_profile(profile)?;

let fingerprint = registration.provider_session_fingerprint(selected_effort)?;
```

Use `fingerprint`, not `registration.identity()`, in
`AiProviderSessionDescriptor`. Every initial or continuation
`ModelRequest` and matching `AiBudgetReservationRequest` carries
`selected_effort`. A changed setting cannot resume the cursor: clean up and
confirm exact absence, issue the ordinary rebind, create a new empty thread,
and bind the new effort fingerprint. Pre-0.80 v3 registration fingerprints
are accepted only by the deletion adapter for draining; they are never valid
for a turn or resume.

For stateless operation the scope is one ephemeral Codex thread: the actor
retains the selected effort through `thread/start` and its `turn/start`, then
clears it only after the terminal turn notification. A later ephemeral thread
may select another profile-admitted effort. The provider-call plan, budget
proof, and continuation checkpoint still require one exact value across every
retry or bounded replay of the same logical turn.

## Exact run-scoped app-server

Enable `provider-codex-app-server` to use the strict adapter. The
host supplies:

1. `AiCodexAppServerRegistration`, binding the logical profile/model,
   executable digest/version, sandbox profile, and exact protocol version;
2. `AiCodexAppServerRunLimits`, including global, per-owner, per-run turn, and
   startup/turn/interrupt/shutdown ceilings;
3. an `AiCodexAppServerRunProcessFactory`; and
4. `AiCodexAppServerRunPool` and `AiCodexAppServerProvider`.

The wire contract follows the official
[Codex app-server protocol](https://developers.openai.com/codex/app-server).
Dynamic tools remain explicitly experimental there: the client opts into
`experimentalApi`, installs reviewed definitions on `thread/start`, and admits
only the documented `item/started` → `item/tool/call` → client response →
`item/completed` sequence for the exact active thread and turn.

The factory is a trusted deployment boundary. It must execute the verified
image directly, never through a shell or path search; clear inherited
environment and credentials; use an empty isolated working directory; deny
unreviewed network, filesystem, child-process, socket, and credential access;
and return `AiCodexAppServerLaunchedProcess` with an idempotent synchronous
process-tree kill callback. The wrapper invokes that callback on final drop,
including an abandoned stream or failed graceful shutdown.

Dynamic tools require a separate closed launch profile. Bind the reviewed
model-catalogue mode and profile into the registration:

```rust
let launch_profile = AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
    AiCodexAppServerModelToolMode::Direct,
)?;
let bootstrap = AiCodexAppServerBootstrapInstructions::from_static(&[
    "Use a registered application tool whenever current facts are needed to answer the request.",
])?;
let registration = AiCodexAppServerRegistration::new(
    "local-dynamic-tools",
    "reviewed-direct-tool-model",
    executable_sha256,
    executable_version,
    "isolated-no-native-tools",
    AI_CODEX_APP_SERVER_PROTOCOL_V2,
)?
.with_launch_profile(launch_profile)
.with_bootstrap_instructions(bootstrap);
```

The factory returns true from `supports_launch_profile` only when it launches
that profile with `registration.launch_profile().codex_arguments()` unchanged,
an environment cleared of unrelated credentials, a private configuration home
containing no project config or MCP servers, an empty working directory, and
the registered operating-system sandbox. The actor additionally sends empty
thread/turn environments and a closed thread config that disables shell,
unified execution, Code Mode, utility tools, connectors, plugins,
collaboration, images, browser/computer use, and hosted search. This is defense
in depth: the process sandbox remains authoritative if a provider version
ignores a feature toggle.

The sole process-level exception is measured on Codex 0.148.0: adding
`--disable code_mode_host` to this otherwise identical profile made a retained
GPT-5.6 Luna turn complete without issuing its offered tool, while omitting
that one argument produced the direct `dynamicToolCall` / `item/tool/call`.
`codex_arguments()` therefore omits only that disable. The actor still sends
`features.code_mode_host=false`, `features.code_mode=false`, and
`features.code_mode_only=false` per thread; shell, file, MCP, browser, web, and
every other native item remain unavailable and are rejected by the protocol
actor if emitted. Re-run the direct-tool readiness probe and negative native-
item suite before adopting another Codex version.

Only a reviewed `Direct` model-tool declaration can construct this profile.
Codex models declared `CodeMode` or `CodeModeOnly` are rejected rather than
silently losing dynamic tools or requiring a native Code Mode host. Such a
registration may still use the strict text-only profile. When the factory does
not attest the dynamic profile, provider capabilities report
`custom_tools = false` and no dynamic process starts.

The crate-owned protocol actor deliberately has no generic JSON-RPC send
method. Initialization always negotiates one fixed opt-out profile for thread
status/settings/cleared-goal, MCP-startup, and account-rate-limit notifications
that this adapter neither consumes nor exposes. Stable and experimental
initialization use the same profile; only the dynamic-tool path additionally
sets `experimentalApi: true`. An opted-out method remains rejected if the
server sends it anyway. The actor admits only initialization, exact thread
start/resume/delete, turn start/interruption, correlated responses, the closed
visible-event allowlist, and—only for an experimental registration—the exact
documented `item/tool/call` server request. Commands, shell, files, patches,
MCP, collaboration, images, hosted web search, browser control, raw reasoning,
and arbitrary methods remain forbidden.

Codex may emit the documented generic `warning` while a turn is open. The
actor accepts only the exact positive-timestamp envelope, an optional thread ID
matching the active thread, and a bounded non-empty control-free message. It
limits each turn to eight warnings and 16 KiB total text, discards every field,
and returns only `AiCodexAppServerInbound::RuntimeWarning`. Hosts treat that
variant as a non-fatal control event; they never log or forward the warning
text. Warnings outside the current turn and every other generic notification
remain rejected.

Every turn explicitly requests `summary: "none"`. Codex may still report an
empty reasoning item lifecycle. The actor accepts only an exact paired item
whose `content` and `summary` arrays remain empty, discards its identifier and
timestamp, and returns `ReasoningLifecycle`. It rejects non-empty reasoning or
summary content and all reasoning deltas, so this control event is neither a
reasoning summary nor hidden chain-of-thought.

The closed default accepts an initial `StatelessReplay` request containing
only bounded trusted instructions and text; each call gets a fresh ephemeral
thread while the exact run process may be reused. A retained turn instead uses
`ModelContinuationMode::ProviderRetained`, an exact
`AiProviderSessionTurnPlan`, and a configured `AiProviderSessionService`.
Creation sends only the immutable model, optional compile-time static
`AiCodexAppServerBootstrapInstructions`, and reviewed dynamic-tool definitions
to an empty persistent thread. It sends no user input, request-local
instruction, route context, secret, or resolver result until the opaque cursor
is durably protected and claimed. The bootstrap fingerprint is part of the
registration identity, and retained requests must leave
`ModelRequest::instructions` empty or copy the exact frozen bootstrap blocks.
Any other request-local instruction is rejected. First activation and every
resume prove the same bootstrap, cursor, owner/session/scope/profile/model/
executable/protocol/policy/transcript/run fence before business input can
start.

The first turn after empty-thread create consumes a crate-owned NewlyBoundEmpty
activation and starts on that exact process. If that activation check fails,
`ProviderError::NewlyBoundTurnRejected` names only the closed phase
(opened-session, registration, cursor, bootstrap, frozen definitions, or
missing process). Coordinator hosts can also receive that phase from
`AiProviderFailureDiagnosticSink::record_newly_bound_turn_rejection`. Those
codes are content-free and do not authorize a fresh-thread fallback.

One protocol actor may perform sequential lifecycle cycles on its retained
process. Each typed `thread/start` or `thread/resume` begins a private
observation phase. New thread creation requires exactly one correlated
response and one matching `thread/started` notification in either order.
Retained resume uses that same pair when both frames are delivered. Codex
0.147.0 may instead deliver one cumulative `thread/tokenUsage/updated`
snapshot around the correlated response. The actor validates its complete
nonnegative generated shape and exact thread correlation, discards all token
values, and permits that content-free snapshot to close only the typed resume
phase once its response is also present. The snapshot is not charged to the
new run and cannot complete initial creation. The next resume and `turn/start`
remain closed until the applicable phase is complete. There is no public state
reset, and the retained model and dynamic-tool definitions cannot change
between creation, resume, or later terminal turns.

Deletion completes from the exact empty successful `thread/delete` response.
It never depends on or admits `thread/status/changed`; the fixed initialization
profile suppresses that notification for the connection.

Experimental dynamic tools require a registration using
`AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1`, a process
factory that attests that exact profile, and
`AiReadOnlyAgentTurnPlan::new_experimental_dynamic_tools`. The provider process
receives no application credential or resolver transport. An exact
`item/tool/call` is schema/fingerprint matched to the current `ModelRequest`,
then the coordinator rechecks cancellation, current rules, run budget, and the
ordinary registered read-only GraphQL tool boundary. Only the already
disclosure- and egress-approved result is returned to app-server. Unknown,
duplicate, stale, over-limit, changed-policy, or incomplete calls poison the
turn and make a retained cursor cleanup-only.

Owning subgraphs compile generated or custom profiles into a canonical
manifest and register it in `AiToolCatalog`. Build the provider definition with
`AiToolCatalog::read_only_model_definition`; do not copy the description,
argument schema, stable ID, or descriptor fingerprint into host code. Codex
0.147.0 accepts a smaller JSON Schema subset than the canonical profile
contract. The adapter therefore performs one closed deterministic projection:
it removes only unsupported schema meta/constraint keywords, carries scalar
bounds into the provider-visible property description, treats an omitted
object `required` keyword as the JSON Schema empty set and emits
`"required": []`, and fingerprints the projection together with the exact
canonical descriptor. The unmodified canonical schema remains authoritative
when a dynamic call is admitted and again at coordinator execution, so
projection cannot weaken the accepted argument range. Malformed `required`
arrays, unknown keywords, and objects that are not `additionalProperties:
false` still fail closed.

JSON-RPC request identifier zero is valid and is used by Codex 0.147.0 for its
first server-initiated dynamic call. The actor correlates it in the same
private pending-request map as every other unsigned identifier; accepting zero
does not weaken method, lifecycle, schema, tool, owner, run, or cursor checks.

Coordinator cancellation and terminal paths call `interrupt_run` and
`close_run` through `AiRuntime`. The process binding includes a non-exported
owner fingerprint for admission only; it grants no provider or application
authority. A changed attempt, lease generation, registration, profile, model,
or owner cannot reuse an entry.

## Visible reasoning summaries

`ModelReasoningSummaryRequest::Disabled` is the closed default. A host may use
`Auto { maximum_bytes }` only after the selected provider advertises
`ProviderCapabilities::visible_reasoning_summaries`; it should select
`Disabled` when the capability is absent so an otherwise valid run continues
without a summary. Current hierarchical rules must independently permit
`AiRuleProviderCapability::VisibleReasoningSummaries`.

The value means a provider-generated summary intended for presentation. It is
not raw chain-of-thought and must never be described as such. An adapter that
cannot distinguish the two must reject the request. Native OpenAI Responses
maps the request to `reasoning.summary = "auto"` and rejects unsolicited or
over-bound summary deltas.

Summary deltas enter `AiProviderActivityPayload::ReasoningSummary`, are
content-protected before durable replay, and are also retained as distinct
protected final provider blocks. Already committed partial summary activity
may remain when cancellation or failure wins; the durable terminal run state
defines it as incomplete. No summary text belongs in logs, telemetry, URLs,
error strings, or unprotected lifecycle records.

## Hosted web search

Web search is enabled only by adding `ModelBuiltinTool::WebSearch` to a
server-authored request. Its domain policy is explicit:

- `ModelWebSearchDomainPolicy::PublicWeb`;
- `allowed_domains(...)`; or
- `blocked_domains(...)`.

Allow/block constructors require one to one hundred canonical domains and
reject schemes, wildcards, duplicates, controls, and malformed labels. An
empty collection never means public search. The model cannot select the mode,
raise `maximum_builtin_tool_calls`, add headers/cookies/credentials, or turn a
search result into arbitrary URL-fetch authority.

Every request needs a distinct `WebSearch` egress manifest and current rule
capability. `maximum_builtin_tool_calls` is checked against deployment limits
and reserved pricing before transport. Hierarchical rules add the independent
cumulative `maximum_web_search_calls` ceiling. The coordinator carries actual
completed search usage across protected checkpoints, so a continuation cannot
reset it. Provider start without completion is not billable completion
evidence and leaves uncertain transport to the existing recovery path.

Native OpenAI Responses can offer hosted search and reviewed application tools
in the same `ProviderRetained` request. Application calls still return to the
coordinator, which rehydrates the principal, reapplies tool policy, issues
narrow delegated authority, executes the exact registered GraphQL operation,
and sends only disclosure-approved output in the next retained continuation.
The existing prohibition on mixing built-ins with application tools in
`StatelessReplay` remains intentional.

`ProviderCitation` accepts only bounded HTTPS source metadata tied to an exact
provider output item, output/content index, and non-empty text span. Streaming
and background normalization use the same validation. Citation URLs are
display metadata, not safe-navigation or source-trust proof. Assistant
Markdown links never become authoritative citations.

## Ordered durable activity

For new integrations, install `OrmAiLiveDeltaService` through
`AiProviderCallExecutor::with_provider_activity_sink`. This mode supersedes
the legacy text-only sink for the executor and writes protected
`provider_activity` events to both the session stream and owner inbox.

`AiProviderActivityCoalescer` batches visible UTF-8 text and summary deltas but
flushes them before every structured event. It then records, in exact provider
order:

- visible text;
- visible reasoning summary;
- hosted-tool started;
- validated citation; and
- hosted-tool completed.

The session stream supplies the authoritative cross-turn sequence. Hosted
activity contains no arguments or result body. Application-tool start and
completion continue to use their separate fenced lifecycle events. Provider
activity persistence rechecks the current principal, owner, scope, content
policy, budget reservation, and exact run lease around protection and commit.
Failure after transport leaves usage uncertain and must not trigger replay.

Legacy `provider_live_delta` rows remain readable. Retention may remove expired
provisional activity without reusing sequence numbers; reconnect across a gap
returns the existing reset-required signal.

## Durable provider-thread binding

`OrmAiProviderSessionService` owns one private binding row per AI session. It
stores a content-protected opaque cursor plus:

- owner principal reference, tenant, and scope;
- provider kind/profile/model;
- executable or adapter registration fingerprint and protocol version;
- host policy fingerprint;
- exact transcript fingerprint and durable message watermark;
- run/attempt/lease and binding claim generations; and
- idle, absolute, provider, cleanup, and retry state.

`AiProviderCallExecutor::execute_with_provider_session` and the coordinator
enforce this creation order:

1. create an empty provider thread and retain a provider-side deletion guard;
2. use the host-planned immutable `AiProviderSessionDescriptor` and canonical
   transcript-prefix fingerprint;
3. call `bind_for_run` under the current run lease;
4. call `open_for_run`, preserve the crate-owned newly-bound activation, and
   consume it once on the exact process/cursor that created the empty thread;
5. start the first `turn/start` directly on that already-loaded thread without
   issuing `thread/resume`, then send business content; and
6. after protected assistant output, its matching
   `assistant_output_persisted` checkpoint, and canonical `Completed` run state
   commit, call `commit_turn` with the new authoritative
   watermark/fingerprint. If this retention-only update fails, quarantine the
   cursor without changing the already-completed user answer.

Later-run resume uses `claim_for_run` and then `open_for_run`. Both require the exact
descriptor and transcript evidence, current principal/session/scope access,
and current run fence. Its provider adapter performs the strict
`thread/resume` response/notification lifecycle before `turn/start`; it cannot
reuse the one-shot newly-bound activation. A crash, cancellation, protocol error, ambiguous
provider state, output-persistence failure, policy/profile/model/executable
drift, or rejected cursor calls `require_cleanup`; v1 never guesses provider
state or advances a watermark from incomplete evidence.

`disposition_for_run` is the authoritative planning boundary. It reports
`New`, exact `Resume`, `Unavailable`, or `RebindAllowed`; hosts must not infer
eligibility from raw state fields. `RebindAllowed` is a crate-issued,
short-lived authorization available only after cleanup persisted exact
provider absence in a `Deleted` tombstone. `rebind_for_run` rehydrates current
authority and atomically compares the exact owner/session/scope, run/attempt/
lease fence, deleted row and cleanup generations, provider descriptor, and
host-authored canonical transcript fingerprint. It protects a fresh cursor in
the same row, so the unique session binding remains intact and concurrent
replacement has one winner. Cleared cursor material is never reopened.

A managed cleanup loop performs:

1. `claim_cleanup(worker_id)`;
2. `open_for_cleanup` under the exact maintenance protection policy;
3. registered `AiProviderSessionDeletionService::delete_or_confirm_absent`;
4. `complete_cleanup` with the exact cursor-bound absence proof, which clears
   the cursor and retains a private absence-proven `Deleted` tombstone; or
5. `schedule_cleanup_retry` with a bounded safe reason and delay.

An ordinary later run may replace only that exact absence-proven tombstone.
Expiry, process death, transport failure, cleanup backoff, and restore
quarantine remain unavailable. If creation of the replacement empty thread
succeeds but its rebind CAS loses, `AiProviderCallExecutor` invokes the exact
registered provider discard boundary.

### Parking across approval and subscription waits

A completed provider-retained tool-request turn may be parked while a durable
human approval or bounded replayable-subscription wait owns progress. This is
distinct from making the binding `Active`: no unrelated run can claim it, and
the cursor remains bound to its source run, attempt, lease generation and
binding claim generation.

The checkpoint owner creates `AiProviderSessionWaitParkRequest`; applications
cannot construct it. The opaque request binds the exact provider-session
claim, already-durable `provider_turn_persisted` checkpoint and hash, closed
approval/subscription wait identity, frozen descriptor and transcript prefix,
and a fingerprint of the complete provider-retained continuation. Alternate
provider-session stores may inspect only the bounded getters needed to enforce
those comparisons.

Parking uses a two-phase state graph:

1. `park_for_wait` performs the exact `Claimed -> ParkedWait` CAS while the
   source run lease is still current.
2. The owning wait transaction persists its wait row and parked coordinator
   checkpoint, transitions the run to `WaitingApproval` or
   `WaitingSubscription`, records the source attempt's nonterminal outcome,
   and clears the ordinary run lease.
3. `confirm_parked_wait` rehydrates the current owner and confirms that exact
   graph. A cleanup worker may idempotently perform the same confirmation after
   a crash between steps 2 and 3; it cannot confirm a partial or substituted
   graph.
4. The ordinary run queue later creates a fresh fence. Only after the exact
   approval or subscription adoption has been claimed and consumed once does
   `reclaim_after_wait` perform `ParkedWait -> Claimed` for that fence.

For approvals, the queue also requires confirmation before it may claim the
approved row. The claim creates a fresh attempt/generation and refences the
exact pending tool call and run step. If approval wins before confirmation,
the row remains unclaimed until maintenance confirms the unchanged graph.

The reclaim authorization is crate-private and derived from durable adoption
state. A host cannot turn a state value, UUID or expired wait into resume
authority. After reclaim, failure before provider transport uses
`require_cleanup` for the returned claim; it never releases the cursor to
`Active`. Concurrent reclaim has one CAS winner.

Parking is available only after the adapter has produced a stable resumable
retained checkpoint. Stateless replay, an in-flight streaming response, and a
provider-native dynamic-tool turn whose synchronous responder is still active
are not suspendable and fail closed. Parking never retains a warm process: the
adapter may shut down while the protected cursor remains durably parked.

Cancellation, terminal convergence, reset, wait expiry, abandonment and an
unconfirmed park whose source claim expires become cleanup candidates. The
registered deletion service must still delete the exact old provider session
or authoritatively prove absence. Expiry, local process death and restore are
not absence proof. Portable restore continues to quarantine/redact provider
session state and cannot reclaim a parked cursor.

Session deletion fences active bindings and removes an absence-proven deleted
tombstone only in the final retention dependency order. Expiry and a backup
redaction marker are not absence proof. Portable
backup redacts the cursor and the required restore audit blocks readiness when
any binding exists; drain provider sessions before a portable backup intended
for ready restore. Normal process restart may resume an exact live binding,
but raw or portable database restore never does.

Deployments may attach `AiProviderFailureDiagnosticSink` to the executor for a
closed machine category such as process exit, timeout, transport unavailable,
rate limit, protocol violation, invalid dynamic-tool call, retained-resume
rejection, cancellation, or persistence-fence loss. These content-free values
cannot change retry or terminal semantics. Provider text, prompts, output,
tool data, credentials, cursors, and authorization detail never enter the
sink; an uncertain turn still becomes `RecoveryRequired`.

## Deferred visual browser

A future visual browser is a separate capability broker, not hosted search and
not an app-server method. Its observation-first, one-shot, host-isolated design
is recorded in [the visual-browser broker plan](visual-browser-broker.md).
No browser capability is implemented by the contracts in this guide.
