---
title: "graphql-orm-ai-tool-profiles changelog"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-11
supersedes: []
---

# Changelog

## [0.4.0] - 2026-08-13

### Added

- Automatic query capabilities compile every finished-SDL `Query` root from
  the canonical semantic catalogue into a finite typed plan schema, exact
  server-authored document and variables, selected disclosure shape, stable
  identity, and complete schema/catalogue/operation/plan fingerprints.
- Explicit bounded nested relationship selection and opt-in generated
  aggregate roots no longer require a hand-authored GraphQL document or static
  AI profile. Secret and `NeverExport` fields remain structurally absent.
- Replayable subscription capabilities compile one bounded event projection,
  optional admitted top-level condition, timeout and event ceiling for a
  separate durable waiter implementation. Best-effort subscriptions remain
  described but receive no durable capability.
- `GraphqlOperationContract::with_semantic_operation_kind` binds query or
  subscription documents to an exact canonical semantic root; the existing
  query convenience API remains source compatible.

### Security

- Finished SDL and semantic root coverage must match exactly. Capacity,
  provider-schema size, relationship cycles, missing collection bounds,
  unknown selections, stale capability fingerprints and schema drift fail
  readiness or compilation instead of silently omitting authority-relevant
  metadata.

## [0.3.0] - 2026-08-11

### Fixed

- Manifest and tool-descriptor fingerprints now hash recursively canonicalized
  JSON object keys. Canonical `DescriptorExtension` transport can no longer
  make an unchanged manifest appear stale merely by reordering nested object
  members.

### Changed

- `AI_GRAPHQL_TOOL_MANIFEST_VERSION` is now 2. Producers and consumers must
  move together so the corrected fingerprint semantics cannot be confused
  with version 1.
