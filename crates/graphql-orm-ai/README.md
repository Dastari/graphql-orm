---
title: "graphql-orm-ai"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-14
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-ai

A project-neutral, security-first AI runtime for `graphql-orm` applications.
It turns explicitly reviewed, server-authored GraphQL operations into agent
tools while keeping application authorization, disclosure, approvals, spend,
and durable history under host control.

It is not a chatbot server, a generic agent loop, a raw-SQL interface, an
arbitrary model-authored GraphQL executor, a shell runner, or an authorization
substitute. Application work always runs through the host's authenticated
GraphQL resolvers; a provider, tool registration, or approval never grants
resolver authority.

## Install

This active pre-release is Git-only. Pin one reviewed full monorepo revision
for AI, ORM, storage, backup, and tool-profile packages:

```toml
[dependencies]
graphql-orm-ai = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.79.0", default-features = false, features = ["sqlite"] }
```

Exactly one persistence backend is required: `sqlite` (default), `postgres`,
or `mssql`. MSSQL currently has schema/compile support only where ORM write
parity is incomplete. This unpublished crate has no docs.rs page; package docs
and rustdoc are available from the pinned repository revision.
Upgrade deliberately: review all pinned companions' migration guides and
changelogs, update them to one reviewed revision, and run recovery-path tests
before using the new pin with protected production data.

## Start safely

Start with the [staged getting-started path](docs/getting-started.md):

1. Compose `AiSchemaModule` in a test-owned SQLite host and use `MockProvider`
   with no tools, network, or secrets.
2. Add one reviewed provider under exact secret, egress, and budget policy.
3. Add default-deny read-only tools only when required.
4. Add approval-bound consequential work and production workers last.

There is intentionally no universal one-call “chat server” example: host schema
application, principal rehydration, GraphQL executor, content protection,
egress, and readiness are application proof boundaries. The guide identifies
the compiled test-backed recipe and the missing reusable bootstrap API.

## What it provides

- Managed AI schema records for protected sessions, messages, runs, budgets,
  provider calls, tool calls, approvals, audit, and restore readiness.
- Fenced durable provider turns, streaming output, bounded checkpoints,
  cancellation, recovery, retention, and current-principal rehydration.
- Watermark-bounded, contiguous durable session and owner-inbox replay whose
  `HasMore` contract remains correct at the configured ORM page maximum.
- Default-deny application tools with server-authored documents and static
  disclosure schemas; consequential work is exact-preview and one-shot
  approval bound.
- Backend-neutral automatic query capabilities compiled from the finished SDL
  and canonical semantic catalogue into closed typed plans. Nested
  relationships and opt-in aggregate roots remain explicitly bounded, and
  one read-only provider plan may expose these capabilities beside exact
  legacy/static read descriptors without duplicating either policy contract.
  Secret/`NeverExport` fields never enter provider schemas.
- Provider-neutral adapters plus deterministic network-free mocks.
- Optional provider profiles, attachments, skills, UI intents, rules, and
  usage/pricing controls, each behind independent proof and policy boundaries.

Detailed mechanics stay in the task-oriented [documentation index](docs/README.md)
rather than hiding the quick start behind an implementation inventory.

## Automatic GraphQL query execution

An owning subgraph builds `AiGraphqlQueryCapabilityCatalog` only after its SDL
and `GraphqlSemanticCatalog` are complete. `AiToolCatalog` registers that
complete catalogue for discovery and projects individual closed provider tool
definitions. When the provider returns a plan, call
`AiRuntime::execute_query_capability` with the stable ID, exact offered
capability fingerprint, typed plan and invocation context.

That boundary compiles the plan into one exact GraphQL document, variables,
selection/disclosure schema and dynamic descriptor. The authenticated bridge
then rehydrates the principal, invokes current target/tool policy, builds the
canonical request context, and lets the ordinary resolver authorize the call.
Registration alone remains default-deny. The runtime enforces descriptor byte
and total-record bounds and rejects response fields outside the selected
disclosure shape. Total records include sibling and nested relationship
expansion, not merely the largest returned list.

Private remote execution carries a crate-authored
`AiRemoteGraphqlCapabilityBinding` to the deployment issuer. Static reads bind
their exact registered descriptor; generated reads additionally bind the
active target, finished schema, semantic catalogue/root and offered capability
fingerprints. Issuers can therefore authorize an exact generated query without
interpreting its dynamic operation name. See
[private remote GraphQL execution](docs/remote-graphql-execution.md).

Mutation and subscription execution do not use the query path. The
backend-neutral profile package can compile a bounded `ReplayThenLive`
subscription observation. `OrmAiSubscriptionWaitService` binds that plan to an
authenticated registered replay source and the existing run queue; it
rehydrates current authority at open, event and adoption boundaries. See
[durable bounded subscription waits](docs/durable-subscription-waits.md).

## Features and capability boundary

| Feature | Default | Meaning |
| --- | --- | --- |
| `sqlite` | Yes | Managed schema and runtime lane. |
| `postgres` | No | PostgreSQL lane. |
| `mssql` | No | Schema/compile lane; check the capability matrix. |
| `provider-*` | No | Opt-in OpenAI, Anthropic, xAI, Ollama, or managed OpenAI-compatible adapters. |
| `local-harness` | No | Installed sandboxed JSON-lines v2 driver; not a generic subprocess launcher. |
| `provider-codex-app-server` | No | Experimental local adapter; deployment owns sandboxing, credentials, and network policy. |
| `graphql-case-pascal` | No | Changes the public GraphQL naming contract. |

Provider features do not configure a model, authorize egress, or disclose
data. Tool discovery and registration are not authorization. Every provider
call requires exact egress and atomic budget proof; every application tool
uses current principal rehydration, a static result disclosure contract, and
ordinary resolver authorization.

Each provider feature is independently buildable with one persistence backend.
Use `scripts/check-ai-provider-lanes.sh` from the repository root to verify one
feature at a time; provider feature unification is not required for a valid
adapter build.

## Configuration, operations, and errors

The [configuration and limits catalogue](docs/configuration.md) lists every
public service limit and provider configuration source without fabricating
defaults. Runtime work is fenced and fail-closed: uncertain effects require
recovery rather than replay. Provider or worker errors never carry a license
to retry an action with side effects.

Hosts own secrets, endpoint policy, principal lifecycle, deployment rules,
GraphQL resolvers, migration application, operational scheduling, and restore
readiness. The crate owns its protected records, bounded orchestration
contracts, and security checks.

## Further reading

- [Documentation index](docs/README.md)
- [Getting started](docs/getting-started.md), [architecture](docs/architecture.md), and [security model](docs/security.md)
- [Backend capability matrix](docs/backend-capability-matrix.md)
- [Read-only tools](docs/read-only-tool-loop.md), [supervised mutations](docs/supervised-tool-loop.md), and [provider turns](docs/worker-provider-turn.md)
- [Recovery and restore](docs/recovery-and-restore.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
