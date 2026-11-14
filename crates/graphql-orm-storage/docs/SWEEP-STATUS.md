# SMB stale temporary-file sweep: status

This branch preserves the `SmbStorageBackend::sweep_temp_files` work for
#593, removed from PR #691 after four review rounds so that the other storage
and backup fixes could ship. It is not merged and has no PR.

## Decision already taken: narrowed contract

The sweep is a maintenance-window operation on a repository tree the host
controls. The witness `SmbRepositoryQuiesced::confirmed_by_host()` states that,
for the whole sweep, no other party modifies the repository tree at all (no
uploads, renames, deletions, or directory or reparse-point changes), and the
sweep must not run where an untrusted party has write access to the tree. The
held directory chain (`FILE_OPEN_REPARSE_POINT`, reparse check on the handle,
no `FILE_SHARE_DELETE`), the held-parent re-query before each candidate, the
exclusive candidate open and the size/last-write re-check are defence in depth
only: they stop rename, delete or replacement of held directories by other SMB
clients on servers that enforce share modes, but not a client with write
access emptying a held directory and turning it into a junction, nor changes
made outside SMB.

## Open findings (re-check of #691 at d35f604f)

1. **Teardown depends on the caller's runtime, and its fallback is disarmed
   too early** (`src/smb.rs`, `SweepSession::shutdown` and `Drop for
   SweepSession`, around lines 2188 and 2199). The dedicated session's
   disconnect on drop is spawned on the current Tokio runtime, so it needs
   that runtime to keep making progress. `shutdown` takes the connection out
   of the guard (disarming the drop fallback) before `Client::close()` is
   awaited, so cancelling `shutdown` leaves no disconnect at all.
2. **Cleanup failures are still dropped on several paths** (`src/smb.rs`
   around lines 922, 1143 and 1264, and inside the vendored `Client::close`).
   Close errors on error and cancellation paths, during directory holding and
   candidate removal, and inside `Client::close` (which ignores per-channel
   close errors) are discarded rather than counted or returned.

Related follow-up, outside the sweep: the vendored
`ResourceHandle::close` (`vendor/smb/src/resource.rs`, around line 798) marks a
handle closed before sending CLOSE, and its destructor then declines cleanup,
so a close future dropped while waiting for a credit leaks the server handle
on a shared session.
