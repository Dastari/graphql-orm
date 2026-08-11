---
title: "graphql-orm-ai-tool-profiles changelog"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-11
supersedes: []
---

# Changelog

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
