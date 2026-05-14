# SMB Mounted Repository Guidance

Native SMB protocol support is out of scope for the current crate. Use an
operating-system mounted SMB share as a filesystem path and point
`LocalBackupRepository` at that mount.

## Responsibilities Outside This Crate

The host system or application deployment must manage:

- SMB mount creation
- credentials
- reconnect behavior
- network availability
- filesystem permissions
- available space monitoring

## Repository Root Validation

Use `LocalBackupRepository::open_existing` when a repository root should already
exist:

```rust
use graphql_orm_backup::LocalBackupRepository;

# async fn example() -> Result<(), graphql_orm_backup::BackupError> {
let repository = LocalBackupRepository::open_existing("/mnt/backups").await?;
# Ok(())
# }
```

`open_existing` validates that the path exists and is a directory. It does not
create the mount or change permissions.

## Write Semantics

The local repository writes blobs by creating parent directories, writing a
temporary file, and renaming it into place. Mounted SMB deployments must support
that workflow reliably enough for the application’s backup requirements.

## Key Safety

Repository keys are validated before joining them to the root path. Keys reject:

- empty keys
- absolute paths
- empty path segments
- `.`
- `..`
- backslashes
- NUL bytes
- platform prefix components

`list_blobs("")` is intentionally allowed and lists the whole repository.
