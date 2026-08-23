---
title: "graphql-orm-ai"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-16
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-ai

A project-neutral, security-first AI runtime for `graphql-orm` applications.
It turns explicitly reviewed, server-authored GraphQL operations into agent
tools while keeping application authorization, disclosure, approvals, spend,
and durable history under host control.

It is not a chatbot server, a generic agent loop, a raw-SQL interface, an
arbitrary model-authored GraphQL executor, a shell runner, or an authorization
substitute. Application work always runs through the host's authenticated
GraphQL resolvers; a provider, tool registration, or approval never grants
resolver authority.

## Install

This active pre-release is Git-only. Pin one reviewed full monorepo revision
for AI, ORM, storage, backup, and tool-profile packages:

```toml
[dependencies]
graphql-orm-ai = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.93.1", default-features = false, features = ["sqlite"] }
```

Exactly one persistence backend is required: `sqlite` (default), `postgres`,
or `mssql`. MSSQL currently has schema/compile support only where ORM write
parity is incomplete. This unpublished crate has no docs.rs page; package docs
and rustdoc are available from the pinned repository revision.
Upgrade deliberately: review all pinned companions' migration guides and
changelogs, update them to one reviewed revision, and run recovery-path tests
before using the new pin with protected production data.

## Start safely

Start with the [staged getting-started path](docs/getting-started.md):

1. Compose `AiSchemaModule` in a test-owned SQLite host and use `MockProvider`
   with no tools, network, or secrets.
2. Add one reviewed provider under exact secret, egress, and budget policy.
3. Add default-deny read-only tools only when required.
4. Add approval-bound consequential work and production workers last.

There is intentionally no universal one-call “chat server” example: host schema
application, principal rehydration, GraphQL executor, content protection,
egress, and readiness are application proof boundaries. The guide identifies
the compiled test-backed recipe and the missing reusable bootstrap API.

## What it provides

- Managed AI schema records for protected sessions, messages, runs, budgets,
  provider calls, tool calls, approvals, audit, and restore readiness.
- Fenced durable provider turns, streaming output, bounded checkpoints,
  cancellation, recovery, retention, and current-principal rehydration.
- Watermark-bounded, contiguous durable session and owner-inbox replay whose
  `HasMore` contract remains correct at the configured ORM page maximum.
- Default-deny application tools with server-authored documents and static
  disclosure schemas; consequential work is exact-preview and one-shot
  approval bound.
- Backend-neutral automatic query capabilities compiled from the finished SDL
  and canonical semantic catalogue into closed typed plans. Nested
  relationships and opt-in aggregate roots remain explicitly bounded, and
  one read-only provider plan may expose these capabilities beside exact
  legacy/static read descriptors without duplicating either policy contract.
  Secret/`NeverExport` fields never enter provider schemas.
- Deterministic per-target compact capability indexes, one canonical
  collision-free multi-target index set, bounded lexical discovery,
  coordinator-selected eager/client-deferred/provider-deferred/fixed-broker
  delivery, and short-lived current-authority loaded bindings. See
  [capability discovery and execution](docs/capability-discovery-and-execution.md).
- One bounded owner-authorized conversation bootstrap combines the newest
  messages, durable watermark, active/recent runs, tool calls, provider
  activity and retention reset state for race-free replay/live handoff.
- An owner-authorized tool-call preview rehydrates current authority before
  returning host-projected arguments and either a disclosure-validated result
  or the exact content-free safe failure envelope. Secret results never enter
  this browser contract.
- Provider-neutral adapters plus deterministic network-free mocks.
- Optional provider profiles, attachments, skills, UI intents, rules, and
  usage/pricing controls, each behind independent proof and policy boundaries.

Detailed mechanics stay in the task-oriented [documentation index](docs/README.md)
rather than hiding the quick start behind an implementation inventory.

## Automatic GraphQL query execution

An owning subgraph builds `AiGraphqlQueryCapabilityCatalog` only after its SDL
and `GraphqlSemanticCatalog` are complete. `AiToolCatalog` registers that
complete catalogue for discovery and projects individual closed provider tool
definitions. When the provider returns a plan, call
`AiRuntime::execute_query_capability` with the stable ID, exact offered
capability fingerprint, typed plan and invocation context.

