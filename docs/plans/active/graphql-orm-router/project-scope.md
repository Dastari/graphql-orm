---
title: GraphQL ORM Router project scope
kind: reference
status: draft
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-07
supersedes: []
---

# GraphQL ORM Router — Project Scope

## Status

Proposed.

## Project Name

`graphql-orm-router`

## Repository

The project will live within the existing `graphql-orm` Rust workspace as independently consumable crates.

Proposed workspace layout:

```text
graphql-orm/
├── crates/
│   ├── graphql-orm
│   ├── graphql-orm-macros
│   ├── graphql-orm-storage
│   ├── graphql-orm-backup
│   ├── graphql-orm-ai
│   ├── graphql-orm-router
│   └── graphql-orm-router-protocol
└── docs/
    └── plans/
        └── active/
            └── graphql-orm-router/
                ├── README.md
                ├── project-scope.md
                ├── functional-requirements.md
                └── technical-design.md
```

## 1. Purpose

The purpose of `graphql-orm-router` is to provide a project-agnostic GraphQL federation router for Rust applications, with first-class integration with:

- `graphql-orm`
- `async-graphql`
- `agql-auth`

The router will provide a single public GraphQL HTTP and WebSocket endpoint over multiple independently deployed GraphQL microservices.

The project is intended to remove the requirement for application projects to depend on external GraphQL routing and event infrastructure solely to provide:

- federated GraphQL queries;
- federated GraphQL mutations;
- GraphQL subscriptions;
- live change notifications;
- schema composition;
- schema discovery;
- scope-based authorization.

The initial motivating project is GEMA, where the target architecture replaces both WunderGraph Cosmo Router and NATS/JetStream.

The router itself must not contain GEMA-specific business logic.

## 2. Objectives

The project will provide:

1. A reusable Rust GraphQL federation router.
2. Automatic discovery and composition of registered subgraph schemas.
3. Runtime adoption of valid schema changes without router restart.
4. Rejection of invalid candidate schema changes without affecting the active graph.
5. HTTP GraphQL query and mutation routing.
6. GraphQL WebSocket subscriptions using `graphql-transport-ws`.
7. Direct routing of subscriptions to the subgraph that owns the subscription field.
8. Support for ephemeral, non-replayed notifications.
9. Integration with `agql-auth` JWT claims and authorization semantics.
10. Router-level authorization based on authorization metadata published by subgraphs.
11. Independent authorization enforcement within each subgraph.
12. First-class generation of router-compatible metadata from `graphql-orm`.
13. A generic protocol allowing non-`graphql-orm` GraphQL services to participate.
14. A reusable library API and standalone router binary.
15. Removal of project-specific dependencies on Cosmo, WGC, NATS, JetStream and EDFS where durable messaging is not required.

## 3. Design Principles

### 3.1 Project Agnostic

`graphql-orm-router` must not depend on GEMA-specific:

- service names;
- scope namespaces;
- entity names;
- URLs;
- authentication policies;
- deployment assumptions.

GEMA will be a consumer of the router.

### 3.2 graphql-orm Is Optional

A subgraph does not have to use `graphql-orm`.

Any compatible GraphQL service may participate if it provides the required:

- GraphQL endpoint;
- SDL;
- federation metadata;
- router protocol metadata where applicable.

`graphql-orm` will provide the preferred automated integration.

### 3.3 Subgraphs Remain Authoritative

Router authorization is defence in depth.

A request rejected by the router must not reach a subgraph.

A request accepted by the router must still pass the subgraph's own authentication and authorization checks.

Direct access to a subgraph must never grant greater access than routed access.

### 3.4 One Authorization Declaration

Where `graphql-orm` generates an operation, the authorization requirement should be declared once and used to generate both:

- the authoritative async-graphql resolver guard;
- router-readable authorization metadata.

Authorization metadata must not become an independent configuration source that can silently drift from resolver enforcement.

### 3.5 Current State Is Authoritative

GraphQL subscriptions are intended primarily as live invalidation and state-change notifications.

The default model is:

> Something changed; here is the updated state or enough information to obtain the updated state.

