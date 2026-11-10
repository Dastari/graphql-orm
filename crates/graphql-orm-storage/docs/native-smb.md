---
title: "Native SMB2/SMB3 Blob Storage"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-09
review_by: 2027-02-01
supersedes: []
---

# Native SMB2/SMB3 Blob Storage

The `smb` feature provides `SmbStorageBackend`, a direct network `BlobStore`.
It does not use a mapped drive, OS mount, `mount.cifs`, Samba CLI, privileged
container, C library, or FFI.

## Client selection

The backend uses the MIT-licensed pure-Rust `smb` (smb-rs) client. Storage
0.7.4 owns a direct-path copy of registry version 0.10.2 under `vendor/smb`.
Only the directory-stream runtime hunks from upstream commit
`f4524d032c8951cc288e1c8231ad68588583abda` are backported: retained notification
permits avoid a lost wake, and an idle producer observes receiver closure.
An already in-flight request is not promised immediate cancellation.

Storage 0.8.1 adds one owner-local security patch for
[GHSA-439h-2cx8-xwgx](https://github.com/Dastari/graphql-orm/security/advisories/GHSA-439h-2cx8-xwgx),
recorded as `PATCHES/ghsa-439h-2cx8-xwgx.patch`. It is not an upstream
backport: upstream smb-rs has the same ordering. See
[response verification](#response-verification).
The protocol/authentication companions, cryptographic versions and feature
selection remain pinned to the existing 0.10.2 family; this is not an adoption
of the newer SDK's crypto graph.

[Provenance](../vendor/smb/PROVENANCE.json) records the registry archive checksum,
original file hashes, the immutable backport and local patches, and the
MIT-license source. `python3 scripts/verify-smb-backport.py --archive <smb-0.10.2.crate>`
reverses both patches offline and compares the result with the archive.
`scripts/test-smb-vendor.sh` runs the vendored unit tests, including the
regression tests for the local patch, on a temporary copy with the archived
lockfile under `--locked`. CI runs both. The exact upstream
license is retained as [LICENSE](../vendor/smb/LICENSE). The archive's original
`Cargo.lock` is preserved as `Cargo.lock.upstream` for provenance only; the
workspace lockfile governs builds. The six original upstream Markdown files
remain verbatim under the narrow documentation exception in
[ADR-0012](../../../docs/decisions/ADR-0012-owner-local-third-party-source-documentation.md).
The root exclusion and the dependency manifest's empty `[workspace]` isolate
this third-party package; it does not add a tenth workspace member. Cargo 1.97
treats literal parent members as overriding nested exclusions, so the root uses
the exact-set member pattern `crates/graphql-orm-storag[e]`: it matches the same
single storage directory while allowing the vendor exclusion to take effect.

This workspace is consumed from Git: Cargo resolves the direct relative path
inside the same reviewed Git source, without a root-only `[patch]` requirement.
A future crates.io packaging flow needs a separate dependency review; do not let
Cargo's registry fallback silently replace this backport with unpatched 0.10.2.

`cargo audit` does not audit non-default-registry dependencies, including
workspace path dependencies and downstream Git-origin dependencies. Preserve normal selected-graph
audits, and separately look up the original registry `smb` 0.10.2 identity and
archive checksum against the **same frozen advisory database**. Retain that raw
result and explicitly assess each advisory against the backported source and
selected features. Neither this supplemental lookup nor the two-hunk patch is a
clean audit result or an advisory waiver. Re-run this review for any vendor update.

A future update must review the archive, patch applicability, license and selected
dependency graph again. Remove this backport when a compatible maintained
upstream release contains the fix and passes the owned consumer and Samba lanes;
do not silently substitute an unpatched registry dependency.

It supplies SMB 2.0.2 through 3.1.1 (never SMB1), pure-Rust NTLM credentials
with optional domain/workgroup, signing, SMB3 encryption, explicit
`FILE_CREATE`, flush/close, rename, recursive enumeration, delete, async
requests, and SMB credit concurrency. Kerberos, SMB1, guest/anonymous sessions,
QUIC, RDMA, and application-managed DFS namespaces are outside this backend's
supported contract.

## Configuration

```rust,no_run
use std::time::Duration;
use graphql_orm_storage::{SmbDialect, SmbStorageBackend, SmbStorageConfig};
use secrecy::SecretString;

# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
let mut config = SmbStorageConfig::new(
    "files.example.org",
    "backups",
    "backup-service",
    SecretString::from("runtime secret"),
);
config.domain = Some("EXAMPLE".to_string());
config.root_prefix = Some("repositories/production".to_string());
config.min_dialect = SmbDialect::Smb3_0;
config.require_encryption = true;
config.connect_timeout = Duration::from_secs(10);
config.operation_timeout = Duration::from_secs(60);
let store = SmbStorageBackend::connect(config).await?;
# Ok(())
# }
```

Port 445, SMB 3.0 minimum, and required signing are defaults. Encryption may be
required. Guest fallback is refused. Server/share fields are names, not local
paths, UNC paths, or SMB URLs.

`root_prefix` and blob keys are `/`-separated relative keys. Empty segments,
`.`, `..`, absolute paths, backslashes, NUL, and platform prefixes are rejected.
Passwords use `SecretString`, are redacted from `Debug`, and never enter errors
or probe results. Hosts own encrypted persistence, rotation, authorization, and
audit; this crate does not serialize credentials.

## Response verification

The vendored client verifies a session response's signature or encryption
before it evaluates the response status. A message that is not signed when
signing is required, or not encrypted when encryption is required, is rejected
with the client's typed `UnverifiedMessage` error, reported as a
`SecurityNegotiation` error, and its status is never acted on. Before 0.8.1, the
status was checked first, so an on-path party without credentials could answer
a pending request with an unsigned error such as `STATUS_OBJECT_NAME_NOT_FOUND`,
even with `require_encryption = true`.

Only the MS-SMB2 3.2.5.1.3 exemptions remain, and only for a status the caller
does not accept, so an unsigned response can only fail a request: an interim
`STATUS_PENDING`, and a `SESSION_SETUP` response before session setup completes.
A rejected response leaves its request's outcome unknown, so the error is not
retryable; verify the affected key before relying on it.

Interim `STATUS_PENDING` responses are consumed before session verification,
so they are treated as untrusted. Responses are delivered by message ID, and an
interim's header session ID is not authenticated, so each interim is checked
against the session the request was sent on. An interim is discarded if it
names another session, if the request's session is unknown to the connection,
or if it is unencrypted while that session requires encryption. On a
signing-only session an unsigned interim for the request's session is
accepted, as the specification allows, but it can only extend the wait: it
never completes the request, and a final response with a different async ID, or
a synchronous one, is still delivered and verified. Forged interims can
therefore delay a request until `operation_timeout`, but not change its
outcome. On a signing-only session the async ID used to cancel a request is the
latest accepted interim's, which an on-path party can overwrite; this backend
does not cancel requests.

Because guest and anonymous sessions are refused, a not-found status is
authenticated. `blob_exists` returning `false`, `head_blob` returning `None`,
and deletion of an absent key returning `Ok` therefore reflect the server.

Listing fails closed. Only a missing configured root lists as empty. If a
directory reported by enumeration is not found, or does not open as a
directory, the listing fails with a non-retryable `Protocol` error instead of
silently omitting its subtree. This backend never removes directories, so this
indicates outside modification of the share.

## Atomicity and streaming

Normal writes create parents, stream to a UUID temporary file in the same
remote directory, flush, close, and rename over the final key. Failures attempt
to remove that unique temporary file, and listings filter its strict name form.

Conditional writes issue SMB CREATE with `FILE_CREATE` directly;
`STATUS_OBJECT_NAME_COLLISION` becomes `Ok(None)`. There is no existence-check
race. This primitive backs repository locks and content-addressed deduplication.
Rename atomicity ultimately depends on the server filesystem, but the Samba
suite exercises same-directory overwrite rename and concurrent CREATE.

Uploads consume `StorageByteStream` incrementally and hash while writing.
An input item may be arbitrarily large: the backend splits it without collecting
the body. SMB2 `MaxWriteSize` is the WRITE payload limit and excludes protocol,
signing, and encryption framing, so no framing bytes are subtracted. For
interoperability, the backend additionally caps each request at 1 MiB and rounds
larger limits down to a 64 KiB SMB2 credit boundary. It handles partial
acknowledgements by advancing both the slice and file offset, and rejects a
zero-byte acknowledgement for a non-empty request.

New files are marked delete-on-close until the complete body is hashed and
flushed. Cancellation guards close and remove owned direct or temporary files.
Cleanup ownership starts only after a successful CREATE response, so a
conditional-create collision can never delete the pre-existing object. A
temporary file remains guarded through publication; cancellation during rename
removes only the unique temporary name, never a successfully published target.

Parent directories use a bounded 4,096-entry per-backend cache and a per-path
single-flight coordinator. The first object opens or creates each ancestor;
later objects under a known parent issue no directory CREATE requests. A
successful reconnect or path-not-found response clears cached directory facts
before safe reconstruction.

Downloads use fixed chunks and retain a bounded transfer permit. Connection and
operation deadlines are separate. Retryable timeouts and connection failures
replace the shared client by generation, so later operations do not inherit a
poisoned smb-rs worker. Directory creation, opens, listings, deletion, and the
CREATE of a unique temporary name may be retried because they are idempotent.
Stream writes are not replayed. An exclusive `FILE_CREATE` is not replayed after
an ambiguous response because the server may already have created that exact
target. Callers retry failed uploads with a fresh stream.

If a response is lost after conditional CREATE, the server may hold a partial
or complete target. Cleanup is best effort, so completed snapshots must still
be verified. Backup manifests are written last and do not advertise incomplete
payload sets.

## Probe and troubleshooting

`SmbStorageBackend::probe` connects, negotiates, authenticates, opens the share,
optionally creates the prefix, lists it, round-trips random bytes, compares, and
deletes the probe. Its result reports the negotiated dialect, signing and
encryption state, maximum read/write/transact sizes, and the effective bounded
WRITE payload, in addition to reachability and read/write facts.

`SmbStorageBackend::diagnostics` returns a redaction-safe snapshot suitable for
polling during long backups. It includes active/completed/failed uploads,
acknowledged bytes, WRITE and partial-response counts, reconnect generations,
directory cache hits, remote directory CREATE requests, and coalesced waits.
With the `smb` feature, structured `tracing` events also report connection,
reconnect, upload start/progress/completion, and failure events. These surfaces
do not include passwords, usernames, endpoints, or configuration debug output.

Use `StorageProviderErrorKind`, which distinguishes connectivity,
authentication, security negotiation, missing share, permission, missing path,
capacity, collision, timeout, connection loss, and protocol failures. Never put
runtime configuration or credentials in support bundles.

## Managed Samba tests

Run `crates/graphql-orm-storage/tests/samba/run.sh` from the workspace root for
the owned Samba protocol and complete backup lifecycle suite. It uses `cargo`
by default. Set `SMB_TEST_CARGO` to an executable path to run all nine unchanged
Cargo/test invocations through a host's resource-controlled Cargo runner:

```sh
SMB_TEST_CARGO=/absolute/path/to/run-cargo-bounded.sh \
  crates/graphql-orm-storage/tests/samba/run.sh
```

The setting accepts one executable, including a path containing spaces, rather
than a shell command with arguments. The host supplies the runner's toolchain
and resource environment; the suite retains its owned fixture lifecycle and
cleanup.

## Manual compatibility plan

For each target, create/verify/restore/delete/prune a full backup with a
multi-gigabyte object, interrupt one transfer, and race two lock acquisitions:

- Windows Server 2022/2025 and a Windows 11-hosted share;
- current Samba with workgroup, signing, and encryption modes;
- a common NAS such as Synology DSM or QNAP QTS with SMB3 enabled;
- Linux, Windows, and macOS hosts; and
- x86-64 and ARM64 where deployed.

Record dialect, security mode, filesystem, reconnect behavior, and flush/rename
limitations without recording usernames or secrets.

## Lock release

Backup lock release uses `BlobStore::delete_blob_once`: the handle is opened,
disposition is attempted and the same handle is closed, without reopening the
path after either mutation step may have succeeded. Ordinary blob deletion
retains its reconnect/retry behavior. A timeout or lost response during lock
release leaves its outcome unknown; do not retry deletion by key. Offline lock
recovery requires quiescing all writers, handles and in-flight requests.

Conditional-create cleanup is handle-only too. Its guard retains no shared key
for fallback deletion after closing the original handle, including failure and
Drop paths. An ambiguous arm/disarm or close can leave an orphan lock for offline
recovery. Unique temporary-upload cleanup retains its ordinary by-key behavior.
