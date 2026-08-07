---
title: GraphQL ORM Router technical design
kind: reference
status: draft
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-07
supersedes: []
---

# GraphQL ORM Router — Technical Design

## Status

Proposed.

## 1. Overview

`graphql-orm-router` will be a reusable Rust GraphQL federation router contained within the `graphql-orm` monorepo.

It will provide:

- federation runtime;
- schema composition;
- subgraph registry;
- automatic schema refresh;
- HTTP routing;
- WebSocket subscription routing;
- JWT authentication;
- declared scope enforcement;
- integration with `graphql-orm`;
- optional integration with `agql-auth`.

The initial production consumer will be GEMA.

The intended GEMA migration removes:

- Cosmo Router;
- WGC;
- Cosmo execution configuration;
- custom Cosmo authorization modules;
- NATS;
- JetStream;
- EDFS;
- NATS-backed GraphQL subscription generation.

## 2. High-Level Architecture

```text
                        GraphQL Clients
                    HTTP + WebSocket /graphql
                              │
                              ▼
                 ┌────────────────────────┐
                 │  graphql-orm-router    │
                 │                        │
                 │  Federation runtime    │
                 │  Subgraph registry     │
                 │  Schema composition    │
                 │  JWT validation        │
                 │  Scope enforcement     │
                 │  Graph lifecycle       │
                 └───────────┬────────────┘
                             │
                   HTTP + WebSocket
                             │
       ┌─────────────────────┼─────────────────────┐
       ▼                     ▼                     ▼
  subgraph-a            subgraph-b            subgraph-c
  async-graphql         async-graphql         async-graphql
  graphql-orm           graphql-orm           custom GraphQL
  agql-auth             agql-auth             compatible auth
  Tokio broadcast       Tokio broadcast       own subscription
```

## 3. Workspace Architecture

Proposed workspace:

```text
graphql-orm/
├── crates/
│   ├── graphql-orm
│   ├── graphql-orm-macros
│   ├── graphql-orm-storage
│   ├── graphql-orm-backup
│   ├── graphql-orm-ai
│   ├── graphql-orm-router-protocol
│   └── graphql-orm-router
└── docs/
    └── plans/
        └── active/
            └── graphql-orm-router/
```

Dependency direction:

```text
                   agql-auth
                       ▲
                       │ optional integration
                       │
graphql-orm ──► graphql-orm-router-protocol ◄── graphql-orm-router
                                                 │
                                                 ▼
                                        Federation runtime
```

Rules:

- `graphql-orm` must not depend on `graphql-orm-router`.
- `agql-auth` must not depend on `graphql-orm-router`.
- protocol types must remain independent of the federation runtime.
- the router may optionally depend on `agql-auth`.
- `graphql-orm` may optionally emit protocol-compatible metadata.
- the router must not expose Hive JWT or object-storage configuration;
- a future deployment-owned storage integration uses `graphql-orm-storage`
  outside the project-neutral router crate; and
- router authentication is public-key resource-server validation only. Private
  keys, token signing/issuance, refresh sessions, and RSA decryption remain in
  an external identity service.

## 4. Crate Responsibilities

### 4.1 graphql-orm-router-protocol

Purpose:

Provide a stable interoperability contract between GraphQL services and compatible routers.

Protocol v1 data model:

```rust
pub struct SubgraphDescriptor {
    pub protocol_version: ProtocolVersion,
    pub subgraph: SubgraphIdentity,
    pub graphql: GraphqlEndpoints,
    pub schema: SchemaAdvertisement,
    pub capabilities: CapabilitySet,
    pub required_semantics: Vec<String>,
    pub operations: Vec<OperationDescriptor>,
    pub fingerprints: DescriptorFingerprints,
}

pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

pub struct OperationDescriptor {
    pub root_type: RootOperationType,
    pub field_name: String,
    pub arguments: Vec<ArgumentDescriptor>,
    pub authorization: AuthorizationRequirement,
}

pub enum AuthorizationRequirement {
    Public,
    Authenticated,
    AllScopes { scopes: Vec<ScopeTemplate> },
    AnyScopes { alternatives: Vec<ScopeSet> },
    SubgraphOnly { policy: UnrepresentablePolicy },
}

pub struct ScopeTemplate {
    pub value: String,
}
```

The protocol crate must contain serializable project-neutral declarations only.
Compatible readers accept later minors in the same major and ignore additive
fields. A producer lists semantics that cannot be ignored in
`required_semantics`; an unknown required semantic or different major fails
with a stable error category.

