---
title: "Session reliability adoption contract"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-20
review_by: 2027-02-01
supersedes: []
---

# Session reliability adoption contract

This is the exact contract for the 0.82.0 session-reliability work: which
public APIs are new or changed, what a client must do differently, and which
behaviours changed with no API change at all. It complements
[MIGRATION.md](../MIGRATION.md), which records the schema and source-breaking
facts.

The guardrail behind every decision here is unchanged: **not every error is
recoverable.** Where it is uncertain whether protected output was persisted,
whether a provider saw a response, or whether a consequential operation
occurred, the run keeps `RecoveryRequired` and nothing below fabricates an
absence proof, a cursor, or settled state.

## Conversation bootstrap: the watermark is a resume floor

`conversation_bootstrap` previously reassembled its snapshot whenever the
session's `row_version` or `stream_head` changed between two reads. Every
coalesced live delta advances both at roughly the streaming coalescer rate, so
the bounded snapshot failed structurally for any session with an assistant
currently answering, and the caller saw `Conflict`.

The retry predicate now covers only fields the bootstrap actually returns. The
stream head, last-activity timestamp, and CAS version are excluded: a live
delta appends a session event and changes nothing in the returned payload.

The resulting watermark contract is:

| Guarantee | Holds |
| --- | --- |
| Every durable effect at or before `watermark` is in the snapshot | Yes |
| The message window may lead `watermark` | No |
| Run and tool-call rows may lead `watermark` | Yes |
| `after_sequence = watermark` can miss an event | No |

**What a client must do.** Subscribe with `after_sequence = watermark`, exactly
as before. Apply replayed events *by identifier*. A run or tool-call event
replayed just after the snapshot may describe a row the snapshot already
reflects; both are identified state, so re-applying is idempotent. A client
that assumed every replayed event was previously unseen must be updated.
Messages are unaffected, because a new message changes the message head and
forces the snapshot to be reassembled.

## Session-event streams: typed close, reauthorization grace, durable fallback

`AiSessionEventEnvelope` gained `closed: Option<AiSessionStreamClose>`. It is
`None` on every ordinary delivery and set on the final envelope of any stream
the server ended:

| Value | Meaning | Client action |
| --- | --- | --- |
| `ResetRequired` | Retention removed needed history | Discard derived state, reload from bootstrap |
| `WakeupChannelClosed` | Host wakeup channel closed, normally shutdown | Resubscribe from the last delivered watermark |
| `AuthorizationRevoked` | Authoritative denial, or session no longer visible | Do not resubscribe with the same credentials |
| `ReauthorizationUnavailable` | Grace window expired with the dependency down | Resubscribe after backing off |

A stream that ends with no close envelope and no error is a client unsubscribe
or a transport failure. `reset_required` stays coupled to `ResetRequired`, and
the existing `AiError` still follows the close envelope, so a client reading
only the boolean or only the error keeps working.

Reauthorization now distinguishes an unavailable dependency from a denial.
Only `AuthServiceUnavailable`, `Store`, `AuthThrottled`, and `AuthLocked` are
retried, inside a bounded grace window with per-session jittered backoff so one
authorization restart cannot produce a synchronized bootstrap storm. Every
other class, including one this crate does not recognize, denies immediately.
Both bounds are crate-owned and configurable through
`with_reauthorization_grace` and `with_replay_check_interval`.

A periodic bounded head-sequence read (`session_stream_head`) now runs
independently of the wakeup channel. Single-replica delivery therefore no
longer depends solely on an in-process `tokio::broadcast`, and a dropped or
missed hint is recoverable rather than terminal.

**What multi-replica delivery would additionally require**, and what this
release deliberately does not supply: the head check makes *one* replica
self-healing, but the wakeup hint is still process-local, so a subscriber on
replica A learns about a commit from replica B only at the next poll. Bounded
staleness, not loss. Real multi-replica delivery needs a cross-process commit
notification — a database `LISTEN`/`NOTIFY` channel or an external bus — fanned
out to subscribers, plus a shared retention floor so a replica cannot serve a
watermark another replica has already pruned past. Neither is in this release.

## Failed and recovery-required runs

`run_failed` and `run_recovery_required` events already existed and were
already emitted in the same transaction as the terminal run write, through the
same sequenced replayable channel as every other session event. What they
lacked was any classification.

The payload advances from the tagged `...-v1` shape to `...-v2` and adds a
`failure` record, mirroring the existing safe failure envelope:

```json
{
  "version": 1,
  "ok": false,
  "code": "provider_turn_uncertain",
  "retryable": false,
  "admission": "refused_uncertain"
}
```

`failure` is `null` for `run_completed`. Readers accept v1 and v2 and fail
closed on anything else, so events written before this release stay readable.
The record carries only server-owned classification, never provider content.

**Retry admission** is computed from committed rows and means "a new run may be
authored for the same durable user message", not a state-machine transition:

