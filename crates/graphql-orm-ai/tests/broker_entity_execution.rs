//! Capability broker version 2 over a real generated schema on SQLite. The
//! owned PostgreSQL parity lane runs the same round trip.

#![cfg(feature = "sqlite")]

#[path = "support/broker_entity_parity.rs"]
mod broker_entity_parity;

#[tokio::test]
async fn broker_v2_entity_capability_executes_one_root_through_the_generated_schema() {
    let database = graphql_orm::prelude::Database::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite");
    broker_entity_parity::entity_capability_round_trip(database).await;
}

#[tokio::test]
async fn broker_v3_browses_and_pages_a_keyset_root_through_continuation_handles() {
    let database = graphql_orm::prelude::Database::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite");
    broker_entity_parity::keyset_continuation_round_trip(database).await;
}
