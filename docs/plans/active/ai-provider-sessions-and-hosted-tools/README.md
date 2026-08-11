---
title: AI provider sessions, hosted tools, and visible activity
kind: plan
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2026-09-11
supersedes: []
---

# AI provider sessions, hosted tools, and visible activity

## Outcome

`graphql-orm-ai` provides project-neutral contracts for an efficient local
provider harness, provider-hosted web search, and provider-generated visible
reasoning summaries without weakening its current authorization, egress,
disclosure, budget, or durable-run boundaries.

The first completed path keeps one strictly allowlisted local app-server
process for every claimed run and reuses it across that run's provider turns.
Later phases may keep an owner-bound process warm and resume a protected
provider thread, but warm operating-system processes and provider-retained
threads remain independently configurable resources.

Application tools remain owned and executed by the coordinator. A local
harness receives no application bearer token, raw router access, database
access, filesystem access, shell, browser, screenshot, remote-control, dynamic
tool registration, or general JSON-RPC capability.

## Non-goals

- Consumer-specific provider, policy, router, authentication, or user-interface
  changes.
- A generic MCP server, arbitrary GraphQL executor, URL fetcher, shell, code
  runner, filesystem bridge, or browser automation endpoint.
- Raw chain-of-thought. The only visible reasoning content is a bounded summary
  deliberately emitted by a provider for presentation.
- Cross-owner process or provider-thread sharing. Multiplexing remains deferred
  until a provider protocol and isolation assessment prove it safe.
- An immediate visual-browser implementation. Its broker boundary is recorded
  separately for future work.
- Making hosted search automatically available. Library defaults remain deny;
  a host must select a reviewed provider profile and bounded search policy.

## Dependencies

- Existing fenced durable runs, current-principal rehydration, cancellation,
  protected content, usage ledger, budget reservations, egress manifests,
  application-tool descriptors, and session/inbox replay contracts.
- Existing `AiProvider`, `ModelRequest`, `ProviderRequestContext`, provider
  event, coordinator, and local-harness boundaries.
- The Codex app-server initialized protocol, thread lifecycle, turn lifecycle,
  and interruption methods. Provider-specific wire handling stays behind a
  strict method, notification, item, and server-request allowlist.
- Provider-hosted web-search and reasoning-summary capabilities. Unsupported
  providers degrade through explicit capability negotiation.
- A schema-module increment before any new durable provider-thread or activity
  records become part of the supported persistence contract.

## Durable boundaries

### Library responsibilities

`graphql-orm-ai` owns:

- provider-neutral request policy, capability negotiation, normalized events,
  continuation identity, budgets, egress, durable ordering, protection,
  retention, fencing, and owner-authorized replay;
- a run-scoped local-harness lifecycle that can retain one process across
  multiple provider turns without sharing it between runs;
- a protected provider-thread binding containing only an opaque resume cursor
  and exact host-owned binding evidence;
- durable search, citation, visible-summary, and lifecycle activity records;
- cancellation propagation and the rule that a stale process, provider thread,
  or run lease cannot persist later output;
- provider-specific adapters that translate only a documented, allowlisted
  protocol into provider-neutral contracts.

### Host responsibilities

A consuming application owns:

- provider installation, executable identity, provider credentials, process
  sandboxing, state-directory protection, and provider-profile selection;
- immutable deployment ceilings and the decision to enable hosted search,
  reasoning summaries, warm processes, or provider retention;
- ordinary current-principal, tenant, row, field, tool, resolver, MFA,
  approval, and delegated-authority enforcement;
- managed worker lifecycle and user-interface presentation;
- deletion of provider-owned state through the library contract when a session,
  provider profile, or retention policy requires it.

The host never receives an API that can supply an unvalidated provider cursor,
forge a watermark, bypass a run fence, add model-selected tools, or widen
egress.

## Phase 1: claimed-run local app-server lifecycle

Add a provider-neutral run-session interface and a provider-specific Codex
app-server adapter. One process is admitted for one fenced claimed run and is
reused for independently bounded fresh text-only provider turns in that run.
Phase 1 deliberately did not claim application-tool continuation support or
retain a Codex thread across those continuations: app-server dynamic tools are synchronous
server requests inside an in-flight turn, while the existing coordinator
executes an application tool only after the provider turn finishes. Treating
them as the same contract would deadlock or move authority into the adapter.
Dynamic tools therefore remained forbidden in that phase. The existing JSONL harness remains
the supported local stateless application-tool path, and each Phase 1
app-server turn uses a fresh, bounded provider thread that is deleted before
reuse of the process. The
process is terminated on run completion, failure, cancellation, lost lease,
worker shutdown, protocol violation, or resource-limit expiry.

