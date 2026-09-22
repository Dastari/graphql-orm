---
title: "Installed Local Harness Boundary"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-01
supersedes: []
---

# Installed Local Harness Boundary

The optional `local-harness` feature supports installed model or agent programs
without turning them into a shell tool. It is separate from Ollama and other
local HTTP providers: those use fixed endpoint adapters, while an installed
program uses an immutable deployment registration and a trusted process-tree
launcher.

The implemented foundation is suitable for a narrow text/structured-output
harness and may opt into exact stateless application tools. It deliberately
does not grant coding-workspace, filesystem, terminal, MCP, provider built-in,
attachment, network, credential, arbitrary callback, or provider-retained
continuation authority.

The separate `provider-codex-app-server` feature adds a second local boundary:
one strictly allowlisted process may be retained for an exact claimed run, and
an explicitly planned turn may resume one protected provider-thread cursor.
Experimental app-server dynamic tools are default-off and use only the
ordinary coordinator-owned registered GraphQL tool path. Native web search is
separately default-off and may be enabled by an immutable launch profile plus
an exact request built-in, egress proof, supported domain policy, and call
ceiling. Structured output, files, images, shell, MCP, browser control, and
generic JSON-RPC remain unavailable. Its construction and exact limitations are in
[provider sessions, hosted search, and visible activity](provider-sessions-and-hosted-activity.md).

## Authority split

`AiLocalHarnessRegistration` fixes:

- the server-authored logical model name;
- a normalized absolute executable and fixed argument vector;
- a required lowercase executable SHA-256 and reviewed version;
- an isolated absolute working directory and named sandbox profile;
- common narrow provider capabilities; and
- request/frame/stdout/stderr/count, startup/turn/shutdown, memory, and CPU
  ceilings.

The type has no environment, secret, mount, URL, network-mode, user argument,
or shell field. It is not serializable and is not accepted through GraphQL.
`AiLocalHarnessRegistry` rejects duplicate models, more than 128 entries, and
capability differences between registrations. GraphQL provider configuration
may enable or scope a `LocalHarness` logical profile but must supply no base
URL and cannot alter deployment process facts. Credential set/rotation is
rejected, and an already credentialed provider profile cannot be converted to
`LocalHarness` until its credential is removed through the audited lifecycle.

Registration validation is not sandbox proof. A trusted implementation of
`AiLocalHarnessProcessLauncher` must:

- atomically verify and execute the same registered image by digest;
- directly execute it without a shell or path lookup;
- use only the registered arguments and clear the complete inherited
  environment;
- supply no user bearer token, provider key, SSH/cloud agent, home directory,
  socket, TTY, or ambient stdin;
- deny network and prevent access outside the reviewed OS/container sandbox;
- enforce memory, CPU, wall-time, output, concurrency, and process-count
  ceilings;
- own the complete descendant tree; and
- synchronously initiate forced tree termination when the process handle is
  dropped before a proven exit.

The crate does not include a generic `std::process::Command` or
`tokio::process::Command` implementation because those properties require a
deployment-specific OS/container boundary. A plain child process with inherited
environment and best-effort child-only kill does not satisfy the trait
contract.

## Protocol and provider path

`AiJsonLinesLocalHarnessDriver` writes exactly one bounded JSON line:

```json
{
  "protocol": "graphql-orm-ai/local-harness-jsonl/v2",
  "type": "request",
  "model": "deployment-logical-name",
  "instructions": ["trusted runtime instruction"],
  "input": [{"type": "text", "text": "authorized content"}],
  "continuation_mode": "stateless_replay",
  "continuation": null,
  "tools": [{
    "tool_id": "records.read",
    "provider_name": "records_read",
    "fingerprint": "reviewed-fingerprint",
    "description": "Read one authorized record",
    "parameters": {"type": "object", "additionalProperties": false},
    "strict": true
  }],
  "output_schema": null,
  "maximum_output_tokens": 256
}
```

It then closes stdin. Stdout is arbitrary transport chunks containing newline-
terminated serialized `ProviderEvent` values. The driver accepts one
ordered sequence of:

1. `response_started` without a response ID;
2. zero or more visible `text_delta` values and bounded tool calls, where each
   call uses an exact offered local tool ID and follows
   `tool_call_started` → optional argument deltas → one object-valued
   `tool_call_completed`;
3. one usage event after every started call is complete and within registered
   context/output token ceilings; and
4. `response_completed` without a response ID, followed by successful process
   exit.

Every line, total stdout, discarded stderr, frame count, startup, turn, and
shutdown is bounded. A partial line, malformed JSON, excessive counter,
duplicate/out-of-order terminal or tool event, unknown/unoffered tool ID,
response ID, reasoning event, citation, built-in request, unknown event, output
overrun, timeout, or unsuccessful exit fails closed. Raw stderr and
process/request content are never placed in a `ProviderError`.

