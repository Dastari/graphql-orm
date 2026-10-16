---
title: "Development"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-10-15
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
cargo clippy -p graphql-orm-storage --lib --no-default-features --features s3 -- -D warnings
cargo clippy -p graphql-orm-storage --all-targets --features s3,smb,azure -- -D warnings
```

Build docs with warnings denied:

```bash
RUSTDOCFLAGS="-D warnings" cargo doc -p graphql-orm-storage --no-deps
```

## S3 Integration Tests

The real-service round trip is opt-in: it returns without contacting a service
unless `S3_TEST_ENDPOINT` and `S3_TEST_BUCKET` are set. The release-replay
regression always creates an owned loopback HTTP listener and uses fixture
credentials; it never reads application/cloud credentials.

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

## Managed SMB fixture ownership

`tests/samba/run.sh` runs the seven managed protocol, cancellation, encryption,
constrained-write, backup-lifecycle and reconnect cases, followed by two SDK
directory-stream regressions. It creates fresh randomly
named containers and a fresh labelled volume, binds dynamic ports only on
`127.0.0.1`, and passes the exact normal container ID to the restart test. It never
reuses or deletes a pre-existing fixed-name fixture. Cleanup selects only its
random run label, rechecks ownership and removes exact container IDs plus its
owned volume; failures and interrupts use the same cleanup path. The runner
selects unused OS ports and publishes each port explicitly so restarts preserve
the mapping; a competing bind fails startup instead of reusing another service.
The restart test requires a full immutable container ID and explicit run ID from
the harness, and verifies the container's run label before any mutation. Missing
or mismatched ownership fails closed, including manually invoked ignored tests.
After restart the test verifies the original mapping and waits at most 30 seconds
for TCP readiness before exercising the same existing backend's reconnect path.

The additional `smb_directory_stream` tests use the actual SDK's
`Directory::query_with_options` on that same owned normal share. A small response
buffer and exact long filenames establish multibatch enumeration through EOF,
with no duplicates. A separate test observes a real entry, drops the stream and
caller ownership, and bounds eventual release of the directory `Arc` through a
`Weak` reference polled every 10 ms. This is drop-liveness coverage: the public SDK
does not expose producer idle state, so the test does not claim deterministic
reproduction of the lost-wake schedule or immediate server-side close. Both use
a current-thread runtime and attempt task-owned path cleanup before reporting
Result errors; unexpected panic or process termination uses the runner's owned
container and volume cleanup fallback.

The daemon-free ownership regression suite substitutes private fake Docker/Cargo
executables and checks startup collisions, exact-ID cleanup, ownership changes,
a lost create response, failed temporary-file inspection and missing native
test matches. Every selected Cargo command must execute exactly its expected
ignored test and report one pass; a zero-match exit or unrelated test is not
coverage. Restart diagnostics may split the native test-start line without
invalidating a successful completion. Docker or remote `find` failure during
temporary-file inspection fails the runner and preserves its error status; it
is never treated as a clean share. These runner evidence checks do not change
the library API or provider behavior. Run it with:

```bash
python3 crates/graphql-orm-storage/tests/samba/test_run.py
```

Local execution defaults to `SMB_TEST_PULL_POLICY=never`: provision and review a
cached image beforehand. `SMB_TEST_IMAGE` selects the reviewed image reference.
Disposable hosted CI explicitly uses `SMB_TEST_PULL_POLICY=missing`. The script
does not change Docker daemon or application resources. Under a host resource
wrapper, keep setup, all tests and cleanup in one admitted invocation; its Docker
shim remains responsible for resource limits and may require cached images even
when the harness permits pulling missing images.

## Owned HTTPS transport fixture

The [HTTPS fixture test](../tests/s3_https_transport.rs) uses OpenSSL and Python
only for test-owned infrastructure. It creates temporary CA/server certificates,
starts the [loopback-only server](../tests/s3/owned_https_fixture.py) on an assigned
port, and verifies both a trusted CA and rejection of an unrelated CA. Each
client runs in a fresh child process with a private `SSL_CERT_FILE`/`SSL_CERT_DIR`;
the parent test environment and system certificate store are unchanged. Child
processes and temporary files are cleaned up by the test's ownership guards.

Run it explicitly on a host with `openssl` and `python3`:

```bash
cargo test -p graphql-orm-storage --no-default-features --features s3 \
  --test s3_https_transport -- --ignored --exact owned_https_transport_fixture
```

The child entry point is an implementation detail; invoke the parent fixture
above. No Docker, cloud account, external endpoint or application secret is used.
The regular HTTP loopback regression runs with `--test s3_integration`.

The all-targets provider Clippy command above enables local defaults and Azure
so its local example and Azure placeholder test helpers are compiled in their
supported configurations. Use the library-only command for an isolated S3 lane.

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
