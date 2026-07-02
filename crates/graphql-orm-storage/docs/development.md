# Development

This repository is a single Rust crate.

## Common Checks

Run the default provider tests:

```bash
cargo test
```

Run the full feature matrix:

```bash
cargo fmt --check
cargo test --all-features
cargo test --no-default-features
cargo test --features s3,azure --no-default-features
cargo check --features s3,azure --no-default-features
cargo clippy --all-features --all-targets -- -D warnings
cargo clippy --no-default-features --lib -- -D warnings
```

Build docs with warnings denied:

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
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
cargo test --features s3 --no-default-features --test s3_integration
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
