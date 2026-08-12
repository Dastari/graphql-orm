---
title: "graphql-orm-ai documentation"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-ai documentation

Start with the package [README](../README.md), then take the staged
[getting-started path](getting-started.md). The first stage is deterministic,
tool-free, and network-free; production integration comes later.

## Learn

- [Getting started](getting-started.md) — staged SQLite foundation, providers, tools, and production.
- [Architecture](architecture.md) and [security model](security.md).
- [Backend capability matrix](backend-capability-matrix.md).
- [Configuration and limits](configuration.md) — public type catalogue and source-backed defaults.

## How-to

- [Run a durable provider turn](worker-provider-turn.md).
- [Add a read-only application-tool loop](read-only-tool-loop.md) or [supervised mutations](supervised-tool-loop.md).
- [Set budgets and report usage](usage-and-budgets.md).
- [Use OpenAI](openai-background.md), [Anthropic](anthropic.md), [xAI](xai.md), [Ollama](ollama.md), or an [OpenAI-compatible endpoint](openai-compatible.md).
- [Use the installed local harness](local-harness.md) or experimental [Codex app-server adapter](provider-sessions-and-hosted-activity.md).
- [Recover after backup or restore](recovery-and-restore.md).

## Reference

- [Attachments](attachments.md), [provider files](provider-files.md), [live streaming](live-streaming.md), [context compaction](context-compaction.md), [skills and UI intents](skills-and-ui-intents.md), and [remote GraphQL execution](remote-graphql-execution.md).
- [Migration guide](../MIGRATION.md) and [changelog](../CHANGELOG.md).

## Concepts and operations

- [Hierarchical rules](hierarchical-rules.md), [review lifecycles](review-lifecycles.md), [recovery](recovery-and-restore.md), [session retention](session-retention.md), and [operational telemetry](operational-telemetry.md).
- [Development](development.md), [release process](release-process.md), and [upstream contributions](upstream-contributions.md).

Completed plans and archives remain historical evidence and are intentionally
not linked from this newcomer index.
