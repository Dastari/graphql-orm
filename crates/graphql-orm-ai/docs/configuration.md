---
title: "AI configuration and limits catalogue"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# AI configuration and limits catalogue

Every public configuration/limit type is linked below. Its source documentation
and `Default` implementation, where present, are canonical; this catalogue
does not invent deployment defaults. Immutable deployment limits can only narrow
GraphQL-managed policy and never grant tool, egress, provider, or resolver
authority.

## Feature and backend matrix

| Feature | Default | Meaning |
| --- | --- | --- |
| `sqlite` | Yes | Initial managed schema and runtime lane. |
| `postgres` | No | PostgreSQL schema and runtime lane. |
| `mssql` | No | Schema/compile support; see [capability matrix](backend-capability-matrix.md). |
| `provider-openai`, `provider-anthropic`, `provider-xai`, `provider-ollama`, `provider-openai-compatible` | No | Explicit provider adapters; each still needs host egress, secret, and budget policy. |
| `local-harness` | No | Installed, deployment-sandboxed JSON-lines driver; never a generic subprocess launcher. |
| `provider-codex-app-server` | No | Experimental local Codex/app-server boundary; deployment owns installation, sandboxing, and credentials. |
| `graphql-case-pascal` | No | Changes the complete public GraphQL naming contract. |

Exactly one persistence backend is required. Provider features do not enable a
provider or authorize egress.

## Provider configuration

`OpenAiProviderConfig`, `AnthropicProviderConfig`, `XAiProviderConfig`,
`OllamaProviderConfig`, and `OpenAiCompatibleProviderConfig` live in
[`src/providers`](../src/providers). They require the matching feature and are
constructed with host-owned secret/endpoint policy. None accepts a model-chosen
URL or incoming bearer credential. `AiCodexAppServerRunLimits` is in
[`codex_app_server.rs`](../src/providers/codex_app_server.rs); it is experimental
and must remain behind `provider-codex-app-server`.

| Type | Fields and defaults/bounds |
| --- | --- |
| [`AiProviderCallLimits`](../src/provider_calls.rs) | `new(events, event_bytes, total_bytes)`: events 1–65,536; byte limits positive, total at least individual, each at most 64 MiB. Tool and built-in call ceilings start at 8 and are separately validated. |
| [`AiProviderAttachmentResolutionLimits`](../src/provider_calls.rs) | Default 8 attachments, 25 MiB each, 50 MiB total. Constructor permits 1–32 attachments and at most 100 MiB per/total. |
| [`AiLocalHarnessLimits`](../src/local_harness.rs) | Default request/frame 2 MiB, output 16 MiB, stderr 16 KiB, 16,384 frames, startup 10 s, turn 120 s, shutdown 5 s, memory 4 GiB, CPU 120 s. The launcher, not this type, proves sandbox enforcement. |
| [`AiCodexAppServerRunLimits`](../src/providers/codex_app_server.rs) | Experimental defaults: 32 processes, 4 per owner, 16 turns/run, startup 30 s, turn 5 min, interrupt/shutdown 5 s. It does not authorize dynamic tools or a local process. |

## Service limits by responsibility

| Area | Public limit types |
| --- | --- |
| Sessions, runs, cancellation | [`AiSessionServiceLimits`](../src/orm_sessions.rs), [`AiRunServiceLimits`](../src/orm_runs.rs), [`AiRunCancellationLimits`](../src/orm_run_cancellation.rs), [`AiSessionTitleWorkLimits`](../src/orm_session_title_work.rs) |
| Provider calls, output, activity | [`AiProviderCallLimits`](../src/provider_calls.rs), [`AiProviderAttachmentResolutionLimits`](../src/provider_calls.rs), [`AiProviderOutputLimits`](../src/orm_provider_output.rs), [`AiProviderSessionLimits`](../src/provider_session.rs) |
| Tools, remote execution, coordinators | [`AiApplicationToolCallLimits`](../src/orm_tools.rs), [`AiRemoteGraphqlExecutionLimits`](../src/remote_execution.rs), [`AiAgentLoopLimits`](../src/agent_loop.rs), [`AiReadOnlyAgentCoordinatorLimits`](../src/orm_coordinator.rs), [`AiSupervisedAgentCoordinatorLimits`](../src/orm_supervised_coordinator.rs) |
| Budgets, pricing, rules | [`AiBudgetServiceLimits`](../src/orm_budget.rs), [`AiBudgetPolicyManagementLimits`](../src/orm_configuration.rs), [`AiPricingCatalogManagementLimits`](../src/orm_pricing.rs), [`AiRuleDeploymentLimits`](../src/rules.rs), [`AiCurrentRuleResolverLimits`](../src/orm_rules.rs) |
| Approval, proposals, UI intent | [`AiApprovalServiceLimits`](../src/orm_approvals.rs), [`AiApprovalWaitReconciliationLimits`](../src/orm_approvals.rs), [`AiProposalServiceLimits`](../src/orm_proposals.rs), [`AiUiIntentDeliveryLimits`](../src/orm_ui_intents.rs) |
| Attachments, live output, retention | [`AiAttachmentServiceLimits`](../src/orm_attachments.rs), [`AiAttachmentCleanupLimits`](../src/orm_attachments.rs), [`AiLiveDeltaCoalescerLimits`](../src/live_delta.rs), [`AiLiveDeltaPersistenceLimits`](../src/orm_live_delta.rs), [`AiSessionRetentionLimits`](../src/orm_session_retention.rs), [`AiInboxPruningLimits`](../src/orm_inbox.rs) |
| Recovery and checkpoints | [`AiCoordinatorCheckpointLimits`](../src/orm_checkpoints.rs), [`AiContextCompactionLimits`](../src/orm_context_compaction.rs), [`AiRestoreCollectorLimits`](../src/orm_restore.rs), [`AiRestorePolicyAuditLimits`](../src/orm_restore.rs), [`AiRestoreAttachmentMetadataAuditLimits`](../src/orm_restore.rs) |
| Background and local integration | [`AiOpenAiBackgroundReconciliationLimits`](../src/orm_background.rs), [`AiOpenAiBackgroundRetrievalLimits`](../src/orm_background.rs), [`OpenAiWebhookVerifierLimits`](../src/providers/openai_webhooks.rs), [`AiLocalHarnessLimits`](../src/local_harness.rs), [`AiCodexAppServerRunLimits`](../src/providers/codex_app_server.rs) |

