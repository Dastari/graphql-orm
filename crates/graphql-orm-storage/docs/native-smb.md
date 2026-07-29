# Native SMB2/SMB3 Blob Storage

The `smb` feature provides `SmbStorageBackend`, a direct network `BlobStore`.
It does not use a mapped drive, OS mount, `mount.cifs`, Samba CLI, privileged
container, C library, or FFI.

## Client selection

The backend uses the MIT-licensed pure-Rust `smb` (smb-rs) client. Version
0.10.2 is pinned with compatible protocol/auth crates because 0.11's
release-candidate crypto graph conflicts with this crate's optional AWS SDK.
Re-evaluate the pin when those upstream dependency graphs converge.

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
