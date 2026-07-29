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

- Current crate version is `0.6.0`.
- The optional ORM adapter pins `graphql-orm` 0.16.0 at
  `dd68a001f47f04178bf3389dd47ee952faa6ecf0`. Keep downstream applications in
  the same canonical source/type universe.
- `graphql-orm` owns its optional `agql-auth` integration and pins
  `agql-auth` 0.12.0 at
  `3f3b0c5365adfbe436514a681d977b600991b797`. This crate must not enable or
  depend directly on application authorization.
- Applying and dry-run restore compare the manifest backend/schema hash with
  the target before target checks or writes. Preserve that fail-closed
  preflight.
- Adapter column policy overrides may only strengthen
  `Include -> Redact -> Exclude` and participate in the schema hash.
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
  `/home/toby/dev/graphql-orm-storage/tests/samba/run.sh`; it includes this
  crate's complete SMB snapshot lifecycle test.
- Read `docs/smb.md`, `docs/digitise-native-smb.md`, and `MIGRATION.md` before
  changing provider integration or host guidance.
