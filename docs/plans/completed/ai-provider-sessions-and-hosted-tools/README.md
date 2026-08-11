---
title: AI provider sessions, hosted tools, and visible activity completion
kind: plan
status: accepted
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-11
supersedes: []
---

# AI provider sessions, hosted tools, and visible activity

## Outcome

`graphql-orm-ai` now provides project-neutral contracts for protected
provider-retained sessions, bounded local app-server execution,
provider-hosted web search, visible reasoning summaries, and one ordered
durable activity stream without transferring application authority into a
provider process.

## Delivered boundaries

- Exact run-bound local-process admission, bounded reuse, interruption,
  terminal cleanup, strict protocol allowlisting, and kill-on-drop.
- Protected provider-session bindings with canonical transcript watermarks,
  owner/scope/profile/model/protocol/policy fencing, bounded retention, exact
  cleanup, and fail-closed restore audit.
- Host-requested visible reasoning summaries clearly separated from hidden
  reasoning, with protected persistence, replay, cancellation, and limits.
- Provider-retained mixed hosted-search and registered application tools,
  explicit public/allowed/blocked domain policy, authoritative HTTPS citation
  provenance, and cumulative per-run search ceilings.
- Default-off experimental synchronous dynamic tools that enter only through
  the coordinator's existing registered-operation, current-principal,
  disclosure, egress, budget, cancellation, and resolver-authorization path.
- A strict Codex app-server adapter supporting protected thread
  create/resume/interrupt/delete and the exact bounded lifecycle messages
  required by the reviewed protocol. It exposes no generic JSON-RPC, shell,
  filesystem, browser, screenshot, remote-control, MCP, or arbitrary tool
  capability.

Warm operating-system processes and provider-retained threads remain separate
policies. Application credentials and delegated GraphQL authority never enter
the provider process.

## Acceptance evidence

- Package tests cover strict protocol admission, correlation, replay,
  cancellation, stale fences, provider-session cleanup, mixed tools, search
  ceilings, citations, visible summaries, and negative capability space.
- SQLite and disposable PostgreSQL persistence lanes, MSSQL compile lanes,
  provider matrices, PascalCase GraphQL, Clippy, Rustdoc, SemVer, release
  policy, and documentation checks are part of CI and the workspace release
  workflow.
- Package version 0.73.0 uses AI schema module 0.55.0 for the completed
  milestone.

## Deferred work

Cross-owner process or thread multiplexing remains unsupported until a
provider protocol and isolation assessment prove it safe. A visual-browser
broker remains a distinct future capability boundary described in the
component architecture; it is not part of hosted web search or this completed
plan.