### 4.2 graphql-orm-router

The router crate will contain modules conceptually similar to:

```text
src/
├── lib.rs
├── config.rs
├── runtime.rs
├── registry.rs
├── discovery.rs
├── composition.rs
├── graph_state.rs
├── auth/
│   ├── mod.rs
│   ├── jwt.rs
│   ├── scopes.rs
│   └── templates.rs
├── subgraphs/
│   ├── client.rs
│   └── health.rs
├── telemetry.rs
└── bin/
    └── graphql-orm-router.rs
```

The crate should expose both:

- a reusable library API;
- a standalone executable.

## 5. Federation Runtime

The project will not implement a Federation query planner from scratch.

An existing Rust Federation engine will be embedded or wrapped.

The initial intended implementation is Hive Router or reusable Hive Router components.

Responsibility split:

```text
graphql-orm-router
        │
        ├── registry
        ├── schema lifecycle
        ├── composition
        ├── authorization integration
        ├── operational configuration
        └── runtime lifecycle
                 │
                 ▼
          Federation engine
                 │
                 ├── operation parsing
                 ├── query planning
                 ├── entity resolution
                 ├── subgraph execution
                 └── subscription execution
```

The chosen federation runtime should not leak unnecessary implementation-specific types through the public `graphql-orm-router` API.

## 6. Public Router Interface

Default endpoints:

```text
POST /graphql
GET/WS /graphql

GET /health/live
GET /health/ready
```

Administrative endpoints may use an internal namespace:

```text
GET  /_router/status
POST /_router/subgraphs
POST /_router/refresh
```

Administrative endpoints must require authentication when enabled outside loopback-only development environments.

## 7. Active Graph Model

Graph state shall be immutable once created.

Indicative model:

```rust
pub struct ActiveGraph {
    pub version: GraphVersion,
    pub fingerprint: String,
    pub activated_at: SystemTime,
    pub supergraph_sdl: Arc<str>,
    pub subgraphs: BTreeMap<SubgraphName, ActiveSubgraph>,
}

pub struct ActiveSubgraph {
    pub descriptor: SubgraphDescriptor,
    pub schema_sdl: Arc<str>,
    pub schema_fingerprint: String,
    pub authorization_fingerprint: Option<String>,
}
```

Graph activation must be atomic.

Readers must observe either:

- the complete previous graph;
- or the complete replacement graph.

They must never observe partially updated graph state.

An immutable graph stored behind `ArcSwap`, `RwLock<Arc<_>>`, or an equivalent atomic swap mechanism is appropriate.

## 8. Subgraph Lifecycle

State model:

```text
REGISTERED
    │
    ▼
CANDIDATE
    │
    ├── SDL fetch failure ──────► REJECTED
    │
    ├── metadata invalid ───────► REJECTED
    │
    ├── composition failure ────► REJECTED
    │
    └── success
            │
            ▼
          ACTIVE
            │
            ├── health failure
            │       │
            │       └── ACTIVE + UNHEALTHY
            │
            ├── invalid schema update
            │       │
            │       └── retain previous ACTIVE schema
            │
            └── valid schema update
                    │
                    └── activate new graph version
```

A health failure does not implicitly alter schema composition.

Explicit administrative removal is required to remove an active subgraph from the graph.

## 9. Schema Discovery

Each participating subgraph should provide an authenticated internal schema endpoint.

Recommended baseline:

```text
GET /sdl
```

Example response:

```text
Content-Type: application/graphql
ETag: "<schema-fingerprint>"
```

The endpoint may require an internal service credential.

Polling process:

```text
refresh timer
      │
      ▼
GET /sdl If-None-Match
      │
      ├── 304 Not Modified
      │       └── no composition
      │
      └── changed
              │
              ▼
           fetch SDL
              │
              ▼
       build candidate graph
```

A push-based schema-change notification may be added later as an optimisation.

Polling remains the baseline because it:

- is simple;
- self-recovers after temporary network failure;
- does not require another messaging system;
- naturally detects restarted services.

## 10. Schema Fingerprints

Subgraphs should expose deterministic schema fingerprints.

The fingerprint must change when the router-relevant schema changes.

It should cover:

- GraphQL SDL;
- federation directives;
- operation authorization declarations;
- router protocol metadata that affects execution.

The fingerprint must not include unstable values such as:

- timestamps;
- hostnames;
- process IDs;
- memory addresses.

SHA-256 over a canonical schema representation is suitable.

