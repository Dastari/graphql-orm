---
title: "AI getting started"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# AI getting started

Build in stages. The first stage is a deterministic, tool-free, SQLite
foundation; it has no network provider, secrets, egress, or model-selected
actions. Add authority-bearing features only after that foundation is tested.

## 1. Add the SQLite foundation

The crate is unpublished and Git-only. Pin one reviewed full monorepo revision
for AI, ORM, storage, backup, and tool-profile packages:

```toml
[dependencies]
graphql-orm-ai = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.77.0", default-features = false, features = ["sqlite"] }
```

Exactly one persistence backend is required: `sqlite` (default), `postgres`,
or `mssql`. MSSQL is schema/compile support until ORM write parity lands; use
the [capability matrix](backend-capability-matrix.md) before selecting it.
For upgrades, review every companion package's migration guide and changelog,
move the complete dependency universe to one reviewed revision, and test
recovery paths before production adoption.

Compose `AiSchemaModule` with the host's managed ORM schema, apply that schema
to a test-owned SQLite database, and bind the host's authenticated GraphQL
executor before opening any AI work. The durable schema module is public and
the provider fixture is network-free:

```rust
use graphql_orm_ai::{AiSchemaModule, MockProvider};
use graphql_orm::graphql::orm::OrmSchemaModule;

let module = AiSchemaModule;
assert!(!module.entities().is_empty());
let provider = MockProvider::new(Vec::new());
assert_eq!(provider.request_count(), 0);
```

That code is intentionally not a runnable chat application. There is no
reusable public “host bootstrap” API yet because schema application, principal
construction, GraphQL executor, content protection, egress, and runtime
readiness are deployment-owned proof boundaries. The source-backed host recipe
is demonstrated by the package's SQLite tests, especially
[`tests/orm_sessions.rs`](../tests/orm_sessions.rs),
[`tests/provider_and_content_security.rs`](../tests/provider_and_content_security.rs),
and `AiRuntimeBuilder` in [`src/runtime.rs`](../src/runtime.rs). A future
project-neutral demo should package these bindings without weakening them.

For this stage, keep tools unregistered, do not enable a provider feature, do
not configure endpoints/secrets, and keep `AiRuntimeStartGate` closed until
the host has applied schema and bound its executor. This validates the data
model and deterministic provider boundary without external effects.

## 2. Add one provider

Enable exactly the provider adapter you intend to use:

| Feature | Guide | Important boundary |
| --- | --- | --- |
| `provider-openai` | [OpenAI](openai-background.md) | Exact egress/budget proof; webhook and background paths are separate. |
| `provider-anthropic` | [Anthropic](anthropic.md) | Fixed official endpoint and secret-store reference. |
| `provider-xai` | [xAI](xai.md) | ZDR verification is on by default. |
| `provider-ollama` | [Ollama](ollama.md) | Explicit endpoint policy even for loopback. |
| `provider-openai-compatible` | [OpenAI-compatible](openai-compatible.md) | Exact managed profile, capability, retention label, and deployment URL. |

Provider features compile adapters; they do not enable a provider, authorize
egress, disclose content, or grant credentials. Configure a secret-store
reference and host-owned endpoint policy, then require an exact egress manifest
and atomic budget reservation for every call. See [worker/provider turns](worker-provider-turn.md).

## 3. Add read-only tools, only if needed

Register server-authored GraphQL operations with static result disclosure
schemas, then separately enable the exact tool. Tool discovery and registration
are not authorization. Rehydrate the principal before every resolver call and
again after relevant I/O; application resolver authorization remains final.

Follow [the read-only tool-loop guide](read-only-tool-loop.md). Do not add raw
SQL, arbitrary model-authored GraphQL, shell execution, or direct repository
access. Consequential work is a later, approval-bound stage; follow
[supervised mutations](supervised-tool-loop.md), not this quickstart.

## 4. Production hardening

Before opening the runtime start gate, complete the package's production
integration obligations:

1. Bind current-principal rehydration, access, tool authorization, egress,
   secret-store, and content-protection implementations.
2. Configure immutable deployment limits and narrower managed policy for
   budgets, retention, tools, providers, rules, and UI intents.
3. Install the bounded workers actually used: recovery/reconciliation, session
   retention, inbox pruning, attachment cleanup, and any provider-specific
   reconciler.
4. Apply and validate managed migrations; after restore, reconcile before
   declaring readiness.
5. Exercise denial, cancellation, provider failure, and restore paths against
   test-owned infrastructure.

The detailed production checklist is now [control-plane and production
integration gates](control-plane-production.md). It remains required guidance,
but is deliberately not the first learning step.

## Local and experimental integrations

`local-harness` supports only a deployment-installed, sandboxed JSON-lines v2
driver. It is not a generic inherited-environment subprocess launcher.
`provider-codex-app-server` is experimental and must remain explicitly
feature-gated; the deployment owns installation, sandboxing, network boundary,
and credentials. Neither feature grants filesystem, shell, network, or tool
authority to a model.

See [configuration and limits](configuration.md), [security](security.md), and
[recovery and restore](recovery-and-restore.md) before production use.
