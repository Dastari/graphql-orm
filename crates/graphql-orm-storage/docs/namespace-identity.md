---
title: "Namespace identity and capacity-domain facts"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-10-21
review_by: 2027-04-01
supersedes: []
---

# Namespace identity and capacity-domain facts

`BlobStore` exposes two deliberately separate identities. Both are descriptive
provider evidence. Neither is an admission, custody or write-authority decision.

- **Object namespace.** A `NamespaceId` identifies exactly the configured
  root/prefix of a store, or a sub-prefix that has been explicitly marked
  (`NamespaceTarget::SubPrefix`). It lives in a reserved marker object
  (`.graphql-orm-namespace`). It reconciles exact aliases of one namespace across
  restarts and credential rotation.
- **Physical/capacity domain.** `DomainFact`s describe where a root lives
  (`RootLocation`), the store that holds it (`PhysicalStore`) and the quotas on it
  (`QuotaConstraint`). Each fact carries a `FactScope`. A marker never proves that
  two namespaces have distinct capacity.

## Operations

| Method | Side effects |
| --- | --- |
| `observe_namespace(target, scan)` | Read-only. It never creates, repairs or deletes anything. |
| `initialise_namespace(target, proposed)` | Explicit. One create-if-absent of the marker, then a validated read-back. |
| `observe_capacity_domain()` | Read-only. |
| `put_blob_fenced(key, body, options, fence)` | Overwriting write, only while the fence still matches. `UnsupportedOperation` on SMB. |
| `put_blob_if_not_exists_fenced(key, body, options, fence)` | Create-only write while the fence still matches. `Ok(None)` means the key already existed and nothing was written. |

`new`, `connect`, `probe`, listing, reads and capacity paths never write a marker.

`initialise_namespace` returns:
- `Created`;
- `AlreadyInitialisedSame` (reconciles a lost reply);
- `AlreadyInitialisedOther` (never overwritten or adopted);
- `Conflict`.

After an ambiguous failure, retry with the **same** identity. Never mint a
replacement.

## States

| State | Meaning |
| --- | --- |
| `Initialised` | A complete, valid marker was read. |
| `Uninitialised` | No marker and no in-progress publication. |
| `PublicationInProgress` | An interrupted or ongoing local publication. Unknown. |
| `Conflict` | The marker is empty, partial, oversized, foreign or non-canonical. |
| `Unsupported` | The provider cannot store or read markers. |
| `Unknown` | The marker read was denied, timed out or failed. |

## Marker format and reads

- **Format.** The marker body is exactly
  `{"format":"graphql-orm-storage/namespace/v1","namespace_id":"<uuid>"}`, with
  one optional trailing newline. The UUID is lower-case and hyphenated.
  Anything else is `Conflict`: duplicate or extra fields, whitespace variants,
  other formats or trailing data.
- **Reads are bounded.** At most 1 KiB is read, under a 10 s deadline, and size
  hints are never trusted:
  - Local reads with `take`.
  - S3 uses a ranged GET plus a bounded body read.
  - SMB checks the size before allocating.
- **Failures are never absence.** Oversized, denied, timed-out or partial reads
  are never `Uninitialised`.

## Publication and durability

| Provider | Publication | Durability claim |
| --- | --- | --- |
| Local | Write and `fsync` `.graphql-orm-namespace.publishing`, hard-link it to the marker (fails if present), `fsync` the parent directory, then remove the temporary file. | Durable after the directory `fsync` on Unix. Weaker elsewhere. |
| SMB | Exclusive `FILE_CREATE` at the marker name, write every byte, flush, then close. A collision means another writer won, and the result is read back. A crash mid-write leaves a partial marker, which reads as `Conflict`, never as absence. | The server's flush semantics only. |
| S3 | `PutObject` with `If-None-Match: *`. A 412 means another writer won, and the result is read back. | The provider's acknowledgement. |

The ordinary `put_blob_if_not_exists` path is not reused for markers.

## Topology scans

- Scans are opt-in through `TopologyScan`.
- **Ancestor scans** stay inside the provider's authority boundary:
  - Local: the filesystem holding the root (same device).
  - SMB: the share root.
  - S3: the bucket root.
- **Nested scans** are bounded by depth, pages, keys, bytes held, results
  returned and time.
- **Limits are validated, never clamped.** `observe_namespace` rejects a scan
  whose limits exceed `MAX_SCAN_DEPTH` (64), `MAX_SCAN_ITEMS` (pages or keys,
  10 000 000), `MAX_SCAN_BYTES` (64 MiB), `MAX_SCAN_RESULTS` (100 000) or
  `MAX_SCAN_DURATION` (600 s) with `PreconditionFailed`, before any I/O.
- **One deadline per scan.** `max_duration` starts when the scan starts. Every
  directory read, entry, listing page and marker read awaits only the remaining
  time, so a single wide directory or a slow provider still stops on time.
