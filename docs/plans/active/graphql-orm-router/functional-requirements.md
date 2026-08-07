---
title: GraphQL ORM Router functional requirements
kind: reference
status: draft
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-07
supersedes: []
---

# GraphQL ORM Router — Functional Requirements

## Status

Proposed.

## 1. Introduction

`graphql-orm-router` provides a project-agnostic federated GraphQL entry point over multiple GraphQL microservices.

It must support queries, mutations and live GraphQL subscriptions while allowing schemas to evolve independently.

The router must integrate particularly well with `graphql-orm` and `agql-auth`, but neither application business logic nor GEMA-specific behaviour may form part of the generic router contract.

## 2. Terminology

### Router

The public GraphQL gateway provided by `graphql-orm-router`.

### Subgraph

An independently deployed GraphQL service participating in the federated graph.

### Candidate Subgraph

A newly registered subgraph that has not yet been admitted into the active graph.

### Active Graph

The currently validated and executable federated schema.

### Candidate Graph

A proposed replacement graph produced after a registration or schema change.

### Last-Known-Good Schema

The most recent successfully admitted schema for a subgraph.

### Router Protocol

Project-neutral metadata used by subgraphs and the router for discovery, capabilities and authorization.

## 3. Public GraphQL Endpoint

### FR-001

The router shall expose a configurable HTTP GraphQL endpoint.

Default:

```text
/graphql
```

### FR-002

The router shall expose GraphQL subscriptions using `graphql-transport-ws`.

### FR-003

HTTP GraphQL and WebSocket GraphQL shall be capable of sharing the same public `/graphql` path.

### FR-004

Clients shall not require knowledge of individual subgraph addresses.

### FR-005

Queries, mutations and subscriptions shall use the federated schema visible through the public endpoint.

## 4. Subgraph Registration

### FR-010

The router shall support statically configured subgraphs.

### FR-011

The router shall support dynamically registered candidate subgraphs.

### FR-012

Every registered subgraph shall have a stable unique identifier.

### FR-013

A registration shall include or resolve:

- subgraph name;
- GraphQL endpoint;
- SDL retrieval location or method;
- optional WebSocket endpoint;
- protocol version;
- advertised capabilities.

### FR-014

Dynamic registration shall require authenticated service identity.

### FR-015

Registering a subgraph shall not immediately modify the active graph.

### FR-016

The router shall fetch and validate the candidate subgraph's SDL before admission.

### FR-017

The router shall attempt composition of the complete candidate graph before activation.

### FR-018

A candidate shall be activated only after all required validation succeeds.

### FR-019

A failed candidate admission shall leave the active graph unchanged.

### FR-020

The router shall expose the reason a candidate was rejected through administrative status and telemetry.

## 5. Schema Discovery

### FR-030

The router shall automatically monitor active subgraphs for schema changes.

### FR-031

The preferred schema change mechanism shall support an inexpensive fingerprint or ETag comparison.

### FR-032

Unchanged schemas shall not require full composition.

### FR-033

When a schema fingerprint changes, the router shall retrieve the candidate SDL.

### FR-034

The router shall compose the candidate SDL with the last-known-good SDLs of all other active subgraphs.

### FR-035

The router shall validate the complete candidate graph.

### FR-036

Successful composition shall atomically replace the active graph.

### FR-037

Failed composition shall retain the previous active graph.

### FR-038

A temporarily unavailable subgraph shall not be automatically removed from the active graph.

### FR-039

Schema polling interval shall be configurable.

### FR-040

Manual schema refresh shall be supported through an authenticated administrative operation.

## 6. Federation

### FR-050

The router shall support Apollo Federation-compatible subgraphs to the level provided by the selected federation engine.

### FR-051

The router shall support queries spanning multiple subgraphs.

### FR-052

The router shall support federated entity resolution.

### FR-053

The router shall support mutations routed to their owning subgraphs.

### FR-054

The router shall produce standard GraphQL responses.

### FR-055

The router shall preserve appropriate GraphQL error paths when downstream errors occur.

### FR-056

