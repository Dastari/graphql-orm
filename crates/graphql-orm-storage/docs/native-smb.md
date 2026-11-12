---
title: "Native SMB2/SMB3 Blob Storage"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-12
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

Storage 0.9.0 adds a second owner-local patch,
`PATCHES/resource-bounds-and-server-identity.patch`, applied after the first. It
bounds the client state a server can make the worker hold, closes a connection
whose transport failed instead of retrying it in a loop, refuses a directory
reply larger than the buffer it requested, adds an optional server-GUID pin to
`ConnectionConfig`, and wraps the password in its zeroizing secret before any
fallible step. See [server identity](#server-identity) and
[resource bounds](#resource-bounds).

Storage 0.10.0 adds a third owner-local patch,
`PATCHES/server-response-errors.patch`. Four server-reachable panics become
typed errors: a transform nested inside a decrypted or decompressed payload
(`InvalidMessage`), an async response header without an async ID
(`InvalidMessage`), a file opened on a printer share (`UnsupportedOperation`;
dropping the handle closes it), and a session key shorter than 16 bytes
(`InvalidState`). MS-SMB2 would right-pad a short session key with zeros; the
client fails closed instead of deriving signing and encryption keys from it.
The protocol/authentication companions, cryptographic versions and feature
selection remain pinned to the existing 0.10.2 family; this is not an adoption
of the newer SDK's crypto graph.

[Provenance](../vendor/smb/PROVENANCE.json) records the registry archive checksum,
original file hashes, the immutable backport and local patches, and the
MIT-license source. `python3 scripts/verify-smb-backport.py --archive <smb-0.10.2.crate>`
reverses every patch offline and compares the result with the archive.
`scripts/test-smb-vendor.sh` runs the vendored unit tests, including the
regression tests for the local patches, on a temporary copy with the archived
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
supported contract. Optional Kerberos support is tracked in
[#585](https://github.com/Dastari/graphql-orm/issues/585).

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
// SMB 3.1.1 is the default minimum. Lower it only for a server that cannot
// negotiate SMB 3.1.1; see "Dialect" below.
config.min_dialect = SmbDialect::Smb3_1_1;
config.require_encryption = true;
config.connect_timeout = Duration::from_secs(10);
config.operation_timeout = Duration::from_secs(60);
let store = SmbStorageBackend::connect(config).await?;
# Ok(())
# }
```

Port 445, SMB 3.1.1 minimum, and required signing are defaults. Encryption may
be required. Guest fallback is refused. Server/share fields are names, not local
paths, UNC paths, or SMB URLs.

`root_prefix` and blob keys are `/`-separated relative keys. Empty segments,
`.`, `..`, absolute paths, backslashes, NUL, platform prefixes, the reserved
`.graphql-orm-storage` namespace and `:` are rejected. Windows servers, and
Samba with `streams_xattr`, treat `:` as an alternate data stream separator,
so `a:b` would open stream `b` of the file `a`; listings skip names with `:`.
Passwords use `SecretString`, are redacted from `Debug`, and never enter errors
or probe results. The backend moves the password straight into the SMB
client's zeroizing secret, and no plain copy outlives the connection call.
Hosts own encrypted persistence, rotation, authorization, and audit; this crate
does not serialize credentials.

### Dialect

The client offers only the dialects from `min_dialect` up, and a connection
whose server selects a lower dialect fails with a non-retryable
`SecurityNegotiation` error before session setup. The vendored client's
negotiation check enforces this and has its own regression test; the
backend's check of the negotiated dialect after connecting is only a backstop.
A server that supports none of the offered dialects refuses the negotiation
itself, which the backend reports as a non-retryable `Protocol` error. Either
way, no credentials are sent.

The default minimum is SMB 3.1.1, the only dialect whose negotiation is bound
to the session by pre-authentication integrity. The vendored client does not
send `FSCTL_VALIDATE_NEGOTIATE_INFO`, so with a lower minimum an on-path party
could make a server that supports SMB 3.1.1 negotiate an earlier offered
dialect undetected. Signing stays mandatory and required encryption is still
enforced at any dialect, but SMB 3.0 and 3.0.2 offer only AES-CCM encryption
and no opportunistic encryption. Set `min_dialect` to `SmbDialect::Smb3_0` or
`Smb3_0_2` only for a server that cannot negotiate SMB 3.1.1, and prefer
`require_encryption` there.

## Authentication

Authentication is NTLM only (`ntlm,!kerberos,!pku2u`). NTLM does not
authenticate the server before the client answers its challenge. A party that
answers for the configured server name cannot complete the session, because the
final `SESSION_SETUP` response must be signed with a key derived from the
password. It has, however, received the NTLM challenge response, which can be
attacked offline to recover the password or relayed to another service.

Operators should therefore:

- use a dedicated, single-purpose service account with access only to the
  backup share;
- give it a long, randomly generated password that is not used anywhere else,
  so an offline attack is impractical, and rotate it if exposure is suspected;
- restrict the network path to the file server, and require SMB signing on
  every other service that accepts the account, so a relayed response is
  rejected; and
- set [`expected_server_guid`](#server-identity) so a changed server is
  refused before authentication.

## Server identity

Every connection negotiates first and reads the server GUID before session
setup sends the NTLM response. The first successful connection pins that GUID,
or the operator pins it in advance with `SmbStorageConfig::expected_server_guid`;
`SmbProbeResult::server_guid` reports the value to record. The pin is enforced
inside the vendored client's negotiation, for every connection it makes,
including the connection made after DNS is resolved again on reconnect.

A server reporting another GUID is refused with a non-retryable
`SecurityNegotiation` error, and only the `NEGOTIATE` request has been sent to
it. The reconnect stops at the first refusal instead of trying again. A later
operation that needs a connection negotiates again and is refused the same way,
without sending credentials. The backend never adopts the new identity: replacing or rebuilding the server
requires constructing a new backend, with a new pin, after the operator has
confirmed the change.

The server GUID is not secret and is not authenticated during negotiation. For
example, Samba derives it from the server's NetBIOS name. The pin detects a
different server answering for the configured name, such as a failover or
redirection to another host, and stops the credential exchange from being
repeated against it on every transient failure. It does not stop an impostor
that has observed and copies the real server's GUID; only server
authentication, such as Kerberos, does that. A cluster whose nodes report
different GUIDs under one name will refuse reconnects to another node; pin one
node's name instead.

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

## Resource bounds

A server, or a party that can reach the client's socket, controls what the
client receives. The backend bounds the resources that input can consume:

| Bound | Default | Configuration | When exceeded |
| --- | --- | --- | --- |
| Listing depth below the root prefix | 64 | `max_listing_depth` | Non-retryable `Protocol` error |
| Entries one listing enumerates | 1,000,000 | `max_listing_entries` | Non-retryable `Protocol` error |
| Bytes of names and paths one listing accumulates | 256 MiB | `max_listing_bytes` | Non-retryable `Protocol` error |
| Duration of one complete listing, including its retry | 15 minutes | `listing_timeout` | Non-retryable `Timeout` error |
| Length of one listed name | 255 UTF-16 code units | Fixed (MS-FSCC component limit) | Non-retryable `Protocol` error |
| Length of one key, written or listed | 4,096 bytes | Fixed | Write: `InvalidStorageKey`; listing: non-retryable `Protocol` error |
| Length of one key component, written | 255 UTF-16 code units | Fixed (MS-FSCC component limit) | `InvalidStorageKey` |
| One `QUERY_DIRECTORY` reply | The requested output buffer | Fixed in the vendored client | Non-retryable `Protocol` error |
| Pages in one `list_blobs_page` continuation chain | 1,024 pages and `max_listing_entries` keys | Fixed; entries follow `max_listing_entries` | Non-retryable `Protocol` error |
| Request message IDs awaiting a response | 8,192 | Fixed in the vendored client | Oldest is forgotten |
| Responses held before a task receives them | 256 | Fixed in the vendored client | Oldest is dropped |
| Probe file read | 16 bytes | Fixed | Non-retryable `Protocol` error |

Listings decode each name strictly. A name that is not valid UTF-16, or that
`validate_blob_key` refuses, such as the reserved `.graphql-orm-storage`
namespace at the root, cannot be a key: it is counted against the limits and
skipped, and the listing does not enter such a directory. Every key below it
would contain the same component. Decoding such names lossily would let two
distinct names merge into one key that opens neither file. This is not a
partial listing: every name a key can address is still listed, and a
directory that enumeration reported but that fails to open still fails the
listing.

A listing that reaches a limit fails instead of returning a partial listing,
because callers such as backup pruning treat a listing as complete. The depth
limit stops a self-nesting directory, such as a server-side link to an
ancestor, which would otherwise be listed forever; `operation_timeout` still
bounds each request. Raise the limits for repositories that legitimately
exceed them.

The byte budget counts the text of every name enumerated and every path the
listing builds, plus a fixed 64-byte allowance per entry for its allocation,
and it is never released during the listing, so it overestimates what is held.
Without it, deep directories with long names followed by many short names in
the deepest one would repeat the long parent path in every key within the
entry and depth limits. Names longer than a valid component and keys longer
than 4,096 bytes fail the listing; the backend also refuses to write such a key,
or a key with a component longer than 255 UTF-16 code units, so everything it
writes can be listed back. The limits apply to the caller's key: `put_blob`
writes first to a temporary `.<uuid>.uploading` name beside the target, which
may be up to 48 bytes longer than a key at the limit, and listings allow an
interrupted upload's temporary file that extra length before filtering it out.
The temporary name does not include the target's name, so it fits the
255-unit component limit for every valid name, and its random v4 UUID keeps it
unique. Temporary files left by earlier versions, named
`.<name>.<uuid>.uploading`, are still recognised and filtered. Only these two
dot-prefixed forms, with a lowercase hyphenated UUID, are temporary: a key such
as `report.<uuid>.uploading` is listed like any other, and writing a key whose
name has a temporary form fails with `InvalidStorageKey`. The backend does not
sweep interrupted uploads' temporary files. A server may
refuse a long path itself; Samba on Linux, for example, rejects paths near
4,096 bytes with `STATUS_OBJECT_NAME_INVALID`.

The vendored client refuses a `QUERY_DIRECTORY` reply larger than the output
buffer it requested before decoding its entries, so one reply cannot expand
into more entries than the budget counts. The reply's bytes are still received
in full first, within the transport's 16 MiB frame limit. When a listing stops
at a limit, the backend closes the open directory handle before returning the
error.

`listing_timeout` is one deadline for the whole listing. If an attempt fails
with a retryable error, the backend reconnects and lists once more, and the
reconnect and the second attempt share the same deadline.

`list_blobs` lists the tree once and filters it by prefix. `list_blobs_page`
must list the whole tree for every page, because SMB has no ordered,
resumable enumeration, so prefer `list_blobs`. Its continuation tokens are
opaque, begin with `smb1:`, and carry the page and key counts of their chain:
a chain fails after 1,024 pages or after returning more than
`max_listing_entries` keys in total, so a server that adds names between
pages cannot keep a paged listing going forever. A token this backend did not
issue is rejected with `PreconditionFailed`. A malformed token, or one whose
chain has used its 1,024 pages, is refused before the tree is listed.

The worker accepts a response only for a message ID this client sent and that
has not been answered. A response to any other ID, including one sent before
authentication, is discarded without being stored. An interim
`STATUS_PENDING` keeps its ID outstanding for the final response.

The worker closes the connection when its transport fails, for example after
a connection reset, instead of retrying the failed socket in a loop, and after
64 consecutive messages that fail to process. Requests waiting on that
connection fail at once with `ConnectionLost`, which triggers the bounded
reconnect.

Connections do not follow DFS referrals, so the client connects only to the
configured server, and server-to-client notifications are disabled. The backend
takes no leases or oplocks that need them.

`probe` checks that the declared size of the probe file equals the 16 bytes it
wrote and reads no more than that. `collect_storage_stream` preallocates at
most 8 MiB from a size hint; it still collects the whole body, so use it only
for bodies whose size the caller bounds.

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

Opening an existing key whose file another handle is deleting returns
`STATUS_DELETE_PENDING`. For `get_blob`, `blob_exists`, `head_blob` and
deletion the backend reports that key as missing: the file disappears once its
last handle closes, so it is already gone for readers, and deleting it again
succeeds as it would once the delete completes. This happens, for example,
right after a cancelled `put_blob_if_not_exists` cleans up through
delete-on-close. Creating such a key keeps reporting the status as a
non-retryable `Protocol` error, never as a conditional-create conflict.

If a response is lost after conditional CREATE, the server may hold a partial
or complete target. Cleanup is best effort, so completed snapshots must still
be verified. Backup manifests are written last and do not advertise incomplete
payload sets.

## Probe and troubleshooting

`SmbStorageBackend::probe` connects, negotiates, authenticates, opens the share,
optionally creates the prefix, lists it, round-trips random bytes, compares, and
deletes the probe. Its result reports the negotiated dialect, signing and
encryption state, maximum read/write/transact sizes, the effective bounded
WRITE payload and the server GUID, in addition to reachability and read/write
facts.

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
by default. Set `SMB_TEST_CARGO` to an executable path to run all thirteen
Cargo/test invocations through a host's resource-controlled Cargo runner:

```sh
SMB_TEST_CARGO=/absolute/path/to/run-cargo-bounded.sh \
  crates/graphql-orm-storage/tests/samba/run.sh
```

The setting accepts one executable, including a path containing spaces, rather
than a shell command with arguments. The host supplies the runner's toolchain
and resource environment; the suite retains its owned fixture lifecycle and
cleanup.

The constrained-write server also caps its dialect at SMB 3.0
(`server max protocol = SMB3_00`), so the suite checks that the default
minimum refuses it and that an explicit `SmbDialect::Smb3_0` opt-in connects.
The suite also runs the provider listing contract shared with the local and
S3 tests.

The fixture image defaults to the reviewed index digest
`dperson/samba@sha256:66088b78a19810dd1457a8f39340e95e663c728083efa5fe7dc0d40b2478e869`.
Local runs never pull (`SMB_TEST_PULL_POLICY=never`), so that exact reference
must already be cached; disposable CI sets `SMB_TEST_PULL_POLICY=missing`.
`SMB_TEST_IMAGE` may select another image only by digest
(`name@sha256:<64 hex digits>`); a tag reference is refused with exit status
64 before any Docker call. Changing the default digest is a reviewed test
change recorded in the changelog.

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
