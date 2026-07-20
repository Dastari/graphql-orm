# Changelog

## 0.6.0

- Native SMB writes now split every incoming stream chunk into requests no
  larger than the negotiated `MaxWriteSize`, with a conservative 1 MiB cap and
  64 KiB SMB2 credit alignment. Partial acknowledgements advance exact offsets;
  zero-byte acknowledgements fail explicitly; the stream remains backpressured
  and is never collected.
- Parent-directory creation is now single-flight per path and backed by a
  bounded per-backend cache. Reconnects and path-not-found responses invalidate
  the cache, eliminating repeated ancestor `OpenIf` calls for shared object
  prefixes.
- Transient failures now replace the shared client by generation and retry only
  idempotent connection, directory, open, listing, deletion, and unique-temp
  creation operations. Conditional `FILE_CREATE` is never replayed after an
  ambiguous response.
- Direct and temporary writes use delete-on-close ownership plus cancellation
  guards. A conditional-create collision never arms cleanup and therefore never
  deletes the pre-existing winner.
- `SmbProbeResult` now reports negotiated dialect, signing/encryption state,
  maximum read/write/transaction sizes, and the effective write-request limit.
- Added redaction-safe `SmbStorageBackend::diagnostics` counters and structured
  `tracing` events for connection generations, upload progress, failures,
  directory coordination, and reconnects.
- Added unit and managed-Samba coverage for oversized chunks, constrained
  negotiated limits, partial and zero-byte writes, exact checksums,
  cancellation cleanup, conditional/overwrite behavior, reconnect cache
  invalidation, concurrency 1/2/4/8, and the shared-parent performance
  regression.

## 0.5.0

- Added the feature-gated native `SmbStorageBackend` using a pure-Rust
  SMB2/SMB3 client. The provider supports streamed reads and writes, safe
  temporary-file publication, atomic conditional creation, signing and
  encryption requirements, reconnect handling, and redaction-safe probing.
- Added `SmbStorageConfig`, `SmbDialect`, `SmbProbeOptions`, and
  `SmbProbeResult` behind the `smb` feature.
- Added `StorageProviderErrorKind` and `StorageError::RemoteProvider` for
  structured network-provider diagnostics and retry classification.
- Added unit coverage for SMB configuration, secret redaction, key mapping,
  temporary-file filtering, and retry/error classification.
- Added opt-in Samba integration coverage for authentication, workgroup
  credentials, signing, encryption, streamed round trips, interruption
  cleanup, atomic conditional creation, and reconnect after server restart.
- `StorageBackend` and `StorageError` gain new variants. Exhaustive downstream
  matches must add SMB and remote-provider cases; see [MIGRATION.md](MIGRATION.md).

Earlier release details remain in [docs/release-notes.md](docs/release-notes.md).
