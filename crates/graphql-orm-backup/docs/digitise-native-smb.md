# Digitise Native SMB Integration Brief

Keep Digitise settings and policy in the host; reusable crates expose storage
and backup primitives only.

- Replace `backup.smb.mountPath` for the native provider with server, port,
  share, optional root prefix, username, optional domain/workgroup, minimum
  dialect, signing/encryption requirements, and timeout fields.
- Persist the password through Digitise's encrypted secret-settings service.
  GraphQL and non-secret exports expose only `passwordConfigured`.
- Build `SmbStorageConfig` from resolved settings and return
  `Arc<dyn BackupRepository>` containing `BlobStoreBackupRepository` over an
  `Arc<dyn BlobStore>` `SmbStorageBackend`.
- If mounted compatibility is needed, retain it under a distinct name such as
  `mounted_smb_legacy`; never interpret a local path as native SMB config.
- Apply the existing platform-admin `agql-auth` guard to credential changes,
  validation, backup, restore, delete, and prune. Restore also retains explicit
  confirmation, maintenance mode, and operator policy.
- Audit configuration changes, probes, backup, restore, verify, delete, and
  prune without authentication material.
- Scheduled backups use an internal trusted service path; do not fabricate a
  GraphQL user or expose an unguarded resolver.

No `agql-auth` change is required. SMB authentication proves an identity to a
remote storage server, separate from application-user authentication. The host
already expresses platform-admin authorization and trusted internal execution.

Digitise currently builds its backup object index only from
`LocalStorageBackend`. Native SMB as a destination does not fix that independent
restriction. Build the index from the configured `Arc<dyn BlobStore>` so full
backups can read referenced objects from any supported primary provider.
