---
title: "graphql-orm-backup Agent Guide"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-10
review_by: 2027-02-01
supersedes: []
---

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

- Current crate version is `0.7.0`.
- The optional ORM adapter resolves `graphql-orm` 0.21.0 from the workspace.
  Internal packages use workspace path dependencies and the root `Cargo.lock`.
  Keep downstream applications on one reviewed monorepo revision so ORM,
  backup, and storage share the same canonical source/type universe.
- `graphql-orm` owns its optional `agql-auth` integration and pins
  `agql-auth` 0.14.0 at
  `413fda3435f060604cd653c11e2cc18a668aace1`. This crate must not enable or
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
  `crates/graphql-orm-storage/tests/samba/run.sh` from the workspace root; it
  includes this crate's complete SMB snapshot lifecycle test.
- Read `docs/smb.md` and `MIGRATION.md` before changing provider integration or
  host guidance. Host-specific integration records are archived outside this
  crate and are not reusable guidance.
