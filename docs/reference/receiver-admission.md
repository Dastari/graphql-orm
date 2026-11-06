---
title: Exact receiver-purpose admission
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-06
review_by: 2027-02-01
supersedes: []
---

# Exact receiver-purpose admission

ORM `receiver-admission` enables `auth-agql` and provides opt-in HTTP admission before request context construction.
The catalogue owns `ReceiverPurposeV1`, `ReceiverPurposeMetadata`,
`RECEIVER_PURPOSE_CLAIM`, `canonical_json_hash` and `exact_document_hash`; profiles
re-exports them for issuers. Existing exact-operation claims and private-remote hashes
retain their byte format. Generated SDL and discovery/authorization fingerprints
are unchanged. ORM/macros 0.93.1, catalogue 0.7.1, profiles 0.21.1 and AI 0.122.1
are compatible patch releases. There is no persistent change or data migration.

Register one reviewed exact named query/root per operation name in an immutable
`AgqlReceiverRegistry`. Contracts bind receiver, target, schema, capability,
projection, disclosure and resource identifiers. These are host-owned opaque
identifiers; the host must resolve/review them against its actual schema and
policy. Schema revision changes require updating the contract. A registry belongs
to one receiver/schema deployment; never select a registry from client claims.

```rust,ignore
let contract = AgqlReceiverContract::new("ReadA", DOCUMENT, "item", metadata.clone())?;
let registry = AgqlReceiverRegistry::new(vec![contract], 16_384)?;
// Issuer: use registered variable definitions to compute coerced root arguments.
let purpose = ReceiverPurposeV1::bind(metadata, "ReadA", DOCUMENT, &variables, &arguments);
let prepared = prepared.with_claim(RECEIVER_PURPOSE_CLAIM, serde_json::to_value(purpose)?);
// Issue and authenticate through agql-auth, including issuer/audience/status/proof checks.
let admitted = registry.admit(&raw_body, AgqlReceiverTransport::Http,
    accepted_principal, &parent_sessions, &current_policy, &clock).await?;
let scopes = admitted.scopes(); // Only exposed after successful admission.
let request = admitted.into_request()?;
let response = schema.execute(request).await;
```

`AgqlReceiverPolicy` has no default implementation. Use authoritative current
scope, tenant, organization and resource policy. The callback receives raw
variables and derived root arguments: check tenant/resource selectors against
current authority, rather than treating signed intent as current authorization.
Parent liveness uses
`VerifiedActiveUserSessionResolver`, independently of MFA/assurance; the adapter
must refuse revoked/expired sessions without extending idle lifetime. Admission
checks identity, session, family, version, tenant, organization, parent grant,
role/scope subsets and own/parent deadlines. It re-reads the parent after the policy await, refuses changed role/scope
snapshots, and checks both parent deadlines again. The parent snapshot must be no more than 30 seconds old. No grants expand. Delegated `authorization_roles` references are unsupported;
issue explicit narrowed roles/scopes for version 1.

The host must authenticate the credential before admission, bound streaming reads
before buffering, keep identity out of shared schema data, and dispatch only the
sealed result. Existing generic identity projection APIs remain available for
other routes; hosts must route delegated receiver traffic through admission.
Admission is a point-in-time proof, not an atomic revocation fence. Continue to
apply business, row, field, sender-proof and current-authority checks as required.
Do not infer assurance from parent liveness or from this proof.

Version 1 accepts only exact UTF-8 registered named queries with one root and plain
field selections. It refuses aliases, fragments, directives, duplicate fields,
nested arguments, multiple operations/roots, mutations, subscriptions, batches,
persisted documents, extensions, unknown envelope/variable keys, duplicate JSON
keys at any depth, malformed bodies and unsupported claim versions/fields.
WebSocket is explicitly and always refused. Replay of the exact admitted HTTP
request during its valid lifetime is allowed; this is not one-shot authorization.

Variables support builtin String, ID, Boolean, Int and Float types and lists.
Custom scalars, enums and input objects require a later version and fail closed.
Reviewed variable declarations must agree with schema argument types. For omitted optional
variables, the schema argument must have no implicit default: express any intended
default in the registered variable declaration.
Schema-only argument defaults are outside version 1; this API does not introspect
a schema. Root arguments must be direct typed variables; literals and nested variable
expressions require schema-specific coercion and are refused. The receiver derives the
root argument object from the registered declarations and raw variables, then
compares its hash with the signed argument hash. Omitted variables, explicit null,
and explicit defaults bind differently even when coercion produces equal arguments;
array order is significant. JSON objects sort recursively, numbers retain
serde_json's encoding (this is not RFC 8785). SHA-256 is unprefixed lowercase hex.
Secret-derived hash comparisons use constant-time equality. All public admission
failures return `AgqlReceiverRefused` / `RECEIVER_ADMISSION_REFUSED` without a reason.

The SQLite signed-token fixture runs positive execution before refusal cases and
tracks dispatch, resolver and SQL-read counters. Admission emits no database code;
PostgreSQL/MSSQL behavioral lanes are unnecessary for this boundary. No new
transport, router forwarding proof, refresh token, upstream auth change or release
automation is included.