That boundary compiles the plan into one exact GraphQL document, variables,
selection/disclosure schema and dynamic descriptor. The authenticated bridge
then rehydrates the principal, invokes current target/tool policy, builds the
canonical request context, and lets the ordinary resolver authorize the call.
Registration alone remains default-deny. The runtime enforces descriptor byte
and total-record bounds and rejects response fields outside the selected
disclosure shape. Total records include sibling and nested relationship
expansion, not merely the largest returned list.

Private remote execution carries a crate-authored
`AiRemoteGraphqlCapabilityBinding` to the deployment issuer. Static reads bind
their exact registered descriptor; generated reads additionally bind the
active target, finished schema, semantic catalogue/root and offered capability
fingerprints. Issuers can therefore authorize an exact generated query without
interpreting its dynamic operation name. See
[private remote GraphQL execution](docs/remote-graphql-execution.md).

Mutation and subscription execution do not use the query path. The
backend-neutral profile package can compile a bounded `ReplayThenLive`
subscription observation. `OrmAiSubscriptionWaitService` binds that plan to an
authenticated registered replay source and the existing run queue; it
rehydrates current authority at open, event and adoption boundaries. See
[durable bounded subscription waits](docs/durable-subscription-waits.md).

## Session reliability

One bounded `aiConversationBootstrap` snapshot plus durable event replay is the
supported way to open a conversation. Its watermark is a **resume floor**:
nothing at or below it is missing from the snapshot, the message window never
leads it, and run and tool-call rows may already reflect a later event, so
replayed events are applied by identifier.

Session-event streams end with a typed close envelope rather than silence,
tolerate a briefly unavailable authorization dependency inside a bounded
jittered grace window while denying authoritative revocation immediately, and
run a periodic bounded durable head check so single-replica delivery does not
depend solely on the process-local wakeup channel.

Terminal `run_failed` and `run_recovery_required` events carry a bounded,
content-free failure record with a stable code and a retryable flag computed
from committed rows. `retryAiRun` authors a new run over the same durable user
message under current policy where re-execution is provably safe;
`acknowledgeAiRunFailure` dismisses a failure without deleting audit history.
Invalidating a retained provider thread emits `provider_session_reset` or
`provider_session_rebound` so a host can tell the user the model's context was
reset even though the durable transcript reads as continuous.

The reviewed Codex app-server adapter can retain a thread after Stop only when
the exact interrupt was acknowledged, no dynamic tool call remains unresolved,
and the ORM transaction proves that the cancelled turn persisted no assistant
message, tool call, or checkpoint. This discard guarantee is version-observed
for `codex-cli 0.148.0` with `gpt-5.4`; reverify it before upgrading Codex. Any
missing proof continues through the disclosed cleanup-and-rebind path.

The retained dynamic-tool launch profile is version-observed on Codex 0.148.0.
It disables Code Mode, Code Mode-only routing, shell, files, MCP, browser, and
every other native item surface by default. Native web search has a separate
default-off `with_web_search(bool)` profile setting and still requires an exact
request built-in, egress proof, supported PublicWeb/allow-domain policy, and
call ceiling. Its other sole process-level exception is `code_mode_host`:
`--disable code_mode_host` suppresses direct
`dynamicToolCall` delivery on that Codex version, so the launch arguments omit
only that flag while the per-thread configuration still sets the feature
false. When native search is enabled, the actor admits only its exact bounded
item lifecycle and exposes structured result metadata for host accounting and
audit; results enter model context before that completion event. Reverify both
direct delivery and the negative native-item matrix before upgrading Codex.

The Codex schema projector preserves bounded nullable scalar `type` arrays in
the crate-authored FixedBroker definitions. It does not pass through arbitrary
JSON Schema unions: only unique combinations of supported scalar types plus
`null`, with compatible enums and constraints, are admitted. Full-surface
readiness must project all three FixedBroker definitions before the host is
ready.

See the [session reliability adoption contract](docs/session-reliability-adoption.md).

## Features and capability boundary

| Feature | Default | Meaning |
| --- | --- | --- |
| `sqlite` | Yes | Managed schema and runtime lane. |
| `postgres` | No | PostgreSQL lane. |
| `mssql` | No | Schema/compile lane; check the capability matrix. |
| `provider-*` | No | Opt-in OpenAI, Anthropic, xAI, Ollama, or managed OpenAI-compatible adapters. |
| `local-harness` | No | Installed sandboxed JSON-lines v2 driver; not a generic subprocess launcher. |
| `provider-codex-app-server` | No | Experimental local adapter; deployment owns sandboxing, credentials, and network policy. |
| `graphql-case-pascal` | No | Changes the public GraphQL naming contract. |