| Terminal state | Admission |
| --- | --- |
| `RecoveryRequired` | Never. Re-execution is what the guardrail forbids |
| `Completed` | Never. The message already has its answer |
| `Cancelled` | Only when the run produced no durable assistant message |
| `Failed` | Only for an explicitly allowlisted, proven-clean code |

`Cancelled` is deliberately not a class. Cancellation is observed at two
points with opposite correct answers: after the assistant output was durably
persisted the message is fully answered and a second run would produce a second
answer, while before persistence the result was discarded and retry is
meaningful. Read the flag rather than the event type.

An absent or unrecognized failure code is refused. The allowlist is opt-in, so
adding a new failure classification cannot silently make it retryable.

## Retry and acknowledge

`retryAiRun` and `acknowledgeAiRunFailure` require
`Arc<dyn AiRunDispositionService>` in schema data;
`OrmAiRunDispositionService` is the generated-ORM implementation.

Retry authors a new `queued` run over the **same** `input_message_id`. It never
duplicates the prompt, never resumes the source run, and carries a fresh
principal reference so it executes under current policy rather than the
authority the source run captured. Admission is re-decided from committed rows
inside the same transaction that authors the new run.

Acknowledge is always available for a terminal failed or recovery-required run,
*including one whose retry is refused*: dismissing a failure asserts nothing
about whether re-execution would be safe.

Both are idempotent under `clientRequestId` and at most one disposition wins
per run; a second key for an already-disposed run conflicts. Neither deletes a
row or an event, so the source run, its immutable attempt outcomes, and its
durable session and inbox events all survive. `run_retry_queued` and
`run_failure_acknowledged` are appended to the ordinary session stream.

## Retained provider sessions

**Every invalidation is now disclosed.** Each funnel that marks a retained
binding cleanup-required, and each explicit rebind, appends a durable session
event carrying only the server-owned reason class:

- `provider_session_reset` — the retained thread stopped being usable;
- `provider_session_rebound` — a new binding replaced a prior one.

This closes the case where the durable transcript rendered as continuous while
the model had silently lost all prior context. The payload uses the existing
content-free tagged envelope, so it discloses no cursor, prompt, provider
payload, tool argument, or authorization detail. A host should tell the user
the model's context was reset when it sees either event.

Reason classes worth distinguishing when rendering: cancellation after a turn,
a changed rule fingerprint, an incomplete dynamic turn, and an exceeded budget
are all ordinary user behaviour rather than faults.

**Interruption reports what it proved.** `AiRunInterruptSettlement` replaces
`()` from `interrupt_run`. `retains_thread()` is true only for `Settled`, which
no adapter currently reports, so it fails closed to invalidation.

Acknowledgement is not settlement. The Codex app-server `turn/interrupt`
response is an empty object, `TurnStatus` has a first-class `interrupted`
value, and a resumed thread pages prior turns back through
`thread/turns/list` — so an acknowledgement cannot distinguish a discarded
partial turn from a retained one. Treating it as settlement would let the model
carry content the durable transcript never recorded, which is the same
divergence the disclosure events above exist to expose. The variant exists so
an adapter that can prove settlement may report it without a further breaking
change.

Interrupting an in-flight turn already invalidates the retained binding through
the executor's own ambiguous-turn cleanup. That path is now *disclosed* rather
than silent, so a mid-generation stop is visible to the user.

## Messages accepted during cleanup

A message accepted while provider-session cleanup is pending converges without
operator intervention, and did so across a host restart before this release:
the deferred turn is scheduled as a durable retry, and `claim_next` reclaims
queued and retry-scheduled runs. The `Deferred` outcome is a report, not the
delivery mechanism, so delivery never depended on it reaching the executor.

What changed is the end of that allowance. Exhausting the bounded retry
allowance while cleanup stayed pending previously propagated a conflict and
left the run running until its lease expired into `RecoveryRequired` — both
misclassified, because nothing had executed, and stuck until an operator looked
at it. The run now closes as `Failed` with
`provider_session_cleanup_unavailable`, which the classifier admits for retry.
A stale fence still fails the terminal write, so ordinary expired-lease
reconciliation keeps owning that case.

## Summary of public API changes

| Item | Change |
| --- | --- |
| `AiSessionEventEnvelope` | Added `closed`; construct via `delivered`/`ended` |
| `AiSessionStreamClose` | New enum |
| `AiRunInterruptSettlement` | New enum; `interrupt_run` returns it instead of `()` |
| `AiRunFailure`, `AiRunRetryAdmission`, `AiRunRetryEvidence`, `classify_run_retry` | New |
| `AiRunDisposition`, `AiRunDispositionView`, `AiRunRetryRefusal` | New |
| `RetryAiRunInput`, `AcknowledgeAiRunFailureInput` | New GraphQL inputs |
| `AiRunDispositionService`, `OrmAiRunDispositionService`, `AiRunDispositionLimits` | New |
| `OrmAiSessionService::session_stream_head` | New |
| `OrmAiSubscriptionService::with_reauthorization_grace`, `with_replay_check_interval` | New |
| `AiRunCompletion::outcome_code` | Now readable |
