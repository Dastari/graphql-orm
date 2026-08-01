---
title: "Development"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Development

`graphql-orm-storage` is a package in the GraphQL ORM workspace. Internal
companion dependencies use workspace path dependencies and the root
`Cargo.lock`; do not introduce internal Git dependencies.

## Common Checks

Run the default local-provider tests:

```bash
cargo test -p graphql-orm-storage
```

Run explicit provider lanes. Do not use workspace `--all-features`:

```bash
cargo fmt --all -- --check
cargo test -p graphql-orm-storage --no-default-features
cargo test -p graphql-orm-storage --no-default-features --features s3
cargo check -p graphql-orm-storage --no-default-features --features azure
cargo check -p graphql-orm-storage --no-default-features --features smb
cargo clippy -p graphql-orm-storage --all-targets -- -D warnings
cargo clippy -p graphql-orm-storage --all-targets --no-default-features --features s3 -- -D warnings
```

Build docs with warnings denied:

```bash
RUSTDOCFLAGS="-D warnings" cargo doc -p graphql-orm-storage --no-deps
```

## S3 Integration Tests

S3 integration tests are opt-in. They compile with the `s3` feature but return
without touching the network unless `S3_TEST_ENDPOINT` and `S3_TEST_BUCKET` are
set.

Example MinIO environment:

```bash
S3_TEST_ENDPOINT=http://127.0.0.1:9000 \
S3_TEST_BUCKET=graphql-orm-storage-test \
S3_TEST_REGION=us-east-1 \
S3_TEST_ACCESS_KEY=minioadmin \
S3_TEST_SECRET_KEY=minioadmin \
S3_TEST_PATH_STYLE=true \
cargo test -p graphql-orm-storage --features s3 --no-default-features --test s3_integration
```

Use a dedicated throwaway bucket or prefix. The test writes and deletes objects
under a generated prefix.

## Documentation

The root `README.md` should stay short. Long-form material belongs in `docs/`
and should be linked from the README or `docs/README.md`.

Public Rust APIs should have rustdoc comments. Public fallible functions should
include a `# Errors` section.

## Versioning

When public APIs or documentation examples change, update:

- `Cargo.toml`
- `Cargo.lock`
- README/docs snippets that show a concrete crate version
