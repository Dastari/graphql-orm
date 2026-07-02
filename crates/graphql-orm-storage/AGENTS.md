# graphql-orm-storage Agent Guide

This crate is a reusable storage companion for applications that use `graphql-orm`.

## Skills

- Use `.agents/skills/rust-skills/SKILL.md` for all Rust implementation, review, refactoring, performance, and API design work.
- Use `.agents/skills/graphql-orm-macros/SKILL.md` for graphql-orm integration decisions.

## Rules

- Keep the crate generic and reusable.
- Do not add Digitise-specific domain names, entity names, collection semantics, accession logic, record logic, media workflows, or policy assumptions.
- Do not store file bytes in a database.
- Prefer traits and small adapters over application-specific coupling.
- Keep provider-specific code behind feature flags.
- Local filesystem support is the baseline provider.
- S3 and Azure Blob support should be explicit feature-gated work; Azure placeholder paths must return clear unsupported errors until implemented.
- Add tests for path safety, checksums, key generation, and provider round trips.

## Current Agent Handoff

- Current crate version is `0.3.0`.
- The storage provider boundary is now the streaming `BlobStore` trait.
- `ObjectStorage` extends `BlobStore`; custom providers must implement `BlobStore` first.
- `BlobStore` includes byte ranges, conditional writes, server-side copy, write options, and paged listing.
- `StorageService` remains the high-level primary object API for generated object metadata.
- `graphql-orm-backup` should adapt `BlobStore` directly for backup repository semantics; it should not use `StorageService`.
- S3 is implemented behind the `s3` feature through the shared `BlobStore` provider layer.
- Azure Blob is still a feature-gated unsupported placeholder. Do not add real Azure SDK code without implementing the shared `BlobStore` provider layer first.
- See `docs/agent-update.md`, `docs/blob-store.md`, `docs/streaming.md`, and `docs/backup-integration.md` before making provider or backup-facing changes.
