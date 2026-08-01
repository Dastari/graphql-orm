---
title: ADR-0006 Streaming storage and backup layering
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2027-08-01
supersedes: []
---

# ADR-0006: Streaming storage and backup layering

## Context

Large objects, backups, recordings, and HTTP range responses need bounded
memory and provider capabilities such as ranges, multipart writes, conditional
create, server-side copy, and paged listing. Duplicating those transports in
backup or coupling storage to application metadata would fragment behavior and
security rules.

## Decision

`graphql-orm-storage` owns provider-neutral byte/stream APIs and provider
implementations. `BlobStore` is the backup-facing provider boundary;
`ObjectStorage` and higher-level services may extend it for application object
metadata. `graphql-orm-backup` adapts `BlobStore` and owns manifests,
repositories, atomic locks, verification, compaction, and restore semantics.

Buffered methods may remain compatibility wrappers, but streaming is the
large-object path. Atomic repository locking uses provider-side
create-if-absent. HTTP request parsing, `206`/`416` selection, response headers,
and delivery policy remain host responsibilities.

## Consequences

- S3, SMB, local, and future Azure transport code has one owner.
- Backup can change manifest/restore behavior without duplicating providers.
- Consumers can serve ranges safely while retaining control of HTTP and auth.
- Provider capability gaps fail explicitly instead of being emulated with
  unsafe exists-then-write or unbounded buffering.

## Supersession

A different provider boundary or ownership split requires a new ADR and
migration plan for storage and backup consumers.
