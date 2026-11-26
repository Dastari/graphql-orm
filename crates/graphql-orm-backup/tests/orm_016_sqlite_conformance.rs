//! Backup adapter conformance against temporary SQLite databases.

// The shared conformance suite builds `LocalBackupRepository::new`, which does
// not exist on Windows.
#![cfg(not(windows))]

include!("support/orm_016_conformance.rs");
