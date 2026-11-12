---
title: "graphql-orm-storage Agent Guide"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-storage Agent Guide

This crate is a reusable storage companion for applications that use `graphql-orm`.

## Skills

- Use `.agents/skills/rust-skills/SKILL.md` for all Rust implementation, review, refactoring, performance, and API design work.
- Use `.agents/skills/graphql-orm-macros/SKILL.md` for graphql-orm integration decisions.

## Rules

- Keep the crate generic and reusable.
- Do not add consumer-specific domain names, entity names, collection
  semantics, record workflows, or policy assumptions.
- Do not store file bytes in a database.
- Prefer traits and small adapters over application-specific coupling.
- Keep provider-specific code behind feature flags.
- Local filesystem support is the baseline provider.
- S3 and Azure Blob support should be explicit feature-gated work; Azure placeholder paths must return clear unsupported errors until implemented.
- Add tests for path safety, checksums, key generation, and provider round trips.

## Integration contracts

- Package versions and workspace dependency identities belong to the generated
  [workspace inventory](../../docs/reference/workspace-packages.md).
- Internal packages resolve through workspace path dependencies and the root
  `Cargo.lock`. Keep downstream companion packages on one reviewed monorepo
  revision so public types share one canonical source.
- `graphql-orm` owns the optional `agql-auth` integration. The root
  [Cargo.lock](../../Cargo.lock) and published workspace release manifest own
  the external authorization revision; this crate does not depend on
  application authorization.
- The storage provider boundary is now the streaming `BlobStore` trait.
- `ObjectStorage` extends `BlobStore`; custom providers must implement `BlobStore` first.
- `BlobStore` includes byte ranges, conditional writes, server-side copy, write options, and paged listing.
- `StorageService` remains the high-level primary object API for generated object metadata.
- `StreamingObjectStore` supports bucket/key large-object streaming, multipart writes, range reads, metadata, listing, and retention deletion.
- `graphql-orm-backup` should adapt `BlobStore` directly for backup repository semantics; it should not use `StorageService`.
- S3 is implemented behind the `s3` feature through the shared `BlobStore` provider layer.
- Native SMB2/SMB3 is implemented behind the `smb` feature as
  `SmbStorageBackend`. Construct it from runtime `SmbStorageConfig` credentials;
  never translate native SMB fields into a mount path or persist its password.
- Backup integrations should wrap `Arc<dyn BlobStore>` in
  `graphql-orm-backup::BlobStoreBackupRepository`. Do not duplicate SMB
  transport, manifest, retention, or locking code.
- `put_blob_if_not_exists` on SMB uses server-side `FILE_CREATE` and is the
  required atomic primitive for repository locking. Do not replace it with an
  exists-then-put sequence.
- Use `SmbStorageBackend::probe` for redaction-safe connection and read/write
  validation. Use `tests/samba/run.sh` for the managed protocol and complete
  backup lifecycle suite.
- Azure Blob is still a feature-gated unsupported placeholder. Do not add real Azure SDK code without implementing the shared `BlobStore` provider layer first.
- See `docs/blob-store.md`, `docs/streaming.md`, `docs/backup-integration.md`,
  `docs/native-smb.md`, and `MIGRATION.md` before making provider or
  backup-facing changes. Historical agent handoffs are archived outside this
  crate and are not current guidance.

- The vendored SMB client must verify a session response's signature or
  encryption before evaluating its status (GHSA-439h-2cx8-xwgx). Never treat a
  status from an unverified message as authoritative, and never let a listing
  skip a directory that enumeration reported. Record every vendor change as a
  patch in `vendor/smb/PROVENANCE.json` and `scripts/verify-smb-backport.py`,
  and run `scripts/test-smb-vendor.sh`.

- Bound every server-driven resource: the vendored worker accepts responses
  only for message IDs it sent and holds a bounded number, its receive loop
  ends on transport failure, and listings fail at their depth, entry and time
  limits rather than returning a partial result. Never adopt a changed server
  GUID on reconnect; the pin is checked during negotiation, before session
  setup sends credentials.

- Keep one listing contract on every provider: a prefix is a plain string
  prefix, optionally ending in `/`, and a listing returns only keys
  `validate_blob_key` accepts. Skipping an unaddressable name (not valid
  UTF-8/UTF-16, or refused by `validate_blob_key`) is not a partial listing;
  a directory that enumeration reported but cannot be opened still fails it.
  Run `tests/support/listing_contract.rs` against every provider.
- `validate_blob_key` reserves the internal `.graphql-orm-storage` namespace
  as a key's first component and as a bucket. Internal code addresses it by
  path, never through a caller key. On local roots the reservation is by file
  identity (`LocalStorageBackend::check_root_name`): every local entry point
  that takes a key, prefix or bucket must call it, and writes must call
  `ensure_internal_dir` first. Never replace it with name folding.
- The SMB default minimum dialect is SMB 3.1.1. Never lower the default; a
  lower dialect is an explicit host opt-in.

- Lock release requires explicit single-attempt deletion through
  `delete_blob_once`; never fall back to retrying ordinary deletion after a
  mutation may have reached the server. Ambiguous outcomes require quiescing
  all writers, handles and in-flight requests before offline recovery.