## Default service limits

These are the concrete defaults most hosts encounter. Constructors with hard
bounds are linked above or in the source table; types without a `Default`
require the host to choose and validate values explicitly.

| Service | Default fields |
| --- | --- |
| [`AiSessionServiceLimits`](../src/orm_sessions.rs) | Title 256 B; message 256 KiB; 10 attachments/message; protected preview 4 KiB. |
| [`AiUiIntentDeliveryLimits`](../src/orm_ui_intents.rs) | Envelope 256 KiB; principal freshness 5 min. Constructor bounds: envelope 128 B–1 MiB, freshness positive and ≤1 h. |
| [`AiContextCompactionLimits`](../src/orm_context_compaction.rs) | 128 messages, 512 blocks, 4 MiB source, 256 KiB summary, 64K tokens, 100 checkpoints/session, 4 recent messages, 5 min principal age. |
| [`AiApprovalServiceLimits`](../src/orm_approvals.rs) / [`AiApprovalWaitReconciliationLimits`](../src/orm_approvals.rs) | Approval: principal 60 s, lifetime 24 h. Reconciliation: principal 60 s, pending 24 h, scan 64; scan constructor bound 1–256. |
| [`AiAttachmentServiceLimits`](../src/orm_attachments.rs) / cleanup | Intake: 25 MiB, filename 255 B, ticket 10 min, processing 1 h. Cleanup: 50 rows, 5 min lease; bounds are 1–200 rows and positive ≤1 h lease. |
| [`AiProviderOutputLimits`](../src/orm_provider_output.rs) / live deltas | Output: 1 MiB/block, 64 blocks, 4 KiB preview, 64 MiB total, 5 min principal age. Live coalescer: 50 ms or 4 KiB. |
| [`AiProviderSessionLimits`](../src/provider_session.rs) | Idle 1 h, absolute 7 d, claim/cleanup leases 5 min, retry delay 1 h, 10 retries, scan 50. |
| [`AiSessionRetentionLimits`](../src/orm_session_retention.rs) / inbox pruning | Retention scan 50 sessions with bounded 100–5,000 related records by kind; inbox: 50 streams × 500 events. |
| [`AiSessionTitleWorkLimits`](../src/orm_session_title_work.rs) / cancellation | Title work: 5 min lease/principal age, 1 h retry, scan 50, 5 provider + 4 transaction retries, 256 B title/256 KiB message. Cancellation: 5 min principal age, 64 active tool calls. |
| [`AiOpenAiBackgroundReconciliationLimits`](../src/orm_background.rs) / retrieval | Reconciliation: 1 min lease, 5 min retry, scan 64, 16 retries, 8 transaction retries. Retrieval: source constants cap response/visible bytes and item counts; default request timeout 30 s, principal age 5 min. |
| [`AiRestoreCollectorLimits`](../src/orm_restore.rs) | 10,000 each for runs, approvals, and egress consents. Policy/attachment audit types require explicit host-attested ceilings. |

## Security-sensitive policies

[`AiContentProtectionPolicy`](../src/content_protection.rs),
[`AiToolPolicySet`](../src/tools.rs), access/egress policies, principal
rehydration, secret stores, and `AiRuleDeploymentLimits` are proof boundaries,
not ergonomic defaults. Begin with the tool-free mock stage in
[getting started](getting-started.md), then add one boundary at a time using
[security](security.md), [worker/provider turns](worker-provider-turn.md), and
[control-plane gates](control-plane-production.md).