## 11. Composition Pipeline

Composition shall operate entirely on immutable candidate input.

```text
changed candidate SDL
         │
         ▼
load last-known-good SDLs
         │
         ▼
validate protocol metadata
         │
         ▼
run Federation composition
         │
         ▼
validate resulting supergraph
         │
         ▼
construct runtime graph
         │
         ▼
atomic activation
```

No candidate artifact may overwrite active runtime state before complete success.

The active graph should record:

- all input subgraph fingerprints;
- router protocol versions;
- composition library version;
- resulting supergraph fingerprint;
- activation timestamp.

## 12. Composition Implementation

Composition should use a native Rust Federation composition implementation where practical.

Potential implementation:

```text
Subgraph SDLs
      │
      ▼
graphql-composition
      │
      ▼
FederatedGraph
      │
      ▼
Supergraph SDL
      │
      ▼
Federation runtime
```

The router must not require:

- Node.js;
- WGC;
- shell execution;
- intermediate Cosmo execution configuration.

## 13. graphql-orm Integration

### 13.1 Existing Generated Operations

`graphql-orm` already generates:

- query roots;
- mutation roots;
- subscription roots;
- operation metadata;
- deterministic generated-surface metadata.

Router integration should extend this existing metadata system rather than introduce a parallel generator.

### 13.2 Canonical Authorization Metadata

Each generated operation should have a canonical authorization descriptor.

Conceptually:

```rust
AuthorizationRequirement::Scopes(
    ScopeRequirement {
        alternatives: vec![
            vec![
                ScopeTemplate::new("endpoint.{Id}.read")
            ],
            vec![
                ScopeTemplate::new("endpoints.read")
            ],
            vec![
                ScopeTemplate::new("global.admin")
            ],
        ],
    }
)
```

The same metadata shall drive:

1. subgraph resolver enforcement;
2. router protocol metadata;
3. Federation authorization directives where the policy can be represented by standard directives.

The derive grammar uses repeatable, disjoint category declarations across every
generated category: `list`, `single_read`, `search`, `keyset_list`, `create`,
`upsert`, `update`, `update_many`, `delete`, `delete_many`, and `subscription`.

```rust
#[graphql_orm(
    operation_authorization(
        categories = ["single_read"],
        any_scopes = [["records.read"], ["records.admin"]]
    ),
    operation_authorization(
        categories = ["list"],
        all_scopes = ["records.list", "tenant.active"]
    ),
    operation_authorization(
        categories = ["search"],
        any_scopes = [["records.search"], ["records.admin"]]
    ),
    operation_authorization(
        categories = ["keyset_list"],
        all_scopes = ["records.page", "tenant.active"]
    ),
    operation_authorization(
        categories = ["create", "upsert", "update", "update_many", "delete_many"],
        all_scopes = ["records.write"]
    ),
    operation_authorization(
        categories = ["delete"],
        any_scope_templates = [["records.{id}.delete"], ["records.admin"]]
    ),
    operation_authorization(
        categories = ["subscription"],
        any_scopes = [["records.events"], ["records.admin"]]
    )
)]
```

Each category may appear in only one declaration. Exactly one of `all_scopes`,
`any_scopes`, `all_scope_templates`, or `any_scope_templates` is required and
empty sets are invalid. Fixed scopes reject whitespace, control characters,
and template braces. Template modes validate balanced GraphQL argument names,
statically reject unknown arguments and complex or nullable inputs, and support
only canonical String, UUID, Boolean, integer, and float substitutions.
Declarations also fail when the entity does not generate the named operation,
preventing an unused policy from silently appearing valid.

### 13.3 Fixed Scope Policies

Fixed scope requirements should use standard Federation directives where supported.

The pinned async-graphql 7.2.1 exporter natively records and imports
`@requiresScopes`, but it does not expose or import Federation's standard
`@authenticated` field metadata. Generated operations therefore address the
non-imported standard directive through the existing Federation link's default
namespace as `@federation__authenticated`. A compatible directive definition is
registered with async-graphql only so its schema registry accepts and emits the
field invocation; it is not linked or composed as a project-owned directive.
Both the pinned Hive composition path and Apollo Composition recognize the
namespaced invocation as standard Federation `@authenticated` metadata.

