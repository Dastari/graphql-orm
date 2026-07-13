# graphql-orm-backup Agent Guide

This crate is a reusable backup and restore companion for applications that use `graphql-orm`.

## Skills

- Use `.agents/skills/rust-skills/SKILL.md` for all Rust implementation, review, refactoring, performance, and API design work.
- Use `.agents/skills/graphql-orm-macros/SKILL.md` for graphql-orm integration decisions.

## Rules

- Keep the crate generic and reusable.
- Do not add Digitise-specific domain names, entity names, collection semantics, accession logic, record logic, media workflows, or policy assumptions.
- Do not store file bytes in a database.
- Prefer traits and small adapters over application-specific coupling.
- Keep provider-specific code behind feature flags.
- Treat restore as a first-class feature. Every backup feature must have restore and verification tests.
- Full backup and restore ship before incremental backup.
- Incremental backup depends on a reliable graphql-orm change journal.

## Current Agent Handoff

- Current crate version is `0.4.0`.
- Native SMB repositories use
  `graphql-orm-storage::SmbStorageBackend -> BlobStoreBackupRepository`; this
  crate must not contain SMB transport code.
- Enable the `smb` feature and construct the backend with runtime credentials.
  Reusable crates never persist those credentials.
- Full backup, referenced-object verification and restore use the streaming
  methods on `BackupRepository`, `BackupObjectIndex`, and `RestoreObjectSink`.
  Preserve their buffered defaults for source compatibility.
- Repository locking depends on atomic
  `BlobStore::put_blob_if_not_exists`. Never implement locking with an
  existence check followed by a write.
- Snapshot manifests and repository key layout are provider-independent and
  unchanged in 0.4.0.
- Run the managed real-Samba suite with
  `/home/toby/graphql-orm-storage/tests/samba/run.sh`; it includes this crate's
  complete SMB snapshot lifecycle test.
- Read `docs/smb.md`, `docs/digitise-native-smb.md`, and `MIGRATION.md` before
  changing provider integration or host guidance.
