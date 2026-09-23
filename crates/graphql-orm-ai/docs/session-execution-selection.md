---
title: "Immutable session execution selection"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-09-23
review_by: 2027-02-01
supersedes: []
---

# Immutable session execution selection

`AiSessionExecutionSelection` records an exact provider family, host routing
profile, model and explicit reasoning effort. It is routing metadata, never
provider egress, tool, budget, authentication or retained-thread authority.
Models remain bounded strings admitted from the host's reviewed discovery;
there is no hard-coded universal model list and no fallback to another model.

Install `AiSessionExecutionSelectionResolver` through
`OrmAiSessionService::with_execution_selection_resolver`. Both the generic
`AiMutationRoot` and direct host calls use this admission boundary. An explicit
request cannot be accepted without a resolver or silently replaced by its
response. An omitted request requires the resolver to select an explicit
reviewed default or reject it. `Unspecified` cannot be frozen as an effort.
`None` means the explicit no-reasoning value, not an unspecified default.

Creation commits the selected choice with the owner-scoped session. Sending a
message re-admits the exact stored choice outside the database transaction and
atomically copies it to the queued run. The transaction checks that the
observed selection did not change. A retry copies its source run's snapshot;
it cannot select a new deployment default. Host async admission and native
readiness never run while a state-machine transaction holds database locks.

A host can retain one global claim loop and concurrency semaphore, then call
`OrmAiRunExecutionSelectionReader::selection_for_run` after claiming each run.
The reader rehydrates the recorded principal, verifies current owner/scope
access and validates the exact active lease, generation, expiry and row
version. It returns the run snapshot, not current runtime settings. Hosts may
use `fingerprint()` as a bounded cache key for ordinary `AiRuntime` and
coordinator instances. Keep active run-to-instance ownership for cancellation,
cleanup and completion, and account concurrency across all cached instances.
The existing provider registry and execution authorization remain unchanged.

`AiSessionTitleWorkInput::execution_selection()` exposes the authorized session
choice to the host's title worker. A title worker must not use a global default
provider that changes the selected account/billing route. An absent or
unsupported title route can remain manually titled; this metadata does not
admit title inference on its own.

## Legacy sessions

Schema module `0.65.0` adds nullable, versioned library-owned selection snapshots
to session and run rows. Existing rows remain unbound. Their history stays
readable. A configured session service refuses new messages on an unbound
session with `AI_SESSION_EXECUTION_UNBOUND`; the run reader also refuses an
unbound queued run. No background migration assigns today's default.

The owner may explicitly call `pin_execution_selection` (GraphQL
`pinAiSessionExecutionSelection`). This is available only when the host's
resolver implements `legacy_descriptor`. The service supplies the exact
observed retained descriptor and owned session ID to that resolver outside the
transaction. The host verifies the selected route's registration against it;
the routing profile need not equal the retained provider's logical profile.
The returned descriptor must equal the observed descriptor. A historical
policy fingerprint is comparison evidence only: normal current rule, egress,
budget and retained-session checks still run before subsequent inference.

The pin transaction independently requires an unchanged owned active session,
unchanged nonexpired idle retained binding, exact descriptor, matching retained
transcript head, no queued/running/waiting/recovery-required run, and explicit
matching settled model/effort evidence in every budget reservation of the
binding's last run. Missing, ambiguous, unspecified, excessive or mismatching
evidence returns `AI_SESSION_EXECUTION_UNAVAILABLE`. Pinning commits a session
and inbox event without modifying or replaying provider state. Exact repeated
pins are idempotent; changing any already-bound choice conflicts. Stateless
legacy sessions and histories without sufficient evidence require a new chat.

An unconfigured service retains its previous unbound behavior for existing
consumers, but explicit selection requests are refused. Hosts adopting routed
execution must configure the resolver and use the lease-fenced reader on every
claim; the type alone cannot change an older host's global routing behavior.

## Reasoning effort

`ModelReasoningEffort::Ultra` serializes exactly as `ultra`. It is accepted only
through an exact model profile that explicitly lists it. Unsupported profiles
remain closed; no transport maps it to `max`, `xhigh` or a provider default.
Provider-specific readiness and wire negotiation remain authoritative.
