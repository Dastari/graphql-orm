---
title: "Private Remote GraphQL Execution"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-09-25
review_by: 2027-02-01
supersedes: []
---

# Private Remote GraphQL Execution

`graphql-orm-ai` can invoke reviewed application resolvers through a private
router or direct service without understanding a federation product, retaining
the user's bearer token, or accepting a model-selected URL. The crate supplies
the exact-binding adapter; the deployment owns credential issuance, network
routing, HTTP behavior, and application audit integration.

## Trust boundary

Register each destination as a `GraphqlExecutionTarget` with a stable logical
ID, `PrivateRouted` or `PrivateDirect` class, required audience/resource, and
reviewed schema fingerprint. The registry contains no URL or credential. The
tool's `GraphqlOperationContract` separately binds the exact server-authored
document, operation name, result projection, and static disclosure schema.
`AiRemoteGraphqlCapabilityBinding` identifies whether that validated request
came from an exact static descriptor or a registered generated capability. A
host cannot construct that identity independently of the authenticated bridge.

The ordinary `AuthenticatedToolBridge` remains responsible for current
principal rehydration and host tool-policy authorization. It passes the exact
validated request to `AiRemoteAuthenticatedGraphqlAdapter`, which:

1. rejects local targets and principals that are future-dated, stale, or
   expired;
2. rejects a document/hash/operation mismatch and recursive AI-control-plane or
   introspection operation;
3. constructs a redacted request binding the logical target, audience/resource,
   fresh subject and original actor, scope, schema/operation/document,
   canonical variables, projection/disclosure, run/tool IDs, correlation and
   causation IDs, safe delegation reference, hashed idempotency key, expiry,
   and the crate-authored registered capability identity;
4. requests one credential from `AiRemoteGraphqlAuthorityIssuer`;
5. verifies after asynchronous issuance that the returned authority asserts
   the same exact request and has not crossed its configured principal-
   freshness or authority-expiry boundary; and
6. rechecks expiry and all request bindings immediately before invoking
   `AiRemoteGraphqlTransport`.

`AiRemoteGraphqlAuthority` is intentionally non-serializable and non-cloneable,
stores its credential in `SecretString`, and redacts that credential from
`Debug`. It is an ephemeral transport value, not durable state or model input.

## Host implementation

Construct one adapter and pass clones of that same value as both
`GraphqlRequestContextFactory` and `AuthenticatedGraphqlExecutor`. Adapter
identity is part of the opaque context, so a context created by an unrelated
adapter instance is rejected.

Implement `AiRemoteGraphqlAuthorityIssuer` at the deployment credential
boundary. It receives the freshly resolved principal and complete redacted
delegation request, but no incoming bearer token. It must:

- inspect `request.capability_binding()` rather than an operation-name prefix;
- require the exact static descriptor ID/fingerprint and operation contract or
  generated capability ID/fingerprint and, for generated operations, the target, finished
  schema, semantic catalogue/operation and root-field binding;
- preserve the original human actor for on-behalf-of work;
- mint authority no broader than the requested audience, resource, scope, and
  exact operation;
- enforce the exclusive expiry and prevent reuse; and
- record only redacted identifiers or `stable_hash()` in issuer audit.

Implement `AiRemoteGraphqlTransport` at the isolated private egress boundary.
It must:

- map logical target IDs only through fixed deployment configuration;
- deny redirects, DNS/endpoint overrides, arbitrary headers, and
  model-controlled destinations;
- submit only the provided server-authored document and variables;
- propagate correlation, causation, actor/mechanism, and ordinary application
  audit metadata;
- apply bounded response size/time limits before returning; and
- return `ToolExecutionError::ResultBudgetExceeded` only when the response is
  refused for crossing that byte ceiling, without retaining or disclosing its
  content; and
- ensure `PrivateDirect` authorization is never broader than the equivalent
  `PrivateRouted` path.

The application resolver remains authoritative for roles, row/field policy,
tenant/resource boundaries, assurance, rate limits, and current object state.
Delegation and AI approval do not bypass those checks.

The adapter's `AiRemoteGraphqlOperationPolicy` defaults to `QueriesOnly`.
Explicit `RegisteredQueriesAndMutations` admits only exact registered mutation
contracts after current runtime authorization. Subscriptions, internal
operations, document/kind substitution and a generated-looking static operation
name still fail before authority issuance. Mutation contexts are single-use
and are consumed before transport; an ambiguous failure never permits reuse.
Initial, provider-retained and stateless mixed-read plans retain the same exact
registered IDs and fingerprints; hosts do not reconstruct a tool-result route
or add a continuation-side binding.

The compatible default `AiRemoteGraphqlAuthorityIssuer::issue_for_request`
forwards to `issue`. Hosts can override it to inspect the actual validated
`ToolGraphqlRequest` variables after the adapter has verified its exact static
contract. `AiToolExecutionProvenance`, when present, comes from the durable
tool lifecycle and binds the run, attempt, callback, argument hash, provider,
budget and optional consumed approval. Current policy version and authorization
state are refreshed at invocation. A direct bridge call need not have durable
provenance; a host that requires this proof must deny its absence.

A federation router may rewrite documents and variables. This adapter does not
prove that custom delegation claims survive or are enforced by that router.
Where exact target arguments must be bound, the host must derive an authenticated
logical argument binding from the registered mapping and actual request, then
verify the resolver's resolved arguments. Model metadata is never its source.
Neither the opaque provenance nor a delegation credential replaces current
target resolver authorization.

## What the adapter does not prove

The crate cannot introspect every proprietary delegated-token format or prove a
deployment's network isolation, DNS policy, router configuration, TLS trust,
or direct-service middleware parity. `AiRemoteGraphqlAuthority::for_request`
asserts that the trusted issuer matched its credential to the supplied request;
it does not independently validate the secret's claims. Treat issuer and
transport implementations as security-critical deployment code and verify them
with product-specific conformance tests outside this project-agnostic crate.

`AiRemoteGraphqlDelegationRequest` JSON now contains the required
`capability_binding` object and its stable hash therefore changes. Issuer and
transport services must deploy the updated request contract together; the
credential is short-lived and no durable row is migrated. No GraphQL SDL or
persistent schema is introduced by this adapter.