Federation implementation details shall not be exposed as mandatory APIs of `graphql-orm-router`.

## 7. Subscriptions

### FR-060

A subgraph may expose standard async-graphql subscription root fields.

### FR-061

The router shall expose those subscription fields through the federated graph.

### FR-062

A client subscription shall be routed to the subgraph owning the selected subscription root.

### FR-063

The router shall establish the required upstream WebSocket subscription automatically.

### FR-064

A subgraph shall not require NATS, JetStream or EDFS in order to participate in subscriptions.

### FR-065

`graphql-orm` generated subscriptions shall be usable through the router.

### FR-066

`graphql-orm` generated writes shall be able to publish change events to generated subscriptions through process-local asynchronous broadcast.

### FR-067

Subscription delivery shall be ephemeral by default.

### FR-068

The router shall not persist subscription events.

### FR-069

The router shall not replay events missed while a client is disconnected.

### FR-070

Subscription events may be dropped for a slow or disconnected consumer according to bounded buffering policy.

### FR-071

The application's underlying state store shall remain authoritative after missed events.

### FR-072

The router may deduplicate compatible upstream subscription connections and fan events out to multiple clients where supported by the federation runtime.

## 8. Authentication

### FR-080

The router shall support JWT bearer authentication.

### FR-081

JWT validation shall support:

- signature verification;
- issuer validation;
- audience validation;
- expiry validation;
- key ID selection;
- JWKS key retrieval or equivalent configured public key validation.

### FR-082

HTTP GraphQL requests shall accept bearer authentication.

### FR-083

WebSocket authentication shall support credentials supplied during `connection_init`.

### FR-084

Invalid WebSocket authentication shall fail closed.

### FR-085

The router shall support propagation of an approved authorization credential to downstream subgraphs.

### FR-086

Subgraphs shall independently validate propagated authentication.

### FR-087

The router shall not become the authoritative issuer of user authentication tokens.

## 9. Scope Authorization

### FR-090

Operations may declare authentication requirements.

### FR-091

Operations may declare one or more required scopes.

### FR-092

Scope policy shall support:

- one required scope;
- any-of scope sets;
- all-of scope sets.

### FR-093

Scope comparison semantics shall be compatible with the configured `agql-auth` scope matcher.

### FR-094

The router shall reject an operation that does not satisfy its declared router policy.

### FR-095

Subgraph resolver authorization shall execute independently even where the router has already authorized the operation.

### FR-096

Missing router authorization metadata must not disable an authoritative subgraph guard.

### FR-097

The router shall support argument-dependent scope templates.

Example:

```text
gema.fame.endpoint.{Id}.read
```

Given:

```text
Id = endpoint-123
```

the evaluated scope becomes:

```text
gema.fame.endpoint.endpoint-123.read
```

### FR-098

Template expansion shall use GraphQL operation arguments after variable resolution.

### FR-099

Failure to resolve a required scope template shall fail closed.

### FR-100

Router and subgraph authorization requirements generated from `graphql-orm` shall originate from the same metadata declaration.

## 10. graphql-orm Generated Metadata

### FR-110

`graphql-orm` shall expose deterministic metadata for generated operations.

### FR-111

Metadata shall include:

- operation identity;
- root field name;
- operation type;
- argument definitions;
- result type;
- auth mode;
- declared scopes;
- any/all semantics;
- templated scope references;
- operation fingerprint.

### FR-112

Generated subscription metadata shall identify subscription root fields.

### FR-113

Metadata generation shall not itself authorize execution.

### FR-114

The generated resolver guard shall remain the authoritative subgraph enforcement point.

### FR-115

Schema and authorization metadata drift shall be detectable through deterministic fingerprints.

## 11. Router Protocol

### FR-120

A lightweight `graphql-orm-router-protocol` crate shall define the interoperable router contract.

### FR-121

The protocol crate shall not depend on:

- Hive Router;
- Axum server runtime;
- graphql-orm database backends;
- GEMA;
- application-specific types.

### FR-122

Non-`graphql-orm` services shall be able to implement the protocol manually.

### FR-123