The adapter must:

- negotiate protocol version and supported features before readiness;
- allow only initialization, thread start, turn start, turn interruption, and
  bounded thread cleanup required by this phase;
- reject every unknown method, notification, server request, item type, and
  response shape;
- reject shell-command, filesystem, dynamic-tool, screenshot, browser,
  arbitrary command, and generic JSON-RPC use even when the provider supports
  it;
- bind every response and event to the exact session, run, provider profile,
  request correlation, attempt, lease generation, and policy fingerprint;
- provide bounded startup, request, idle, shutdown, output, and protocol
  limits plus global, per-profile, and per-owner admission limits;
- integrate durable cancellation with `turn/interrupt` and bounded process
  termination;
- settle provider usage and durable reservations exactly once.

Application-tool execution continues through the existing coordinator. A
provider-specific request may identify an exact reviewed tool call, but it
cannot execute the tool, select the GraphQL destination/document, or receive a
delegated credential.

### Phase 1 acceptance

- One exact claimed-run binding with several fresh text-only turns launches one process; each turn
  gets a fresh isolated thread that is deleted before the next turn.
- Concurrent runs do not share state; resource admission is bounded and
  produces stable backpressure.
- Cancellation interrupts the current turn and prevents later output or tool
  execution after the cancellation fence wins.
- Unknown or forbidden app-server traffic terminates the run fail-closed.
- Crash, EOF, timeout, malformed frames, stale leases, and executable identity
  changes cannot produce accepted output.
- Existing one-shot local-harness registrations and other provider adapters
  remain compatible.

## Phase 2: requested visible reasoning and ordered activity

Add a provider-neutral, host-selected reasoning-summary request mode. Disabled
is the default. Providers advertise support separately; an unsupported provider
may omit the optional summary without making an otherwise valid run
unavailable unless a future profile explicitly requires it.

Normalize bounded reasoning-summary deltas into protected content blocks and a
single canonical ordered activity stream with application-tool, hosted-search,
citation, assistant-output, cancellation, and terminal activity. Preserve the
provider's order while assigning library-owned durable sequence numbers.

Summary content:

- is explicitly described as a provider-generated visible summary, never raw
  internal reasoning;
- has independent byte, token, delta, and block ceilings;
- is protected at rest, owner-authorized at read time, and subject to existing
  retention and purge behavior;
- never enters ordinary logs, error strings, URLs, analytics, or unprotected
  lifecycle payloads;
- may end as a bounded partial summary when a turn is cancelled or fails, with
  the terminal state making that incompleteness explicit.

### Phase 2 acceptance

- Disabled, supported-auto, and unsupported-auto provider paths are covered.
- Streaming summary, tool/search activity, citations, output, and terminal
  state replay in one authoritative order after reconnect.
- Cancellation stops later deltas and output while preserving only the already
  committed protected partial summary.
- Ordinary events and logs contain no summary text.
- Existing providers that emit no summary remain source and behavior
  compatible.

## Phase 3: hosted web search with application tools

Permit a host-authored provider builtin and reviewed application tools in one
run through the already supported provider-retained continuation contract.
Extend the app-server path only after it has a separate, explicit continuation
design that preserves coordinator-owned tool execution without using dynamic
tools. Do not remove the existing stateless-replay prohibition until stateless
mixed-tool reasoning and continuation evidence can be represented and
validated completely.

Web-search policy is explicit rather than inferred from an empty list:

- disabled;
- public web; or
- domain-constrained web with normalized host-authored allow/block policy when
  supported by the selected provider.

The host also supplies a maximum search-call count below immutable deployment
limits. The model cannot enable search, select the policy mode, raise limits,
or add arbitrary headers, credentials, cookies, or URLs.

Search lifecycle and source attribution are normalized from provider-authored
events. Durable citations retain validated provider source identity and URL
metadata separately from assistant Markdown. Markdown links are never promoted
to authoritative citations.

### Phase 3 acceptance