The pinned `graphql-composition` renderer preserves the canonical
`@authenticated` and `@requiresScopes` invocations but omits the corresponding
supergraph SECURITY feature links used by Hive to activate authorization
metadata. The private router composition adapter adds only the required
`authenticated/v0.1` and `requiresScopes/v0.1` links through the parsed
supergraph AST before constructing Hive's immutable candidate. This remains a
structural compatibility adapter, not SDL string rewriting. Regression tests
prove the namespaced subgraph form composes, both SECURITY links are present,
and Hive extracts both authorization rules.

The same async-graphql release does not expose its dedicated `requires_scopes`
attribute on subscription fields. Generated subscriptions therefore use the
standard directive through the existing Federation link's default namespace as
`@federation__requiresScopes`; composition normalizes it to the same
`@requiresScopes` identity. This is the subscription analogue of the
`@authenticated` compatibility path, not a project-owned directive.

Example:

```graphql
type Query {
    Records: [Record!]!
        @authenticated
        @requiresScopes(
            scopes: [
                ["records.read"]
                ["global.admin"]
            ]
        )
}
```

### 13.4 Parameterised Scope Policies

Standard Federation scope directives may not be sufficient for argument-dependent scopes.

The router protocol must therefore support templates such as:

```text
endpoint.{Id}.read
```

A project-neutral custom directive may optionally expose the same information in SDL:

```graphql
directive @routerRequiresScopes(
    scopes: [[String!]!]!
) on FIELD_DEFINITION
```

Example:

```graphql
type Subscription {
    EndpointChanged(Id: String!): EndpointChangedEvent!
        @routerRequiresScopes(
            scopes: [
                ["endpoint.{Id}.read"]
                ["endpoints.read"]
                ["global.admin"]
            ]
        )
}
```

The protocol metadata remains authoritative for router-specific semantics if custom directives create composition compatibility problems.

Generated argument templates use protocol metadata only. Their authoritative
subgraph guard performs one-pass substitution after GraphQL coercion, so braces
inside argument data are never reinterpreted as placeholders. Authorization
fingerprint version 2 binds each referenced argument's GraphQL type and
requiredness while the existing discovery fingerprint remains unchanged.

## 14. Scope Template Evaluation

Templates shall reference GraphQL root-field arguments by name.

Example template:

```text
endpoint.{Id}.read
```

Client operation:

```graphql
subscription EndpointChanged($endpoint: String!) {
    EndpointChanged(Id: $endpoint) {
        Id
    }
}
```

Variables:

```json
{
  "endpoint": "endpoint-123"
}
```

Resolved requirement:

```text
endpoint.endpoint-123.read
```

Rules:

- variables must be resolved before evaluation;
- a referenced missing argument causes denial;
- an invalid or null required argument causes denial;
- unsupported complex values cause denial;
- substitutions must use deterministic canonical string conversion;
- unresolved templates fail closed;
- template values are data and must never become executable expressions.

## 15. agql-auth Integration

### 15.1 JWT Scope Claim

Current applications may use:

```json
{
  "scopes": [
    "records.read",
    "records.write"
  ]
}
```

The router ecosystem should align new tokens with the conventional claim:

```json
{
  "scope": "records.read records.write"
}
```

OAuth represents `scope` as a space-delimited JSON string. The legacy
project-specific `scopes` claim remains an array during the bounded migration.

Migration strategy:

```text
token issuer:
    emit "scope"

validators:
    accept "scope"
    optionally accept legacy "scopes"
```

If both claims appear and differ, validation must follow a clearly defined fail-closed rule rather than silently unioning them.

### 15.2 Shared Scope Matching

Router authorization and subgraph authorization must use equivalent matching rules.

Where exact matching is configured:

```text
orders.read == orders.read
```

Where hierarchical matching is configured, router and subgraph must use:

- the same matcher implementation;
- or tested compatibility vectors with identical results.

Preferred design:

`graphql-orm-router` optionally reuses `agql-auth` resource-server and scope-matcher primitives directly.

At the 0.14 pin, the adapter consumes the validator's verified normalized
principal and configured legacy-scope policy directly; it does not perform a
second unverified payload decode.

The optional adapter uses `AccessTokenValidator` with public key or JWKS
material. It must not expose or construct issuer-side `AuthService`, signing
configuration, private PEM input, refresh/session stores, or decryption APIs.
Hive's own JWT runtime remains unconfigured.

At the pinned revision, `AuthService` already loads a private PEM and signs
tokens, so an external identity service can keep that responsibility in
`agql-auth`. `AccessTokenValidator` already provides the public-key/JWKS RS256
resource-server seam needed by the router, including WebSocket
`connection_init` validation. `agql-auth` does not expose RSA private-key
decryption; the router has no reason to add such an operation.