- **Bytes are charged before the read.** Each candidate directory is charged
  its recorded path plus the largest admissible marker read (1 KiB) before the
  marker is read, so no outcome, including `Complete`, exceeds `max_bytes`.
- Reaching any limit reports `Truncated { limit }`, naming the limit.
- Every scan reports a `ScanCompleteness` value: `Complete`, `Truncated`,
  `Denied`, `Unsupported` or `NotRequested`. Only `Complete` with no markers is
  evidence of absence.
- Local scans do not follow symbolic links, and report `Truncated` when they
  meet one.
- A nested scan on SMB is `Unsupported` in this version.

## Reserved names

Every ordinary read, write, copy, delete, streaming and multipart path rejects any
key component that addresses `.graphql-orm-namespace` or
`.graphql-orm-namespace.publishing`. The match follows each provider's key
equivalence:

| Equivalence | Provider | Rule |
| --- | --- | --- |
| `Exact` | S3 | Byte-exact. |
| `CaseFolding` | Local on non-Windows hosts (roots may be case-insensitive) | Case-folded, with trailing dots and spaces trimmed. |
| `Windows` | Local on Windows hosts | As SMB below. |
| `Windows` | SMB | Case-folded and trimmed. Any `:` (alternate data stream) is refused. 8.3 short-name forms such as `GRAPHQ~1`, which may resolve to a reserved name, are refused as unprovable. |

Markers are hidden from ordinary listings. Hiding never turns a partial scan
into a negative. The local temporary-file sweep never enters or removes
anything under a reserved name.

## Domain facts and equality

| Provider | Facts |
| --- | --- |
| Local (Unix) | `RootLocation(dev, ino)` and `PhysicalStore(dev)`, both scoped `HostBoot { boot_id }`. Quotas and a filesystem identity that survives a reboot are reported missing (`Partial`). |
| SMB | `RootLocation(volume_object_id, file_id)` and `PhysicalStore(volume_object_id)`, scoped `StoreIdentity`, when the server reports a volume object ID. Otherwise `RootLocation(volume_serial, file_id)` scoped `Unspecified`, which is never comparable. |
| S3 | `RootLocation(endpoint, bucket, prefix)` scoped `Unspecified`, because endpoints can alias. There are no domain facts (`Unknown`). |

`DomainFact::compare` returns `Same` only for the same category, provider and
complete tuple within a covering scope. A missing value or `Unspecified` scope
gives `Unknown`, never "different". A shared quota principal never joins
different physical stores.

`relate_namespaces` combines identities and locations:

| Relation | Condition |
| --- | --- |
| `SameNamespace` | Same ID at the same location. |
| `Duplicated` | Same ID at a different location: a copied marker, not alias proof. |
| `DifferentNamespaces` | Different IDs. Says nothing about capacity. |
| `Unknown` | Anything else. |

## Fences

Observations carry an `ObservationEpoch`: the backend instance, the connection
generation and a witness of the observed **target** root (not the store root).
A fence binds a target, a namespace ID and that epoch.

Every fenced write first requires the key to lie inside the fence's target, at
a component boundary: for `SubPrefix("a")`, `a/x` is inside, while `a`,
`a-sibling/x` and `b/x` are refused. `Root` contains every key. It then
re-validates the epoch and the marker, and returns a non-retryable
`PreconditionFailed` ("namespace fence invalidated") on any mismatch. Callers
must re-observe.

| Provider | What the fence re-validates | Residual risk |
| --- | --- | --- |
| Local | Backend instance, target directory identity (device and inode on Unix) and the marker. A target replaced by a directory with a copied marker is refused. | Check-then-act: a replacement between the check and the write is not excluded. |
| SMB | Backend instance, connection generation, target directory identity (volume object ID or `none`, volume serial and file ID) and the marker. Every effect of the fenced create (each missing parent directory create and its close, the exclusive file create, writes, flush, publication and cleanup) is issued only on the checked session. The generation is re-checked before each parent create, before the file create and at the publication point. A failure is returned without reconnect, directory-cache repair or replay, and cleanup never reconnects. | Overwriting fenced writes are `UnsupportedOperation`, because a rename cannot be bound to the session. Parent directories created before a failure remain (in the original session). If the session is lost between the exclusive create and arming delete-on-close, an empty object can remain at the key. |
| S3 | Backend instance and the marker only (configuration-only). Both fenced writes send every mutation (single `PutObject`; or multipart create, each part, completion and abort) exactly once, with SDK retries disabled; failures are `SingleAttemptFailed`. A failed multipart lifecycle is aborted once. | Endpoint aliasing, bucket recreation and concurrent marker replacement are undetectable. A lost abort can leave an incomplete multipart upload for the bucket's lifecycle rules. |

`put_blob_if_not_exists_fenced` never overwrites. If it returns `Ok(None)`,
read the existing object and compare it with your own body. If the outcome is
unknown (a retryable or ambiguous error), never resend a different body under
the same key; observe the key first.

No atomic remote exclusion is claimed.
