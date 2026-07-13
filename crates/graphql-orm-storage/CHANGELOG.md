# Changelog

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
