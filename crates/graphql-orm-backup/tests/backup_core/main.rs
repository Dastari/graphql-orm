//! Core backup tests: capacity limits, compression, locking, manifests and full backups.

mod capacity_limits;
mod compression;
mod custom_blob_store_lock;
mod full_backup_creation;
mod manifest_chain;
mod manifest_round_trip;