Tool-capable registrations must set both `custom_tools` and
`stateless_continuation`; `parallel_tool_calls` may then be enabled. The
request's protected continuation contains only the original trusted
instructions, visible text/JSON, exact assistant calls, and
disclosure-validated tool outputs. Each replayed tool output has a distinct
fresh egress proof. The harness cannot ask the server to invoke an arbitrary
callback: normalized calls still flow through the ordinary durable
`graphql-orm-ai` tool service and authenticated GraphQL resolver path.

`AiLocalHarnessProvider` first validates the ordinary `ProviderRequestContext`
as `ProviderKind::LocalHarness`, including the exact current call's atomic
budget and model-inference egress proofs. The normal `AiProviderCallExecutor`
still performs principal rehydration, session/scope access, immutable egress
audit, uncertain-boundary accounting, output limits, usage settlement, fenced
checkpoints, and protected transcript persistence. “Local” does not mean free,
trusted, or exportable.

## Construction outline

```rust,no_run
use std::sync::Arc;

use graphql_orm_ai::{
    AiJsonLinesLocalHarnessDriver, AiLocalHarnessLimits,
    AiLocalHarnessProcessLauncher, AiLocalHarnessProvider,
    AiLocalHarnessRegistration, AiLocalHarnessRegistry, ProviderCapabilities,
    ProviderError,
};

# fn build(launcher: Arc<dyn AiLocalHarnessProcessLauncher>) -> Result<(), ProviderError> {
let capabilities = ProviderCapabilities {
    streaming: true,
    structured_output: true,
    custom_tools: true,
    parallel_tool_calls: true,
    stateless_continuation: true,
    local: true,
    maximum_context_tokens: Some(8_192),
    maximum_output_tokens: Some(1_024),
    ..ProviderCapabilities::default()
};
let registration = AiLocalHarnessRegistration::new(
    "local-reviewed-model",
    "/opt/reviewed/bin/model-harness",
    vec!["--json-lines".to_owned(), "--single-turn".to_owned()],
    "/var/empty/model-harness",
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "reviewed-1.0.0",
    "isolated-no-network-v1",
    AiLocalHarnessLimits::default(),
    capabilities,
)?;
let registry = AiLocalHarnessRegistry::new([registration])?;
let driver = Arc::new(AiJsonLinesLocalHarnessDriver::new(launcher));
let provider = AiLocalHarnessProvider::new(registry, driver);
# let _ = provider;
# Ok(())
# }
```

Register the provider under `ProviderKind::LocalHarness`. The request model is
the logical registration name, never an executable path. Do not expose the
registration getters to GraphQL or model-authored configuration.

## Deterministic conformance

The repository tests use an in-memory fake process/launcher. They prove that
the process request does not contain executable, arguments, sandbox identity,
or stderr; fixed launch facts cannot be swapped by model input; model/budget
proof swaps fail before launch; arbitrary output chunk boundaries normalize;
unsafe capabilities and unoffered/malformed tool events fail; stateless
history and exact definitions use v2 framing; stderr and partial-frame limits
terminate; and dropping a partial stream exercises the required
kill-on-drop path. The suite starts no subprocess, contacts no model/provider,
and opens no database.

ACP framing, general mediated callbacks, resumable process sessions, and any
separately sandboxed coding workspace remain future adapters. Exact completed
stateless tool batches can cross a worker generation through protected ORM
adoption; this revalidates stored history and never resumes or reconstructs a
process session. Future adapters must not widen this safe registration
implicitly.

## Grok ACP adapter and admission boundary

The opt-in `provider-grok-acp` feature supplies `AiGrokAcpProvider`, immutable
`AiGrokAcpRegistration`, and `AiGrokAcpWireProcess`. The deployment supplies a
bounded `AiGrokAcpWireTransport`, isolated process factory and whole-tree kill
closure. The actor owns initialization, existing cached-login selection, scalar
model/effort configuration, streaming, SDK MCP, cancellation, resume and deletion.
No credentials or subprocess are discovered by this crate. Explicit effort is
required; unknown provider defaults cannot drift. Bootstrap is an owned, validated
host-policy string frozen into the registration, never a per-request override.

### Budgets and terminal accounting

Grok uses the existing estimate/reserve/actual reconciliation contract, as does
the Codex app-server adapter. `maximum_output_tokens` bounds the admitted request
estimate, not native sampler output. Complete aggregate usage can exceed that
estimate and is committed in full; it is never truncated to reserved amounts.
The ORM budget service then applies committed usage to future admission. Tests
exercise actual input 29,390/output 3,000 against estimates of 100 each. Missing,
incomplete or inconsistent usage is uncertain, never zero. Provider prices and
cost ticks do not replace host usage accounting. A failed/cancelled terminal
remains uncertain even if a usage event preceded it; existing reconciliation owns
recovery. It is not reported as a successful settled response.

An earlier draft incorrectly required a native hard output ceiling and rejected
actual tokens above the estimate. Grok 1.0.40 does exceed configured native output
settings (cap 1 returned 57 tokens; cap 64 returned 92), but the pinned Codex wire
also does not forward its requested output estimate as a native token cap. Those
observations are a limitation of token-budget precision, not a different Grok
admission contract. No token/spend guarantee or saved-setting increase is implied.

