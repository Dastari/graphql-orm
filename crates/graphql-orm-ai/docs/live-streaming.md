---
title: "Protected Live Streaming and Provider Activity"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-01
supersedes: []
---

# Protected Live Streaming and Provider Activity

Provider output and hosted activity can be exposed as bounded provisional
session events without
turning raw transport frames into an unaudited client channel. This path is
optional. Without an `AiLiveDeltaSink`, provider execution retains its normal
bounded result and only the final protected assistant message becomes durable.

## Data path

The built-in path is deliberately ordered:

1. the normalized provider adapter yields visible text, a provider-generated
   visible summary, hosted-tool lifecycle, or validated citation metadata;
2. `AiProviderActivityCoalescer` creates UTF-8-safe visible batches within the deployment
   limits, which may be no weaker than 50 milliseconds or 4 KiB;
   pending visible content is flushed before every structured event;
3. `OrmAiLiveDeltaService` rehydrates the current principal and checks current
   scope and session write access;
4. it resolves the current ready content-protection policy, protects the batch,
   then rehydrates and resolves policy again to reject asynchronous drift;
5. one state-machine transaction validates the exact active run, worker,
   attempt, generation, unexpired lease, provider/model, uncertain budget
   reservation, active session owner, and scope;
6. that transaction advances both the session and owner-inbox cursors, inserts
   protected `provider_activity` events, and queues commit-only wakeups; and
7. an authorized subscription wakes after commit and re-reads the event through
   the ordinary protected cursor window.

The sink awaits each batch sequentially. This is intentional backpressure:
client-visible durability cannot fall behind an unbounded provider stream.
The sink transaction does not renew or rotate the worker lease. Coordinator
heartbeats remain authoritative, while state-machine transaction isolation
serializes each fence check with recovery and other run transitions.

## Enabling the sink

Construct the ORM sink from the same runtime and run service used by the worker,
then install it explicitly on the provider executor:

```rust,no_run
# use std::sync::Arc;
# use agql_auth::{Clock, SystemClock};
# use graphql_orm_ai::{AiLiveDeltaCoalescerLimits, AiLiveDeltaPersistenceLimits,
#     AiProviderCallExecutor, OrmAiLiveDeltaService};
# use time::Duration;
# fn configure(
#     executor: AiProviderCallExecutor,
#     run_service: graphql_orm_ai::OrmAiRunService,
#     runtime: Arc<graphql_orm_ai::AiRuntime>,
# ) -> Result<AiProviderCallExecutor, graphql_orm_ai::AiError> {
let persistence_limits =
    AiLiveDeltaPersistenceLimits::new(4_096, Duration::seconds(30))?;
let sink = Arc::new(OrmAiLiveDeltaService::new(
    run_service,
    runtime,
    Arc::new(SystemClock) as Arc<dyn Clock>,
    persistence_limits,
));

let executor = executor.with_provider_activity_sink(
    sink,
    AiLiveDeltaCoalescerLimits::default(),
);
# Ok(executor)
# }
```

The coalescer and persistence byte limits should agree. A stricter persistence
limit is allowed but will fail closed if a generated batch exceeds it.

## Durable event contract

The protected `provider_activity` payload currently has `formatVersion: 1`
and includes:

- `provisional: true`;
- exact run, attempt, lease generation, and batch sequence;
- provider kind, model, and optional response reference;
- the exact uncertain budget reservation ID;
- one typed activity kind: `text`, `reasoning_summary`,
  `hosted_tool_started`, `hosted_tool_completed`, or `citation`;
- text and UTF-8 byte count only for visible text/summary; hosted call ID and
  closed kind only for lifecycle; or validated HTTPS/output-span metadata only
  for a citation.

The containing session event supplies the authoritative monotonic cursor,
correlation ID, run ID, and causation link. Consumers fetch it only through an
authorized session-event page or subscription. Protected persistence does not
make a batch public and does not authorize any provider or third-party egress.

The path structurally excludes application-tool arguments/results, hosted-tool
result bodies, raw provider frames, hidden chain-of-thought, credentials, and
arbitrary model metadata. A custom `AiProviderActivitySink` has the same
security obligations and must not weaken these exclusions. The older
`AiLiveDeltaSink` remains source compatible for text/summary-only integrations
and writes `provider_live_delta`; installing the activity sink supersedes it
for one executor rather than writing duplicate progress streams.

## Client reconciliation

`provider_activity` is progress, not a final message. A client may render its
text in a virtualized transient view keyed by run, attempt, and batch sequence.
When `assistant_message_completed` arrives, replace the provisional rendering
with the authoritative windowed message and blocks. Do not append both copies.

If the run becomes `RecoveryRequired`, retained provisional events describe an
incomplete historical attempt. They must not be relabeled as a complete answer.
Cursor pagination remains stable, so a client can discard old DOM nodes and
re-fetch a bounded window without holding the full session in memory.

## Failure and recovery

Once transport begins, the exact budget reservation is uncertain. Protection,
authorization, fencing, or persistence failure returns an error and leaves it
uncertain for privileged reconciliation. The worker must not replay that
provider call. A stale worker cannot append after lease expiry or recovery
because the durable transaction re-reads the exact current fence.

The host-only `OrmAiSessionRetentionService` deletes expired
`provider_live_delta` and `provider_activity` rows in bounded per-session transactions under the exact
current GraphQL-managed scope policy. It never rewinds or reuses sequence
values. A later replay that crosses a removed sequence returns
`reset_required`, so the client discards provisional rendering and reloads its
bounded authoritative windows. Stable lifecycle/completion events are retained
by ordinary delta expiry, but become eligible after the separately configured
deleting-session cutoff. See the
[session-retention guide](session-retention.md).

Visible-summary requests, hosted search, citations, and the durable provider
session lifecycle are documented together in
[provider sessions, hosted search, and visible activity](provider-sessions-and-hosted-activity.md).