### 15.3 HTTP Authentication

Flow:

```text
Client
  │ Authorization: Bearer <JWT>
  ▼
graphql-orm-router
  │
  ├── validate signature
  ├── validate issuer
  ├── validate audience
  ├── validate expiry
  ├── parse roles/scopes
  └── evaluate router policy
          │
          ▼
        subgraph
          │
          ├── validate JWT independently
          └── execute resolver guard
```

### 15.4 WebSocket Authentication

Client:

```text
connection_init
{
    "Authorization": "Bearer <JWT>"
}
```

Router:

```text
validate token
create authenticated WebSocket connection context
```

Subgraph connection:

```text
propagate approved Authorization credential
```

Subgraph:

```text
agql-auth connection-init validation
        │
        ▼
AuthUser/AuthRuntime
        │
        ▼
generated resolver guard
```

A long-lived WebSocket must not turn initial authentication into permanent authorization.

Expiry, revocation and assurance-aging policies must remain enforceable.

## 16. Router Authorization Pipeline

For each GraphQL operation:

```text
parse document
     │
     ▼
resolve operation
     │
     ▼
resolve variables
     │
     ▼
identify protected root field(s)
     │
     ▼
load authorization metadata
     │
     ▼
expand scope templates
     │
     ▼
evaluate authenticated principal
     │
     ├── deny ─► GraphQL authorization error
     │
     └── allow
             │
             ▼
       Federation execution
```

The subgraph repeats its own authoritative authorization when execution reaches its resolver.

## 17. Subscription Architecture

### 17.1 graphql-orm Generated Subscriptions

For supported write-capable backends, `graphql-orm` should use standard async-graphql subscriptions driven by local event broadcast.

```text
generated write
      │
      ▼
commit state change
      │
      ▼
generate change event
      │
      ▼
tokio::sync::broadcast::Sender
      │
      ▼
async-graphql Subscription resolver
```

Existing change event semantics should be reused rather than introducing a new router-specific event system.

### 17.2 Router Subscription Path

```text
Apollo/client
     │
     │ graphql-transport-ws
     ▼
graphql-orm-router
     │
     │ upstream graphql-transport-ws
     ▼
owning subgraph
     │
     ▼
async-graphql subscription
     │
     ▼
Tokio broadcast receiver
```

The router does not need direct access to the subgraph's Tokio sender.

The subgraph owns local event production.

The router sees an ordinary GraphQL subscription stream.

### 17.3 Event Semantics

Events are ephemeral.

If no client is subscribed when an update occurs:

```text
event is discarded
```

This is intended behaviour.

The authoritative state remains in the database or owning service.

### 17.4 Buffering

Broadcast buffers must be bounded.

A slow consumer may miss events.

Lagging should be:

- observable through metrics;
- handled without unbounded memory growth;
- treated as an invalidation loss rather than application-state corruption.

Clients should refetch authoritative state where necessary.

## 18. Generated Subscription Example

An entity:

```rust
#[derive(GraphQLEntity, GraphQLOperations)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub state: String,
}
```

could expose:

```graphql
type Subscription {
    EndpointChanged(Id: String!): EndpointChangedEvent!
}
```

A normal frontend subscription:

```graphql
subscription WatchEndpoint($id: String!) {
    EndpointChanged(Id: $id) {
        Action
        Endpoint {
            Id
            Name
            State
        }
    }
}
```

When the generated mutation updates the row:

```text
database update
      ↓
EndpointChangedEvent
      ↓
Tokio broadcast
      ↓
GraphQL subscription
      ↓
router
      ↓
Apollo
```

No message broker is involved.

## 19. Horizontal Scaling

Process-local broadcast does not cross service instances.

Example:

```text
                   Router
                 /        \
          FAME instance 1  FAME instance 2
                │               │
             broadcast A     broadcast B
```

If instance 1 processes a write while the active subscription is attached to instance 2, instance 2 will not receive that local event.

This is acceptable for the initial single-active-instance target.

If horizontal scaling becomes required, a pluggable ephemeral event adapter may be introduced.

Potential providers:

```text
Tokio local broadcast
NATS Core
Redis Pub/Sub
PostgreSQL LISTEN/NOTIFY
```

The adapter should expose semantics similar to:

```rust
pub trait ChangeEventBus {
    async fn publish(&self, event: ChangeEvent) -> Result<()>;
    async fn subscribe(&self, topic: &str) -> Result<EventStream>;
}
```

