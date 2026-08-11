---
title: "Provider Sessions, Hosted Search, and Visible Activity"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
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
| Codex app-server v1 | disabled | process memory for one run only | not supported |
| Native OpenAI hosted search | absent from each request | Responses continuation only when selected | mixed retained continuation supported |
| Visible reasoning summary | disabled | protected activity/final blocks | authority-neutral |
| `AiProviderSessionService` | not constructed | protected opaque cursor | authority-neutral |

The Codex app-server adapter and durable provider-session service are not
automatically coupled. A process may be warm without a retained provider
thread, and a retained thread may exist while no process is running.

## Exact run-scoped app-server

Enable `provider-codex-app-server` to use the first-phase strict adapter. The
host supplies:

1. `AiCodexAppServerRegistration`, binding the logical profile/model,
   executable digest/version, sandbox profile, and exact protocol version;
2. `AiCodexAppServerRunLimits`, including global, per-owner, per-run turn, and
   startup/turn/interrupt/shutdown ceilings;
3. an `AiCodexAppServerRunProcessFactory`; and
4. `AiCodexAppServerRunPool` and `AiCodexAppServerProvider`.

The factory is a trusted deployment boundary. It must execute the verified
image directly, never through a shell or path search; clear inherited
environment and credentials; use an empty isolated working directory; deny
unreviewed network, filesystem, child-process, socket, and credential access;
and return `AiCodexAppServerLaunchedProcess` with an idempotent synchronous
process-tree kill callback. The wrapper invokes that callback on final drop,
including an abandoned stream or failed graceful shutdown.

The crate-owned protocol actor deliberately has no generic JSON-RPC send
method. It admits only initialization, fresh thread start, fresh turn start,
turn interruption, exact correlated responses, and the closed visible-event
allowlist. Unknown notifications and server requests fail closed. Dynamic
tools, commands, shell, files, patches, MCP, collaboration, images, web search,
browser control, raw reasoning, and arbitrary methods are forbidden.

The v1 adapter accepts an initial `StatelessReplay` request containing only
bounded trusted instructions and text. Each call gets a fresh ephemeral Codex
thread; only the process is reused for the exact session/run/attempt/lease
generation. It does not yet support an application-tool continuation or a
resumable Codex thread. The JSONL local harness remains the implemented local
path for exact stateless application-tool loops.

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

The safe creation order is:

1. create an empty provider thread and retain a provider-side deletion guard;
2. construct the immutable `AiProviderSessionDescriptor` and canonical
   transcript-prefix fingerprint;
3. call `bind_for_run` under the current run lease;
4. only after the protected binding commits, send business content; and
5. after protected assistant output and its matching
   `assistant_output_persisted` checkpoint commit, call `commit_turn` with the
   new authoritative watermark/fingerprint.

Resume uses `claim_for_run` and then `open_for_run`. Both require the exact
descriptor and transcript evidence, current principal/session/scope access,
and current run fence. A crash, cancellation, protocol error, ambiguous
provider state, output-persistence failure, policy/profile/model/executable
drift, or rejected cursor calls `require_cleanup`; v1 never guesses provider
state or advances a watermark from incomplete evidence.

A managed cleanup loop performs:

1. `claim_cleanup(worker_id)`;
2. `open_for_cleanup` under the exact maintenance protection policy;
3. registered `AiProviderSessionDeletionService::delete_or_confirm_absent`;
4. `complete_cleanup` with the exact cursor-bound absence proof; or
5. `schedule_cleanup_retry` with a bounded safe reason and delay.

Session deletion fences active bindings and cannot finalize until the row is
absent. Expiry and a backup redaction marker are not absence proof. Portable
backup redacts the cursor and the required restore audit blocks readiness when
any binding exists; drain provider sessions before a portable backup intended
for ready restore. Normal process restart may resume an exact live binding,
but raw or portable database restore never does.

## Deferred visual browser

A future visual browser is a separate capability broker, not hosted search and
not an app-server method. Its observation-first, one-shot, host-isolated design
is recorded in [the visual-browser broker plan](visual-browser-broker.md).
No browser capability is implemented by the contracts in this guide.