- One retained run can perform bounded hosted search, execute one exact
  registered application tool through the coordinator, and produce a final
  answer carrying authoritative citations.
- Search and application-tool calls retain distinct egress, usage, pricing,
  budget, lifecycle, and disclosure accounting.
- Search content cannot register a tool, change a GraphQL target/document,
  enable another provider capability, bypass current authorization, or leak
  secrets into ordinary telemetry.
- Public web is available only when explicitly selected; disabled remains the
  library default; domain constraints round-trip and fail closed when unsupported.
- Reconnect and replay preserve source identities and event ordering.

## Phase 4: protected provider-thread bindings and warming

Persist an opaque provider-thread binding only after the run-scoped lifecycle
is stable. A binding contains protected resume material plus library-owned
evidence for:

- exact session owner/principal reference and tenant;
- target scope;
- provider profile, provider kind, model, and executable/configuration
  identity;
- relevant deployment, rule, tool-manifest, disclosure, and egress
  fingerprints;
- the exact durable session message sequence/watermark represented in the
  provider thread;
- creation, last use, idle expiry, absolute expiry, deletion state, and
  provider-retention declaration.

Resume is allowed only when every binding is current and the provider thread's
watermark exactly matches the authoritative durable history. A recoverable
missing-thread result may start a replacement thread and reconstruct the
bounded authoritative history. It must not silently continue from only the
newest input. Any other mismatch fails closed and requires a reviewed reset.

An in-memory warm process is a separate cache. It may be evicted without
deleting the protected provider-thread binding, and a binding may be retained
without a warm process. Idle TTL, absolute lifetime, maximum processes,
per-owner/profile limits, upgrade draining, and deletion are independently
configured and bounded.

### Phase 4 acceptance

- Same-session resume succeeds only at an exact authoritative watermark.
- Cross-owner, cross-tenant, cross-scope, stale-policy, stale-tool-manifest,
  model, provider-profile, or executable mismatches deny.
- Cursor and provider state deletion follows archive/delete/retention/provider
  removal without resurrecting a session.
- Warm-process eviction and durable-thread deletion are independently tested.
- Restart recovery fences stale workers and never accepts output for a newer
  lease.
- Multiplexing remains disabled.

## Phase 5: optional multiplexing investigation

Do not implement cross-session multiplexing by default. Investigate one
app-server managing several provider threads only after protocol conformance,
resource accounting, cancellation isolation, output demultiplexing, state
directory isolation, crash blast radius, and cross-owner confidentiality have
dedicated tests. Any eventual implementation must retain a library-enforced
per-thread owner/run binding and a host-configurable one-thread-per-process
mode.

## Future visual-browser broker

Visual browsing is a separate capability broker, not an extension of hosted
search or the local provider process.

A future project-neutral contract may expose typed, approval-aware operations
such as navigate-to-reviewed-origin, capture-bounded-screenshot, inspect a
bounded accessibility snapshot, and perform a constrained interaction. Each
operation would carry an exact browser-session capability, origin/redirect
policy, owner and tenant binding, expiry, rate/byte limits, current-principal
check, audit correlation, and disclosure classification.

The broker would own isolated browser contexts, network and download policy,
cookie/credential prohibition, popup and protocol filtering, visual-result
protection, and destruction on revocation or expiry. `graphql-orm-ai` would own
only the typed capability, descriptor, approval, budget, event, and protected
result contracts. A host-provided broker would perform the actual browser
automation. No generic page-evaluation script, browser cookie access, arbitrary
local file upload, bearer-token injection, or reuse of an authenticated human
browser context would be exposed.

This work remains back-burner and is not a dependency of hosted web search.

## Cross-phase acceptance gates

- `cargo fmt --all -- --check`.
- Focused and full `graphql-orm-ai` tests for every affected explicit backend
  and provider feature; never workspace `--all-features`.
- Warnings-denied Clippy and Rustdoc for supported feature lanes.
- Existing provider, cancellation, application-tool, protected-stream,
  retention, restore, SemVer, release-policy, PascalCase, and dependency
  boundary checks remain green.
- Persistence changes have migrations, rollback/failure tests, restore parity,
  schema-module versioning, changelog, migration guide, and backend coverage.
- Public contracts remain project-neutral and include no consumer names, router
  credentials, application resolver documents, or host-specific policy.