Durable history and replay are explicitly not required by this abstraction.

## 20. Subgraph Protocol Endpoint

A preferred generic metadata endpoint may be introduced:

```text
GET /.well-known/graphql-router
```

Example:

```json
{
  "protocolVersion": { "major": 1, "minor": 0 },
  "subgraph": { "id": "fame-service", "name": "fame" },
  "graphql": {
    "http": "http://fame:8080/graphql",
    "websocket": "ws://fame:8080/graphql"
  },
  "schema": {
    "url": "http://fame:8080/sdl"
  },
  "capabilities": {
    "subscriptions": true,
    "authorizationMetadata": true,
    "schemaFingerprints": true
  },
  "operations": [],
  "fingerprints": {
    "schema": "sha256:...",
    "authorization": "sha256:...",
    "combined": "sha256:..."
  }
}
```

URLs returned by the service must be subject to deployment policy.

For static configurations, the router may override self-advertised URLs.

## 21. Dynamic Registration

An authenticated registration operation may resemble:

```http
POST /_router/subgraphs
```

Payload:

```json
{
  "name": "fame",
  "metadataUrl": "http://fame:8080/.well-known/graphql-router"
}
```

Flow:

```text
service registers
      │
      ▼
verify service credential
      │
      ▼
fetch descriptor
      │
      ▼
fetch SDL
      │
      ▼
validate
      │
      ▼
compose candidate graph
      │
      ├── failure
      │      └── mark REJECTED
      │
      └── success
             └── mark ACTIVE
```

Dynamic registration must not permit arbitrary untrusted URLs without SSRF controls.

## 22. SSRF and Network Trust

The router performs outbound requests to registered subgraphs.

Therefore:

- allowed schemes should default to `http` and `https`;
- registration should restrict destinations to configured service networks or allowlists;
- link-local and metadata-service addresses should be rejected unless explicitly configured;
- redirects should be bounded or disabled;
- credentials must not be forwarded to arbitrary hosts;
- a subgraph identity must be bound to its permitted destination.

## 23. Last-Known-Good Behaviour

Assume the active graph contains:

```text
fame schema hash A
ninja schema hash B
zorus schema hash C
```

FAME publishes hash D.

Composition fails.

The runtime remains:

```text
fame A
ninja B
zorus C
```

The router records:

```text
fame candidate D: rejected
```

It must not produce:

```text
ninja B
zorus C
```

with FAME silently missing.

## 24. Explicit Removal

Schema disappearance due to service failure is not removal.

A subgraph must be removed using an explicit administrative action or configuration change.

Removal flow:

```text
request removal
      │
      ▼
compose graph without subgraph
      │
      ├── invalid ─► reject removal
      │
      └── valid
              │
              ▼
        activate new graph
```

## 25. Configuration

Indicative configuration:

```yaml
server:
  listen: "0.0.0.0:4000"
  graphql_path: "/graphql"

composition:
  refresh_interval: "10s"
  retain_last_known_good: true

authentication:
  required: true
  jwks_url: "https://auth.example.com/.well-known/jwks.json"
  issuer: "example-auth"
  audience: "example-clients"

subgraphs:
  - name: "service-a"
    graphql_url: "http://service-a:8080/graphql"
    sdl_url: "http://service-a:8080/sdl"

security:
  max_request_body_size: "1MB"
  max_depth: 20
  max_fields: 500

telemetry:
  prometheus:
    enabled: true
```

Project-specific secret values should be supplied externally.

## 26. Standalone Binary

The crate shall expose:

```text
graphql-orm-router
```

Example:

```text
graphql-orm-router --config router.yaml
```

The binary should be sufficient for most projects.

## 27. Embeddable Library

Applications must also be able to construct the router programmatically.

Conceptual API:

```rust
let router = Router::builder()
    .config(config)
    .auth_provider(auth_provider)
    .build()
    .await?;

router.serve().await?;
```

This allows project-specific wrappers without modifying the generic router.

Example:

```text
gema-router
     │
     └── depends on graphql-orm-router
```

A GEMA wrapper should only be needed if GEMA requires behaviour that is genuinely outside the generic configuration model.

## 28. Observability

Structured tracing should include:

- graph version;
- operation name;
- operation type;
- selected subgraphs;
- execution duration;
- composition attempt ID;
- schema fingerprint;
- authorization decision category.

It must not include:

