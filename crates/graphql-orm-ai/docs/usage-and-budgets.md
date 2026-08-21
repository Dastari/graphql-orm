---
title: "Usage Ledger, Budgets, and Reporting"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-01
supersedes: []
---

# Usage Ledger, Budgets, and Reporting

Provider execution has two related but distinct persistent contracts:

- budget reservations prevent concurrent calls from independently spending the
  same remaining capacity; and
- usage facts record only authoritative completed consumption for reporting.

Neither contract grants provider egress, resolver access, or permission to
read a transcript.

## Transactional lifecycle

Before transport, `OrmAiBudgetService` checks every applicable policy and the
current principal/session/run fence, then reserves all relevant counters in one
state-machine transaction. At the transport boundary the reservation becomes
uncertain. Capacity remains held until there is authoritative evidence.

`Commit` requires complete actual amounts and provider-reported cached input.
Cached input must be no greater than total input. The reconciliation moves the
actual amounts to committed counters, releases only proven unused capacity,
updates the reservation, and appends one `graphql_orm_ai_usage_entries` fact in
the same transaction. The usage fact has a unique reservation ID, so an exact
idempotent replay returns the prior result without duplicating usage.

`ReleaseUnused` is permitted only when transport provably did not occur and
appends no usage. `MarkUncertain` retains capacity and appends no usage; the
privileged recovery surface is described under
[Stranded reservations](#stranded-reservations-and-privileged-reclamation).

A denial at reservation happens strictly before the transport boundary. It is
therefore a certain, local refusal, not provider uncertainty: it consumes no
provider turn and leaves no reservation held. The coordinator closes such a run
as terminal `Failed` with outcome code `provider_budget_denied`, and the
run-failure record admits a retry once capacity exists. It never closes the run
as `RecoveryRequired`/`provider_turn_uncertain`, which would tell a user that a
proven refusal could not be confirmed and would permanently refuse retry.

## Stranded reservations and privileged reclamation

Reserved capacity counts against a policy ceiling exactly like committed usage.
A reservation that never reconciles therefore consumes the ceiling for the rest
of its policy period, and if enough of them accumulate the deployment starts
refusing every new provider call.

Two reservation states can strand:

- `uncertain`, when the worker died after the transport boundary; and
- `reserved`, when the worker died between the reservation transaction and the
  transport boundary.

Neither carries a durable proof that the provider was not reached, so neither
may be released. `expires_at` bounds how long a reservation could still belong
to a live provider call; it does not prove anything about transport.

`aiBudgetScopeCapacity` reports, for one exact scope under
`ReadBudgetPolicies`, each policy's current-period reserved and committed
amounts beside its ceilings, counts of unresolved reservations, and a bounded
oldest-first list carrying each reservation's state, expiry, owning-run
terminality, CAS version, and whether it meets the deployment and durable
time/run conditions to be a reclamation candidate. The mutation still rechecks
current authorization, recent MFA, exact CAS, scope, and stored-graph
integrity. Every count is a lower bound when `truncated` is set. Alarm on a
rising unresolved count rather than on the eventual refusal to serve.

`reclaimAiBudgetReservation` resolves one exact reservation. It requires
`ManageBudgetReclamation` for the exact scope, a current user principal with
recent MFA, the deployment-owned
`OrmAiConfigurationService::with_budget_reservation_reclamation` opt-in, an
exact CAS version, an expiry that has already passed by the deployment's
`minimum_expired_age`, and an owning run that reached a durable terminal state
holding no lease. It then commits the reservation's own reserved amounts as
authoritative usage, appends one usage fact and one redacted audit fact, and
CAS-updates the reservation to `committed`, all in one state-machine
transaction.

That resolution is conservative by construction: it charges the estimate that
was already being held, so it can only over-count. It also does not create
headroom. The reserved column falls by exactly the amount the committed column
rises, and `reserved + committed` against the ceiling is unchanged. What it
does is make held capacity accountable, reportable, and finite instead of
permanently unreachable. A deployment that needs headroom back raises or
replaces the policy through `upsertAiBudgetPolicy`; the crate will not
manufacture an absence proof to release capacity it cannot account for.

Reclamation is **not** performed automatically by expired-lease recovery or any
other maintenance pass. Automation would buy nothing operationally, because
committing frees no headroom, while it would add an unattended writer of
authoritative usage facts attributed to a principal who is not present. The
decision to charge an unprovable turn stays with an authorized, MFA-current,
audited human.

`input_tokens` is the provider-reported total. `cached_input_tokens` is a
validated subset, not an additional amount. Deployment pricing can calculate
cached and uncached rates from those two facts while preserving the provider's
authoritative total.

## Budget-policy management

Budget policies are managed only through `AiConfigurationQueryRoot` and
`AiConfigurationMutationRoot`; private generated CRUD is not an application
surface. The host authorizes `ReadBudgetPolicies`, `ManageBudgetPolicies`, and
`ManageBudgetReclamation` separately; a host that does not recognize an action
must return `false`. Mutations also require a current user principal with
recent MFA.

Before enabling mutations, the deployment must call
`OrmAiConfigurationService::with_budget_policy_management` with
`AiBudgetPolicyManagementLimits`. Those hard bounds cap every configurable
token, tool, image, cost, and run value and the number of policies per exact
scope. They are independent from, and should be no broader than, the provider
call and operational spend limits.

`upsertAiBudgetPolicy`/`UpsertAiBudgetPolicy` creates or exact-CAS updates a
policy and appends its redacted audit fact atomically. At least one
non-negative ceiling is required. Optional principal kind and subject must be
provided together. Scope, tenant, principal target, and interval are immutable
after creation; create a replacement and disable the old policy to change
them. There is no delete mutation.

A tenant-absent policy is an explicit wildcard for matching scope kind and ID.
Runtime lookup requests only the exact tenant scope and that wildcard scope,
then verifies the deterministic scope key and every stored scope field before
applying principal filters. The scope key is a bounded lookup aid, never
authorization.

## Immutable pricing catalog

Pricing configuration uses the same composable configuration GraphQL roots but
a separate `Arc<dyn AiPricingCatalogService>`. The host independently decides
`ReadPricingCatalog` and `ManagePricingCatalog` for the exact requested scope.
Creation additionally requires recent MFA and deployment-owned
`AiPricingCatalogManagementLimits`; the mutation and its redacted audit append
commit in one state-machine transaction.

Each row binds an exact scope, provider family, model, and globally unique
version reference. Fixed-call, per-million input/cached-input/output, and
per-call web/file-search rates are non-negative integer microunits. Cached
input cannot cost more than ordinary input. Rows are append-only: there is no
update, delete, activation, or implicit latest-version lookup. Rate changes
create a new reference, while existing reservations and uncertain work retain
the old one.

`OrmAiPricingService` implements three contracts:

- `AiPricingCatalogService` for authenticated bounded administration;
- `AiPricingQuoteService` for a conservative pre-transport estimate bound to
  exact scope/provider/model/version; and
- `AiProviderUsageAccounting` for authoritative post-transport cached and
  non-cached token plus completed web/file-search settlement under that same
  immutable version.

The quote treats every estimated input token as non-cached. Settlement uses
the provider-reported cached subset. Each integer-priced dimension rounds up
independently and all multiplication/addition is checked. For web/file search,
the quote binds distinct enabled `AiPricedBuiltinToolKind` values and the exact
shared provider tool-call ceiling, then reserves that many `tool_units` at the
greatest enabled rate. Settlement ignores advertised-but-unused tools and
charges only exact normalized start/completion pairs. The crate never embeds
or fetches current provider prices: administrators append exact deployment-
reviewed rates, and nonzero built-in rates remain disabled until the host sets
an independent `maximum_builtin_tool_microunits_per_call` management ceiling.

Every built-in-enabled `ModelRequest` requires
`maximum_builtin_tool_calls`; copy that same ceiling into the pricing quote and
reserve its returned amounts. The
opaque provider budget proof rechecks the reserved tool-unit ceiling before
transport, while
`AiProviderCallLimits::with_maximum_builtin_tool_calls` sets an independent
deployment ceiling. The existing `with_maximum_tool_calls` continues to bound
custom application-tool calls. Completed code-interpreter and image-generation
calls remain unsupported by the concrete accountant because their complete
authoritative billing dimensions are not modeled.

Hierarchical AI rules add a separate cumulative
`maximum_web_search_calls` ceiling. Before each provider turn the coordinator
projects the request's entire host-authored built-in call maximum as web
searches when search is offered; after transport it records only exact
normalized completed web-search pairs. `AiRuleRunUsage` is protected in the
coordinator checkpoint and carried into every continuation, so starting a new
provider call cannot reset the per-run search ceiling. This rule ceiling is in
addition to, and never a replacement for, the atomic pricing/budget
reservation, `maximum_builtin_tool_calls`, or WebSearch egress decision.

## Reporting authorization

Compose `AiQueryRoot` (or `AiUsageQueryRoot` separately) and install
`Arc<dyn AiUsageService>`. The ORM implementation also requires an
`AiUsageAccessPolicy` that returns one of:

- `OwnPrincipal`: exact principal kind and subject within the requested exact
  scope;
- `WholeScope`: every fact in the requested exact scope, only after the host
  independently authorizes that administrative view; or
- `Denied`.

This decision is evaluated from the current GraphQL `AuthPrincipal` for every
query. Stored principal/scope values are row partitions and audit dimensions,
not cached authorization. API-token principal kinds remain distinct from human
users.

The `aiUsage`/`AiUsage` field accepts an exact scope, optional provider/model
and creation-time filters, and Relay-style `first/after` or `last/before`
keysets. The default is 50 rows, the hard maximum is 200, and one time filter
requires both bounds and cannot exceed 366 days. Clients should retain only the
visible window.

The public view contains IDs, redacted scope/principal dimensions,
provider/model, token/unit amounts, settled cost, and commit time. It excludes
prompts, responses, transcript blocks, attachments, tool arguments/results,
pricing catalogs, budget ceilings/counters, credentials, and raw provider
payloads.

## GraphQL composition

```rust,no_run
use std::sync::Arc;

use async_graphql::{EmptySubscription, Schema};
use graphql_orm_ai::{
    AiMutationRoot, AiQueryRoot, AiUsageAccessPolicy, AiUsageService,
    OrmAiUsageService,
};

# fn example(
#     database: graphql_orm::db::Database<graphql_orm::graphql::orm::DefaultWriteBackend>,
#     policy: Arc<dyn AiUsageAccessPolicy>,
# ) {
let usage = Arc::new(OrmAiUsageService::new(database, policy));
let schema = Schema::build(AiQueryRoot, AiMutationRoot, EmptySubscription)
    .data(usage as Arc<dyn AiUsageService>)
    .finish();
# let _ = schema;
# }
```

The service's presence is not authorization. A missing service, missing
principal, denied policy, malformed scope, invalid filter, stale cursor, or ORM
failure closes the query.

## Migration, backup, and restore

The usage ledger was introduced by schema module `0.17.0`; current deployments
apply module `0.62.0`, which adds a scope/tenant/state/expiry index to
`graphql_orm_ai_budget_reservations` so stranded-reservation reporting is a
bounded indexed read. Module `0.54.0`'s append-only immutable pricing catalog
includes defaulted web/file-search per-call rates. Keep workers, configuration
writes, and readers closed during each managed migration. Unsupported legacy private
usage rows must be proven from
complete committed reservation evidence or removed; never fabricate a binding.
Existing committed reservations are not automatically treated as historical
usage.

Backups preserve usage facts as append-only records. Before reopening after a
restore, the database auditor must validate unique reservation linkage,
committed reservation state, matching scope/principal/provider/model,
non-negative amounts, and cached input not exceeding total input. Usage facts
and their reservation/counter graph remain `NotImplemented` and fatal.

Budget-policy and pricing-catalog database audits are implemented when the
host supplies `AiRestorePolicyAuditLimits` built from host-attested immutable
administration ceilings. Budget validation rederives exact scope identity,
principal pairing, interval, at least one nonnegative ceiling, per-scope
cardinality, and every supplied deployment maximum. Pricing validation rejects
noncanonical or duplicate references, scope-key or provider/model swaps,
negative or over-ceiling token/fixed/built-in rates, invalid cached-rate
ordering, route-cardinality overflow, and missing, duplicate, malformed, or
orphan immutable creation-audit linkage. Omitting deployment limits, reaching
a row bound, or observing any invalid graph is fatal rather than an assumed
zero. These inputs do not prove equivalence to live service configuration;
that exact configuration epoch must be bound by the future applied validator.
Reporting remains closed until the remaining audits and applied restore
reconciliation succeed.
