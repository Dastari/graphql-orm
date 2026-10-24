---
title: "Host-bound principal projection"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-10-24
review_by: 2027-02-01
supersedes: []
---

# Host-bound principal projection

`AuthPrincipal::reference()` never carries an audience for a user session. A
host whose `CurrentPrincipalResolver` requires its verified audience on every
durable reference, and refuses references without it, therefore cannot resolve
the engine's default projection. `AiPrincipalProjection` is the opt-in seam
that lets such a host keep its strict resolver.

## Contract

- `AiPrincipalProjector::namespace(&AuthPrincipal)` is a host function returning
  an `AiPrincipalNamespace` (1–256 bytes, no control characters or padding). It
  is pure: no I/O and no request-global or mutable state. Hosts with one
  audience use `AiPrincipalProjection::fixed`.
- `AiPrincipalProjection::project` returns `principal.reference()` with only
  `audience` set. A user-session reference gains the namespace. An API-token
  reference keeps its own audience, which must already equal the namespace. Any
  other existing audience is refused. Identity, grant kind, session version,
  tenant/resource/actor/operation/correlation bindings and the exact
  `expires_at` cannot change, because the projector returns only a namespace.
- A projector error or contradiction is `AiError::ReauthorizationFailed`, with
  no host detail.
- The engine proves only that nothing but the namespace changed. It cannot
  prove the host verified that namespace. The host resolver stays the authority
  and still fails closed on mismatch, expiry, revocation and store errors.

## Installation

Install one shared `Arc<AiPrincipalProjection>` on every admitting service:

| Service | Effect |
| --- | --- |
| `OrmAiSessionService::with_principal_projection` | New runs and first title work persist the projected reference. Session-event subscriptions built on this service inherit it. |
| `OrmAiRunDispositionService::with_principal_projection` | Retry and acknowledgement resolve it; an explicit retry persists it. |
| `OrmAiRunCancellationService::with_principal_projection` | Cancellation resolves it. |
| `OrmAiToolCallResultPreviewService::with_principal_projection` | Preview resolves it before disclosure. |
| `OrmAiInboxService::with_principal_projection` | Inbox streams resolve it. |
| `OrmAiProposalService` / `OrmAiApprovalService` `::with_principal_projection` | Proposal and approval access resolves it. |

With a projection installed, session-event and inbox streams strictly resolve
the projected reference before the first page and withhold every page if that
fails. Periodic reauthorization then resolves the same reference.

`OrmAiSessionService::with_submission_admission(resolver, clock)` is a separate
opt-in. It strictly resolves each new submission's final reference before any
write and requires the exact reference back while it is still live. Under a
strict resolver this refuses a missing projection as well as a wrong namespace,
and the refused send stores no message, run, title work or event.

Without these methods every service behaves exactly as before.

## Composition with run authorization

Projection happens first. `AiRunAuthorizationIssuer` is unchanged and may return
either the unprojected or the projected identity; the engine stores the
projected reference with the issued deadline. Projection alone keeps the
original deadline and needs no issuer. Neither renews existing work.

## Durable work and existing records

Leases, title work, budgets, approvals, checkpoints, provider bindings and
tool waiters copy the persisted reference and keep exact equality and
fingerprints. Automatic retry, recovery and replay use that reference. They
never re-project it.

Records written without a projection keep an audience-less reference. They are
never stamped, upgraded or inferred. Under a strict resolver they fail closed at
their next resolution; an explicit retry by a currently authenticated caller
is a new admission, not a repair. The same holds after restore.

Ordinary reads (session, message and list queries) still authorize the
authenticated caller directly through access policy and owner identity, and
build no reference.

## Limits

- Projection does not partition ownership. Sessions remain owned by principal
  kind and subject. Several namespaces sharing one AI store is unsupported:
  each host's workers would fail the other host's runs closed.
- `agql-auth` sensitive revalidation (`token_status`) re-projects
  `accepted.reference()` itself. A host using it with stamped ordinary user
  references must resolve through `ResolvedPrincipal::from_current_session`,
  otherwise the comparison of `principal().reference()` with the stamped
  reference fails closed. No AI path uses that revalidation.