- Security tests cover the strict negative space: no shell, filesystem,
  browser, screenshot, dynamic tool, arbitrary URL, arbitrary GraphQL,
  unregistered application tool, cross-owner provider state, or hidden
  chain-of-thought path.

## Current checkpoint

Phases 1 through 4 have project-neutral upstream contracts from the 0.69.0
development line, the retained Codex milestone is implemented in 0.70.0,
0.71.0 corrects canonical router transport of its tool manifests, and 0.72.0
aligns the strict adapter with Codex CLI 0.147.0 initialization. Version 0.73.0
completes the generated lifecycle envelope and live persistent-thread probe:

- a strict fresh-turn Codex app-server adapter with exact-run reuse, global and
  per-owner admission, cancellation/terminal cleanup, protocol allowlisting,
  and synchronous kill-on-drop;
- host-requested bounded visible summaries plus one protected ordered activity
  stream for text, summary, hosted-tool lifecycle, and citations;
- native OpenAI provider-retained mixed hosted-search/application-tool
  requests, explicit public/allow/block domain policy, exact citation
  provenance, and cumulative per-run web-search rule ceilings; and
- a private protected provider-session binding service with canonical
  transcript watermarks, current-principal/run fencing, exact cleanup/absence,
  session-retention dependency, and fail-closed portable restore audit.

- exact protected Codex thread create/resume/interrupt/delete, with process and
  provider retention governed independently; and
- default-off experimental app-server dynamic tools that route only through
  the existing coordinator-owned registered GraphQL tool boundary; and
- exact disabled-only remote-control status admission, truthful retained
  continuation capability negotiation, and explicit never-approval/read-only
  policy on every Codex thread create or resume; and
- strict positive signed notification timestamps, independently correlated
  thread response/start ordering, and the exact deletion-bound `notLoaded`
  transition observed during a live Codex CLI 0.147.0 create/delete handshake.

Phase 5 multiplexing and the visual-browser broker remain deferred
investigations. The current review boundary is full backend/provider,
documentation, SemVer, and release-policy verification for 0.73.0 / schema
module 0.55.0.

## Current milestone: retained Codex threads

This development line couples the existing provider-session persistence
contract to the strict Codex app-server adapter without widening the app-server
protocol into a generic bridge.

The milestone delivers:

- create a persistent Codex thread with no business content, bind its protected
  cursor under the exact current run, and only then begin the first turn;
- resume only an exact current owner/session/scope/profile/model/executable,
  protocol, policy, transcript-watermark, attempt, and lease binding;
- use `turn/interrupt` for durable cancellation and `thread/delete` for the
  existing cleanup-worker absence proof;
- keep one process per exact run while allowing the protected thread to outlive
  that process under independent retention limits;
- advance the durable provider-session watermark only after protected final
  assistant output, its checkpoint, and terminal run completion are committed;
  a retention-only commit failure quarantines the cursor without changing the
  completed answer; and
- invalidate the cursor after cancellation, transport ambiguity, stale policy,
  protocol failure, or output-persistence uncertainty.

Application-tool requests use app-server `dynamicTools` only through an
explicit experimental provider capability that is disabled by default and
bound into the registration, protocol, policy, request, and tool fingerprints.
The strict protocol actor admits only the documented `item/tool/call` server
request for an exact tool offered in the current `ModelRequest`. It forwards a
typed, bounded request through a coordinator-owned in-flight bridge; the
app-server adapter cannot answer it itself. The ordinary coordinator rechecks
the run fence, cancellation, current principal, rules, tool policy, egress,
budget, and resolver authorization, executes the exact registered GraphQL
operation, and returns only the disclosure-approved result. No bearer token,
delegated authority, raw router access, or generic request callback enters the
provider process.

Provider/tool ambiguity remains non-replayable. Cancellation, lease loss,
protocol failure, a stale tool definition, or a failed result handoff poisons
the process, invalidates the retained cursor, and moves the run through the
existing recovery path. Experimental dynamic tools do not weaken the ordinary
non-Codex provider continuation contract.

The milestone remains closed to hosted app-server web search, arbitrary
structured output, attachments, images, shell, filesystem, patches, MCP,
skills, collaboration, screenshots, browser control, raw reasoning, and every
server-initiated request other than the exact experimental dynamic-tool call.
Provider-hosted search continues to use the native OpenAI Responses path until
a separately reviewed Codex hosted-search protocol is available.
