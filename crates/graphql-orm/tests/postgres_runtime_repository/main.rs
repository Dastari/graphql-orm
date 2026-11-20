//! Owned PostgreSQL runtime reads and repository contracts.
#![cfg(feature = "postgres")]

#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

const POSTGRES_16_IMAGE: &str =
    "postgres:16-alpine@sha256:f377d590e5474681d4d7838635031a2b50647923b7abfa803ce9b5ff35d5c663";

mod repository_only_postgres;
mod runtime_records_postgres;
mod runtime_relations_postgres;
