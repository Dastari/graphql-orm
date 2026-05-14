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
- S3 and Azure Blob support should be explicit feature-gated work; placeholder paths must return clear unsupported errors until implemented.
- Add tests for path safety, checksums, key generation, and provider round trips.