Subscription events are not durable records.

If a client is disconnected while an event occurs, the normal GraphQL query remains the authoritative source of current state.

### 3.6 Last-Known-Good Federation

A failed subgraph health check, failed SDL fetch or failed composition must never automatically remove that subgraph from the active federated graph.

The active graph remains unchanged until a complete candidate graph successfully validates and composes.

## 4. In Scope

### 4.1 Router Runtime

A new `graphql-orm-router` crate will provide:

- HTTP GraphQL endpoint;
- WebSocket GraphQL endpoint;
- federation query planning and execution;
- subgraph HTTP routing;
- subgraph WebSocket routing;
- JWT validation;
- router-level authorization;
- header propagation;
- health endpoints;
- telemetry;
- graph reload support.

A standalone binary will also be supplied.

### 4.2 Federation Engine

The project will reuse an established Rust federation execution engine rather than implement Apollo Federation query planning from scratch.

The initial intended engine is Hive Router or its reusable Rust components.

The external federation engine must remain an implementation detail behind `graphql-orm-router` APIs wherever practical.

### 4.3 Schema Composition

The router will maintain the active federated graph from registered subgraphs.

It will:

- fetch subgraph SDL;
- calculate or consume schema fingerprints;
- detect schema changes;
- compose a candidate supergraph;
- validate the candidate;
- activate it atomically when valid;
- retain the current graph if candidate composition fails.

Composition should use a native Rust implementation where possible.

### 4.4 Subgraph Registry

The router will maintain a registry containing:

- stable subgraph identity;
- GraphQL HTTP endpoint;
- optional WebSocket endpoint;
- SDL endpoint or SDL retrieval method;
- schema fingerprint;
- capability information;
- health state;
- last-known-good schema;
- authorization metadata version;
- registration state.

Subgraphs may be:

- configured statically;
- registered dynamically;
- admitted as candidates;
- activated after successful validation;
- disabled explicitly.

### 4.5 Subscriptions

Subscriptions will use GraphQL WebSockets rather than NATS-backed EDFS.

For a normal `graphql-orm` subgraph:

```text
Database write
    ↓
graphql-orm change event
    ↓
tokio broadcast
    ↓
async-graphql Subscription
    ↓
graphql-orm-router
    ↓
client WebSocket
```

No persistence or replay is required by the router.

### 4.6 Authorization

The project will support:

- JWT authentication;
- local role and scope claims;
- fixed scope requirements;
- any/all scope requirements;
- authentication-only requirements;
- argument-dependent scope templates;
- HTTP authentication;
- WebSocket `connection_init` authentication;
- downstream propagation of the authenticated token.

### 4.7 graphql-orm Integration

`graphql-orm` will be enhanced to expose router-consumable metadata for:

- generated root fields;
- generated subscriptions;
- authentication requirements;
- scope requirements;
- parameterised scope requirements;
- operation fingerprints;
- generated schema fingerprints.

### 4.8 agql-auth Integration

`agql-auth` 0.14 has been enhanced to make its JWT and authorization contract directly usable by the router.

This includes standardising scope claim interoperability.

## 5. Out of Scope

The initial project will not provide:

- durable event storage;
- message replay;
- distributed queues;
- exactly-once delivery;
- workflow orchestration;
- event sourcing;
- replacement for Kafka, NATS JetStream or RabbitMQ;
- arbitrary service discovery platforms;
- database replication;
- automatic horizontal subscription fan-out between independent instances;
- frontend cache management;
- application-specific authorization policy.

If durable asynchronous messaging is required by a project, it should be introduced separately for that workload.

## 6. Multi-Instance Constraint

The initial subscription implementation may use process-local Tokio broadcast channels.

This works where one active instance of a subgraph is responsible for both:

- the write that generates an event;
- the GraphQL subscription serving that event.

Horizontal scaling introduces a separate cross-instance fan-out problem.

Future deployments may add an ephemeral pub/sub provider such as:

- NATS Core;
- Redis Pub/Sub;
- PostgreSQL LISTEN/NOTIFY;
- another project-selected transport.

