// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
// The entity exists only to be described; its generated roots are never mounted.
#![allow(dead_code)]
//! #851: a guarded table is recorded by every typed metadata constructor.
//!
//! This binary is its own process, and nothing in it calls
//! `FreshGuarded::metadata()`. The registry is therefore fresh when the test
//! builds metadata through `EntityMetadata::from_schema::<FreshGuarded>()`,
//! so the refusal cannot depend on the generated `metadata()` having run.

use graphql_orm::graphql::orm::{EntityMetadata, RuntimeSchema, RuntimeSchemaDiagnostics};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "fresh_guarded_records",
        plural = "FreshGuardedRecords",
        auth = "none",
        mutation_shapes = "create,update,delete",
        update_expected_version = "required"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "fresh_guarded_records",
        plural = "FreshGuardedRecords",
        auth = "none",
        mutation_shapes = "create,update,delete",
        update_expected_version = "required"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "fresh_guarded_records",
        plural = "FreshGuardedRecords",
        auth = "none",
        mutation_shapes = "create,update,delete",
        update_expected_version = "required"
    )
)]
struct FreshGuarded {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    state: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

const MESSAGE: &str = "update_expected_version and update_assurance are not represented";

fn refused(result: Result<RuntimeSchema, RuntimeSchemaDiagnostics>) -> bool {
    result.err().is_some_and(|diagnostics| {
        diagnostics
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains(MESSAGE))
    })
}

#[test]
fn typed_constructors_record_a_guarded_table_before_metadata_runs() {
    let metadata = EntityMetadata::from_schema::<FreshGuarded>(
        "FreshGuarded",
        true,
        None,
        None,
        None,
        None,
        false,
    );
    assert!(refused(RuntimeSchema::from_static_entities(&[&metadata])));
    assert!(refused(RuntimeSchema::from_static_entities_for_repository(
        &[&metadata]
    )));
    let with_retention = EntityMetadata::from_schema_with_retention::<FreshGuarded>(
        "FreshGuarded",
        true,
        None,
        None,
        None,
        None,
        false,
        None,
    );
    assert!(refused(RuntimeSchema::from_static_entities(&[
        &with_retention
    ])));
    // Equivalent spellings of the table are refused too.
    for spelling in ["FRESH_GUARDED_RECORDS", "public.fresh_guarded_records"] {
        let mut copy = metadata.clone();
        copy.table_name = spelling;
        assert!(
            refused(RuntimeSchema::from_static_entities(&[&copy])),
            "{spelling}"
        );
    }
}