The registration separately bounds input **bytes**, native model rounds and
requested output estimates. Usage counters have independent protocol/storage
sanity bounds; token counts are not compared to byte bounds. The host selects a
native `maxTurns` no larger than its authorized remaining provider-turn allowance;
this adapter admits at most 64 native rounds and one prompt per process/binding.
A tool conversation can consume more native rounds than broker callbacks. The
host must not schedule additional hidden prompts or silently raise saved limits.
Broker callbacks still pass through ordinary run/tool/result/authorization checks.
The provider bounds process capacity, one active process per owner, startup and
turn time (at most one hour), 16 MiB frames, 64 MiB transport/broker byte budgets
and up to 4096 SDK callbacks independently of the 64-definition limit. Native
tool identifiers are capped at 8192 and incoming frames at 65,536; host tool/run
budgets remain authoritative. The existing coordinator additionally admits at
most 64 dynamic callbacks per dispatch (or fewer under configured host limits).
One ACP prompt is one dispatch; the transport allowance does not authorize 1024
application calls from a larger run limit or invent native subround admissions.
Dropped launch, empty-session creation, activation
and stream futures terminate their process trees.
Cancellation/close races during launch cannot install a process after cancellation.

### Native isolation and retained state

`AiGrokAcpProcessFactory::admits` defaults to false until the trusted host verifies
its exact executable/profile. Registration hashes executable/version, sandbox,
model, effort, usage alias, bootstrap, tool fingerprints and operational bounds.
Before admission, `with_retained_namespace` must bind a SHA-256 digest of the
canonical retained runtime path and stable login identity. It must remain stable
across model/effort changes and change when storage or account changes. Cursor
kinds carry that namespace; cleanup rejects a moved namespace before launching,
so native idempotent deletion cannot falsely prove absence in a different home.
Capability overlays require the three canonical broker definitions for the exact
index plus the binding's exact static bootstrap fingerprint set. Generated broker
definitions are not counted as static catalogue entries. The host must prove
private home/cwd, existing managed cached authentication, absence of ambient
instructions/plugins/hooks/MCP, native filesystem/shell/subagent exclusion, and
policy-compliant web access. It must disable automatic title-refresh/turn-summary, memory and
`GROK_TWO_PASS_COMPACTION` (`features.two_pass_compaction=false`) to prevent
unobservable background compaction prefire. Ordinary compaction-start events
fail closed and mark the turn uncertain; cancellation is not a precharge fence.
Other optional side work must also be disabled. No API-key fallback or billing-path switch occurs.

Before returning a new empty cursor, the actor selects the model/effort, assigns
one fixed host title, closes and resumes the session. This prevents the native
first-content title sampler. Resume repeats the frozen curated profile and
`yoloMode` setting. Only an exact manual-title notification is admitted; generated
summaries, compaction, background tasks and unknown execution events fail closed.
Every message is transported inside a reversible JSON-string text envelope,
with a fixed non-command prefix and literal at-signs encoded as JSON Unicode
escapes. This prevents native slash-command dispatch and implicit file-reference
expansion before tool admission, while preserving original message content for
the model. Prompt blocks never carry native bash/control metadata. Encoded text
still obeys the registration byte bound. Internal thought chunks and signatures are discarded. No payloads or credentials
are logged. Exact bounded `skills-reload` internal acknowledgments are recognized
without exposing any ambient skills or authorizing tools.

The native profile includes only `GrokBuild:search_tool` and
`GrokBuild:use_tool`; an empty inherited allowlist is not safe. The SDK bridge
answers the initial `server/discover` probe with method-not-found, then handles
MCP initialization, exact `tools/list` and offered `tools/call`. Legacy fallback
has its own inner correlation generation; ACP outer IDs remain unique. Broker
state resets only after confirmed empty-session close before resume. Tools during
resume are rejected, and historical messages are never re-emitted or executed.
Only opaque durable `ProviderDynamicToolResult` values can return tool results.
The coordinator remains authoritative for resolver access, budgets and disclosure.

Retained cursors remain opaque and fenced by the existing session service.
Interruptions require cleanup/recovery, never blind continuation. Detached cleanup
shares process capacity and requires `_x.ai/session/delete` success; process death
alone never proves absence. Live probes verified idempotent deletion and a later
resume failure, using only synthetic data and the existing managed login.

### Verification

The official source audit used
[xai-org/grok-build commit 4247f66](https://github.com/xai-org/grok-build/tree/4247f661689354b831191f11eeeac8424993fe3d).
Ignored opt-in live tests run the actual upstream actor over an explicitly supplied
isolated Unix socket; the external host owns process isolation and existing login.
They exercise synthetic discover/describe/execute, streamed text and complete
usage, process-restart resume without repeated callbacks, retained follow-up,
Stop uncertainty and exact deletion. No provider is enabled by running a test.
Deterministic tests cover correlation/replay/schema boundaries, registration,
unknown/native execution, title preflight, actual-over-estimate accounting,
uncertain terminals and cancellation lifecycle races. No database schema or data
migration is introduced. Consumer deployment still requires its own authorization,
isolation, runtime/restart workflow and end-to-end acceptance.