This must remain optional and must not reintroduce a mandatory durable broker.

## 7. Required Repository Changes

### 7.1 graphql-orm

Implemented changes:

- introduce router-compatible operation authorization metadata;
- expose generated subscription ownership and signatures;
- expose stable schema/operation fingerprints;
- ensure generated subscription resolvers use request-authenticated context;
- provide a single-source authorization declaration for guard and router metadata generation;
- expose fixed and templated scope requirements;
- optionally depend on `graphql-orm-router-protocol`;
- add integration tests covering router metadata drift.

`graphql-orm` must not depend on `graphql-orm-router`.

### 7.2 graphql-orm-macros

Required changes:

- generate router protocol metadata where enabled;
- generate Federation authorization directives or equivalent metadata;
- generate deterministic operation identities;
- support argument references in scope templates;
- ensure compile-time validation of malformed scope templates where possible.

### 7.3 New graphql-orm-router-protocol Crate

This crate will define stable, project-neutral data structures for communication between subgraphs and routers.

It should contain data types only and avoid router implementation dependencies.

### 7.4 New graphql-orm-router Crate

This crate will contain:

- router runtime;
- registry;
- composition;
- authentication;
- authorization;
- schema lifecycle;
- federation runtime integration;
- WebSocket support;
- configuration;
- telemetry;
- standalone binary.

### 7.5 agql-auth

Required changes:

- support a standards-compatible JWT scope claim;
- accept legacy `scopes` tokens during migration;
- expose reusable resource-server validation suitable for router use;
- retain WebSocket `connection_init` validation;
- expose or share scope matching semantics used by subgraphs;
- preserve fail-closed defaults.

Preferred migration:

- new tokens emit `scope`;
- validators accept `scope` and legacy `scopes`;
- legacy support may later be deprecated.

### 7.6 GEMA

GEMA will become a consumer and migration target.

Required eventual changes include:

- replace Cosmo Router with `graphql-orm-router`;
- remove WGC composition;
- remove Cosmo execution config generation;
- remove Cosmo-specific configuration;
- remove the Go subscription authorization module;
- remove NATS-backed EDFS for GraphQL notifications;
- remove JetStream dependency where no other workload requires it;
- migrate generated events to native GraphQL subscriptions;
- update Apollo WebSocket authentication payload to the router-supported standard;
- retain subgraph-side `agql-auth` enforcement.

## 8. Deliverables

The project is complete when the workspace contains:

1. `graphql-orm-router-protocol`.
2. `graphql-orm-router` library.
3. `graphql-orm-router` standalone binary.
4. Updated `graphql-orm` metadata generation.
5. Updated `graphql-orm-macros`.
6. Required `agql-auth` interoperability changes.
7. Router/subgraph integration tests.
8. Subscription integration tests.
9. Authorization equivalence tests.
10. Automatic schema refresh tests.
11. Last-known-good rollback tests.
12. Documentation and example project.
13. GEMA migration plan.

## 9. Success Criteria

The project will be considered successful when:

- multiple GraphQL subgraphs appear as one public graph;
- a new compatible subgraph can be admitted without router restart;
- a valid schema update becomes available automatically;
- an invalid schema update does not change the active graph;
- queries and mutations are correctly federated;
- clients subscribe over the same public GraphQL endpoint;
- generated `graphql-orm` subscriptions work without NATS;
- subscription notifications reach connected clients in real time;
- disconnected clients recover current state using ordinary GraphQL queries;
- router scope decisions match subgraph scope decisions;
- parameterised scopes are enforced correctly;
- unauthorized subscriptions are rejected before opening an upstream subscription;
- direct subgraph access remains protected;
- GEMA can operate without Cosmo Router or NATS for GraphQL federation and notifications.

## 10. Non-Goals

The router is not intended to become:

- a general message broker;
- a durable event bus;
- a distributed database;
- an application server;
- an authentication provider;
- an ORM;
- a replacement for subgraph business logic.

Its responsibility is federation, routing, schema lifecycle and enforcement of declared GraphQL access policy.