- raw JWTs;
- refresh tokens;
- API tokens;
- private keys;
- arbitrary GraphQL variable bodies by default.

Metrics should include:

```text
router_graphql_requests_total
router_graphql_errors_total
router_subgraph_requests_total
router_subgraph_latency_seconds
router_websocket_connections
router_active_subscriptions
router_subscription_lagged_total
router_schema_refresh_total
router_composition_success_total
router_composition_failure_total
router_authorization_denied_total
router_subgraph_health
```

## 29. Failure Handling

### Subgraph Unavailable

Return an appropriate downstream GraphQL error.

Do not mutate the graph schema.

### SDL Unavailable

Retain last-known-good SDL.

Record health failure.

### Candidate Composition Failure

Retain active graph.

Record the composition error.

### Invalid JWT

Reject before downstream execution.

### Missing Scope

Reject before protected downstream execution.

### Subgraph Guard Rejects Router-Accepted Request

Return the downstream authorization error.

This is expected defence-in-depth behaviour and should be observable as router/subgraph authorization disagreement.

### WebSocket Upstream Failure

Close or error affected subscriptions without affecting unrelated graph operations.

## 30. Testing Strategy

### 30.1 Protocol Tests

Test:

- serialization;
- version compatibility;
- deterministic fingerprints;
- unknown fields;
- incompatible major versions.

### 30.2 Composition Tests

Test:

- initial graph composition;
- compatible field addition;
- incompatible field change;
- subgraph addition;
- explicit removal;
- failed candidate;
- last-known-good retention.

### 30.3 HTTP Federation Tests

Test:

- single-subgraph query;
- multi-subgraph query;
- entity resolution;
- mutation routing;
- propagated authentication.

### 30.4 Authorization Tests

Test:

- no token;
- invalid token;
- expired token;
- correct scope;
- incorrect scope;
- any-of scopes;
- all-of scopes;
- hierarchical matcher where enabled;
- argument-templated scope;
- unresolved template;
- router/subgraph equivalence.

### 30.5 Subscription Tests

Test:

- WebSocket authentication;
- generated subscription establishment;
- generated write;
- live event receipt;
- filtered entity subscription;
- multiple subscribers;
- disconnected client;
- no replay;
- lagging consumer;
- token expiry;
- scope rejection.

### 30.6 Security Tests

Test:

- unauthorized registration;
- SSRF destination rejection;
- invalid SDL;
- oversized request;
- excessive query depth;
- excessive field count;
- token redaction in logs;
- administrative endpoint protection.

## 31. Required Changes to graphql-orm

Implementation work should include:

1. Extend existing operation metadata with router authorization descriptors.
2. Add deterministic authorization fingerprints.
3. Ensure generated subscriptions expose standard GraphQL subscription fields independently of NATS.
4. Confirm generated change events use process-local event senders suitable for Tokio streams.
5. Ensure subscription resolvers receive normal async-graphql request context.
6. Ensure generated guards execute for subscriptions.
7. Generate fixed-scope Federation authorization directives where appropriate.
8. Generate parameterised-scope router metadata.
9. Add optional `graphql-orm-router-protocol` integration.
10. Add router compatibility integration tests.

## 32. Required Changes to graphql-orm-macros

Implementation work should include:

1. Generate canonical operation authorization metadata.
2. Generate router protocol descriptors.
3. Validate scope template syntax.
4. Validate referenced root-field arguments where possible.
5. Include generated subscriptions in operation catalogues.
6. Produce deterministic metadata ordering.
7. Avoid generating router-specific runtime dependencies into applications that do not enable router integration.

## 33. agql-auth Integration and Upstream Changes

The exact-pinned revision already provides:

1. `AccessTokenValidator` for public-key/JWKS RS256 validation;
2. WebSocket `connection_init` authentication;
3. exact scope matching through its existing authorization runtime; and
4. issuer-side private-PEM loading and token signing through `AuthService`,
   which remains outside the router.

The separately authorized `agql-auth` 0.14 work now provides:

1. Emit standards-compatible space-delimited `scope` claims for new tokens.
2. Support legacy `scopes` during transition.
3. Define conflict behaviour when both claims are present.
4. Preserve fail-closed validation and exact matching by default.
5. Router interoperability tests covering HTTP and WebSocket validation.

The workspace pins that contract at
`413fda3435f060604cd653c11e2cc18a668aace1`. Purpose tokens retain their
separate `scopes` array; the router consumes access tokens only.