Protocol payloads shall be versioned.

### FR-124

Unknown incompatible protocol versions shall fail registration clearly rather than being silently accepted.

## 12. Health and Availability

### FR-130

The router shall expose liveness and readiness endpoints.

### FR-131

Router readiness shall require a valid active graph.

### FR-132

Subgraph health state shall be independently observable.

### FR-133

Temporary subgraph unavailability shall not mutate the active schema.

### FR-134

The router shall distinguish:

- registered;
- candidate;
- active;
- unhealthy;
- rejected;
- disabled.

## 13. Administrative Status

### FR-140

The router shall expose authenticated administrative state describing:

- active graph version;
- active graph fingerprint;
- known subgraphs;
- current subgraph fingerprints;
- last successful composition;
- rejected candidates;
- last composition errors.

### FR-141

Administrative endpoints shall not expose JWTs, secrets or sensitive downstream credentials.

## 14. Configuration

### FR-150

Configuration shall support:

- listener address;
- public GraphQL path;
- authentication/JWKS settings;
- static subgraphs;
- schema refresh interval;
- request limits;
- WebSocket limits;
- telemetry;
- administrative endpoint policy.

### FR-151

Secrets shall be externally supplied and not stored in committed configuration.

### FR-152

Environment variables and structured configuration files may both be supported.

## 15. Observability

### FR-160

The router shall emit structured tracing.

### FR-161

Metrics shall include at minimum:

- HTTP GraphQL request count;
- GraphQL failures;
- subgraph request latency;
- active WebSocket connections;
- active subscriptions;
- schema refresh attempts;
- composition successes;
- composition failures;
- rejected subgraphs;
- authorization denials.

### FR-162

Raw bearer tokens shall never be logged.

### FR-163

Sensitive GraphQL variable values shall not be logged by default.

## 16. Security

### FR-170

Public GraphQL access shall support configurable authentication-required defaults.

### FR-171

Administrative operations shall require explicit authentication and authorization.

### FR-172

Subgraph registration shall require trusted service identity.

### FR-173

Schema retrieval endpoints shall be capable of requiring internal service authentication.

### FR-174

The router shall enforce configurable request body, parser, depth and complexity limits.

### FR-175

WebSocket connection and subscription counts shall be bounded.

### FR-176

Authorization and schema errors shall fail closed.

## 17. Compatibility

### FR-180

The router library shall be independently usable without GEMA.

### FR-181

A project shall be able to use `graphql-orm-router` without using `graphql-orm`.

### FR-182

A project shall be able to use `graphql-orm` without using `graphql-orm-router`.

### FR-183

`agql-auth` shall remain independently usable.

### FR-184

Optional integrations shall not create cyclic crate dependencies.

## 18. GEMA Acceptance Requirements

GEMA will serve as the first full integration target.

The integration shall demonstrate:

1. Removal of Cosmo Router.
2. Removal of Cosmo WGC runtime composition.
3. Removal of NATS/JetStream from GraphQL notifications.
4. Removal of generated EDFS event routing.
5. Removal of the Cosmo Go subscription-auth module.
6. Automatic composition of existing GEMA subgraphs.
7. Automatic adoption of schema changes.
8. Native `graphql-orm` subscriptions over the federated endpoint.
9. Existing GEMA scope semantics.
10. Existing parameterised endpoint scopes.
11. Apollo HTTP and WebSocket client compatibility.
12. Last-known-good graph behaviour during invalid or unavailable subgraph updates.

## 19. Acceptance Criteria

The implementation shall not be considered production-ready until automated integration tests demonstrate:

- valid graph startup;
- invalid graph startup rejection;
- new subgraph admission;
- incompatible subgraph rejection;
- live schema addition;
- live schema removal where explicitly approved;
- failed update rollback;
- HTTP query federation;
- mutation federation;
- WebSocket subscription;
- authentication failure;
- fixed-scope authorization failure;
- templated-scope authorization failure;
- valid scope acceptance;
- token expiry handling;
- WebSocket reauthentication policy;
- subgraph-side guard enforcement;
- notification delivery without NATS.
