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
| `put_blob_fenced(key, body, options, fence)` | Writes only while the fence still matches. |

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
| SMB | `FILE_CREATE` at the marker name, held delete-on-close until flushed, then close. | The server's flush semantics only. |
| S3 | `PutObject` with `If-None-Match: *`. A 412 means another writer won, and the result is read back. | The provider's acknowledgement. |

The ordinary `put_blob_if_not_exists` path is not reused for markers.

## Topology scans

- Scans are opt-in through `TopologyScan`.
- **Ancestor scans** stay inside the provider's authority boundary:
  - Local: the filesystem holding the root (same device).
  - SMB: the share root.
  - S3: the bucket root.
- **Nested scans** are bounded by depth, pages, keys and time.
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
| `CaseFolding` | Local (roots may be case-insensitive) | Case-folded, with trailing dots and spaces trimmed. |
| `Windows` | SMB | Case-folded and trimmed. Any `:` (alternate data stream) is refused. 8.3 short-name forms such as `GRAPHQ~1`, which may resolve to a reserved name, are refused as unprovable. |

Markers are hidden from ordinary listings. Hiding never turns a partial scan
into a negative.

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
generation and a root witness. `put_blob_fenced` re-validates before writing and
returns a non-retryable `PreconditionFailed` ("namespace fence invalidated").
Callers must re-observe.

| Provider | What the fence re-validates | Residual risk |
| --- | --- | --- |
| Local | Root device/inode and the marker. | A root replacement between the check and the write is not excluded. |
| SMB | The reconnect generation, root file identity and the marker. | A reconnect after the check is not excluded. |
| S3 | The marker only (configuration-only). | Endpoint aliasing and bucket recreation are undetectable. |

No atomic remote exclusion is claimed.