No router requirement calls for RSA private-key decryption. If a future
identity-service requirement introduces it, that is an issuer-side security
decision and not part of the router integration.

## 34. Required Changes to GEMA

The GEMA migration should eventually:

1. Add `graphql-orm-router` as the federation router.
2. Route frontend `/api/graphql` or equivalent to the new router.
3. Route frontend WebSocket GraphQL to the new router.
4. Migrate Cosmo-specific `WsAuthorization` behaviour to the standard connection payload if required.
5. Remove Cosmo Router configuration.
6. Remove `execution-config.json`.
7. Remove WGC invocation.
8. Remove Cosmo configuration rendering scripts.
9. Remove the custom Go subscription authorization module.
10. Replace EDFS/NATS-generated event subscriptions with native subgraph subscriptions.
11. Remove `edfs-kit` where no other functionality requires it.
12. Remove NATS server startup where no other workload requires it.
13. Remove NATS credential and ACL generation.
14. Remove JetStream health and monitoring logic.
15. Preserve existing scope semantics.
16. Preserve direct subgraph authorization.
17. Preserve existing Apollo client query, mutation and subscription behaviour.

## 35. Migration Strategy

Recommended staged implementation:

### Phase 1 — Protocol and Authorization Contract

Build:

```text
graphql-orm-router-protocol
graphql-orm metadata changes
agql-auth scope interoperability
```

No GEMA runtime changes yet.

### Phase 2 — Native graphql-orm Subscriptions

Ensure generated subscriptions operate entirely through local Tokio broadcast and async-graphql.

Prove:

```text
write → subscription event
```

without NATS.

### Phase 3 — Router Prototype

Implement:

- static subgraphs;
- composition;
- HTTP federation;
- WebSocket federation;
- JWT validation;
- router scope enforcement.

### Phase 4 — Schema Lifecycle

Add:

- automatic SDL polling;
- fingerprints;
- candidate graph composition;
- last-known-good activation;
- dynamic registration.

### Phase 5 — GEMA Parallel Runtime

Run:

```text
Cosmo/NATS production path

and

graphql-orm-router test path
```

against the same subgraphs where practical.

Compare:

- schema;
- query output;
- authorization;
- subscription behaviour.

### Phase 6 — Remove NATS GraphQL Notifications

Move GEMA subscriptions to native subgraph WebSocket subscriptions.

Remove EDFS dependency from those paths.

### Phase 7 — Replace Cosmo

Make `graphql-orm-router` the public GraphQL endpoint.

Retain rollback capability during initial deployment.

### Phase 8 — Cleanup

Remove obsolete:

- Cosmo runtime;
- WGC;
- NATS GraphQL infrastructure;
- JetStream GraphQL configuration;
- Go auth module;
- EDFS schema generation;
- obsolete deployment scripts.

## 36. Future Extensions

Potential future features include:

- optional Redis/NATS Core/Postgres subscription fan-out;
- multiple router instances sharing graph registry state;
- schema push notifications;
- persisted operations;
- configurable operation allowlists;
- router plugin API;
- distributed graph registry;
- UI/CLI graph inspection;
- schema compatibility checks in CI;
- graph history and rollback.

These should remain outside the minimum initial implementation.

## 37. Final Target

The intended reusable architecture is:

```text
                    Client
                      │
              HTTP / WebSocket
                      │
                      ▼
           ┌──────────────────────┐
           │ graphql-orm-router   │
           │                      │
           │ Federation engine    │
           │ Composition          │
           │ Registry             │
           │ JWT validation       │
           │ Scope enforcement    │
           └──────────┬───────────┘
                      │
          ┌───────────┼──────────────┐
          ▼           ▼              ▼
      Service A    Service B      Service C
      GraphQL      GraphQL        GraphQL
      │            │              │
      ├ ORM        ├ ORM          └ custom
      ├ agql-auth  ├ agql-auth
      └ broadcast  └ broadcast
```

For GEMA this becomes:

```text
Apollo
  │
  ▼
graphql-orm-router
  │
  ├── fame-service
  ├── ninja-service
  ├── huntress-service
  ├── zorus-service
  ├── cove-service
  ├── ninite-service
  └── other GraphQL services
```

with no Cosmo Router and no NATS/JetStream requirement for GraphQL subscriptions.

The router owns federation and graph lifecycle.

The subgraphs own their data, authentication enforcement and live change streams.

`graphql-orm` supplies generated GraphQL surfaces and canonical policy metadata.

`agql-auth` supplies interoperable authentication and scope semantics.
