---
title: graphql-orm-router agent guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router agent guide

- Keep this crate project-neutral. It must not depend on `graphql-orm`, AI,
  backup, storage, a consuming product, or application types.
- Keep Federation composition, Hive, planner, executor, parser, and `ArcSwap`
  types behind private adapters. Public errors and data must be router-owned.
- Do not expose or initialize Hive JWT, S3, or `object_store` configuration.
  Future durable storage is wired by a higher-level integration through
  `graphql-orm-storage`; this project-neutral crate remains independent of it.
- The router may validate JWT signatures with public keys through an
  engine-neutral provider. It must never accept RSA private keys, sign or issue
  tokens, refresh sessions, perform RSA decryption, or instantiate issuer-side
  `agql-auth` services.
- A candidate graph is immutable and fully composed and constructed before one
  atomic publication. Any failure preserves the complete last-known-good graph.
- Router authorization may deny early but never grants authority. Subgraph
  resolver guards and database policy remain authoritative.
- Do not replace the structural root-ownership compatibility adapter with SDL
  string rewriting. Its exact pinned-version regression tests must pass before
  dependency updates.
- Ordinary in-flight requests remain pinned to their selected graph. Retired
  subscriptions terminate with the documented reload signal and reconnect;
  they never migrate silently.
- Use test-owned loopback services only. Never contact a live application
  database or subgraph from tests.
- Never verify the workspace with `--all-features`. Use explicit package and
  optional-integration lanes.