Provider features do not configure a model, authorize egress, or disclose
data. Tool discovery and registration are not authorization. Every provider
call requires exact egress and atomic budget proof; every application tool
uses current principal rehydration, a static result disclosure contract, and
ordinary resolver authorization.

Each provider feature is independently buildable with one persistence backend.
Use `scripts/check-ai-provider-lanes.sh` from the repository root to verify one
feature at a time; provider feature unification is not required for a valid
adapter build.

## Budget capacity and stranded reservations

Reserved capacity counts against a budget ceiling exactly like committed
usage, so a reservation that never reconciles consumes the ceiling for the rest
of its policy period. `aiBudgetScopeCapacity` reports per-policy reserved and
committed amounts, ceilings, and a bounded list of unresolved reservations
under `ReadBudgetPolicies`. `reclaimAiBudgetReservation` resolves one expired
reservation whose owning run is terminal, under `ManageBudgetReclamation`,
recent MFA, an exact CAS version, and the
`with_budget_reservation_reclamation` deployment opt-in. It commits the held
estimate as authoritative usage rather than releasing it, because an
unreconciled reservation carries no proof that the provider was not reached.
A denial at reservation is pre-transport and certain: the run fails with
`provider_budget_denied` and stays retryable. See the
[usage and budgets guide](docs/usage-and-budgets.md).

A local stateless adapter can likewise close a refused provider-native item as
`provider_native_item_rejected` only after the completed turn's authoritative
usage is committed and both adapter and executor prove there was no admitted
answer or host tool effect. That failure is retryable. Incomplete, retained, or
otherwise ambiguous provider turns remain recovery-required.

## Reasoning effort profiles

`ModelReasoningEffort` is the closed provider-neutral selection:
`Unspecified`, `None`, `Low`, `Medium`, `High`, `XHigh`, or `Max`.
`Unspecified` omits a provider override; explicit `None` does not. Hosts define
the reviewed exact-model set and default with
`ModelReasoningEffortProfile::new`, install profiles on the native OpenAI
configuration or exact Codex registration, and expose only
`profile.supported()` to settings clients.

Set the same selected value on `ModelRequest::reasoning_effort` and
`AiBudgetReservationRequest::reasoning_effort`. The runtime rejects an
explicit value absent from the active model profile before provider execution.
For retained Codex sessions without capability delivery, create the
provider-session descriptor with
`registration.provider_session_fingerprint(selected_effort)?`. A retained
capability-delivery host instead constructs the exact
`AiProviderCapabilitySessionBinding` from that raw fingerprint, installs the
binding with `registration.with_capability_session_binding(binding.clone())?`,
and persists a descriptor created by
`AiProviderSessionDescriptor::new_with_capability_binding`. The adapter
validates the binding's embedded raw identity and then recognizes its complete
fingerprint for create, resume, and cleanup. The app-server effort override
affects subsequent turns, so changing effort requires exact cursor cleanup and
rebind. Effort selection neither enables visible reasoning summaries nor
grants tools, egress, filesystem, shell, browser, MCP, approval, or mutation
authority. See the
[provider-session guide](docs/provider-sessions-and-hosted-activity.md) and
[migration guide](MIGRATION.md).

## Configuration, operations, and errors

The [configuration and limits catalogue](docs/configuration.md) lists every
public service limit and provider configuration source without fabricating
defaults. Runtime work is fenced and fail-closed: uncertain effects require
recovery rather than replay. Provider or worker errors never carry a license
to retry an action with side effects.

Hosts own secrets, endpoint policy, principal lifecycle, deployment rules,
GraphQL resolvers, migration application, operational scheduling, and restore
readiness. The crate owns its protected records, bounded orchestration
contracts, and security checks.

## Further reading

- [Documentation index](docs/README.md)
- [Getting started](docs/getting-started.md), [architecture](docs/architecture.md), and [security model](docs/security.md)
- [Backend capability matrix](docs/backend-capability-matrix.md)
- [Read-only tools](docs/read-only-tool-loop.md), [supervised mutations](docs/supervised-tool-loop.md), and [provider turns](docs/worker-provider-turn.md)
- [Recovery and restore](docs/recovery-and-restore.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
