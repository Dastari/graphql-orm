// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

//! A minimal external-style read surface (generated key, server-owned
//! `skip_input` column and a relation) beside a writable entity, compiled for
//! whichever single backend this build selects. The CI backend lanes compile
//! this target for SQLite, PostgreSQL and SQL Server.

use graphql_orm::graphql::orm::{GraphqlOperationKind, GraphqlOperationMetadata};
use graphql_orm::prelude::*;

macro_rules! entities {
    ($($policy:tt)*) => {
        /// No publicly writable create or update field.
        #[derive(
            GraphQLEntity,
            GraphQLOperations,
            GraphQLRelations,
            graphql_orm::async_graphql::SimpleObject,
            Clone,
            Debug,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[graphql(complex)]
        #[graphql_entity(table = "licences", plural = "Licences", default_sort = "id ASC" $($policy)*)]
        pub struct Licence {
            #[primary_key]
            #[sortable]
            pub id: String,
            #[graphql_orm(skip_input)]
            pub refreshed_at: String,
            #[graphql(skip)]
            #[serde(default)]
            #[relation(target = "Seat", from = "id", to = "licence_id", multiple, emit_fk = false)]
            pub seats: Vec<Seat>,
        }

        /// An ordinary writable entity.
        #[derive(
            GraphQLEntity,
            GraphQLOperations,
            GraphQLRelations,
            graphql_orm::async_graphql::SimpleObject,
            Clone,
            Debug,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[graphql(complex)]
        #[graphql_entity(table = "seats", plural = "Seats", default_sort = "label ASC" $($policy)*)]
        pub struct Seat {
            #[primary_key]
            #[sortable]
            pub id: String,
            #[filterable(type = "string")]
            pub licence_id: String,
            #[sortable]
            pub label: String,
            #[graphql(skip)]
            #[serde(default)]
            #[relation(target = "Licence", from = "licence_id", to = "id", emit_fk = false)]
            pub licence: Option<Licence>,
        }
    };
}

#[cfg(feature = "mssql")]
entities!(, schema_policy = "external_writable");
#[cfg(not(feature = "mssql"))]
entities!();

macro_rules! batch_by_id {
    ($($entity:ty),*) => {$(
        impl BatchLoadEntity for $entity {
            fn batch_column() -> &'static str {
                "id"
            }
            fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
                // SQL Server rows expose `try_get` without the sqlx trait.
                #[cfg_attr(feature = "mssql", allow(unused_imports))]
                use graphql_orm::sqlx::Row;
                row.try_get("id")
            }
        }
    )*};
}
batch_by_id!(Licence, Seat);

schema_roots! {
    generated_mutations: "none",
    query_custom_ops: [],
    entities: [Licence, Seat],
}

fn operations<T: GraphqlOperationMetadata>(kind: GraphqlOperationKind) -> Vec<&'static str> {
    T::generated_graphql_operations()
        .iter()
        .filter(|operation| operation.kind() == kind)
        .map(|operation| operation.field_name())
        .collect()
}

#[allow(dead_code)]
fn repository_inputs_keep_server_owned_columns() -> (CreateLicenceInput, UpdateLicenceInput) {
    (
        CreateLicenceInput {
            refreshed_at: "t1".into(),
        },
        UpdateLicenceInput {
            refreshed_at: Some("t2".into()),
        },
    )
}

#[test]
fn only_input_free_mutations_are_described_for_the_read_surface() {
    assert_eq!(
        operations::<Licence>(GraphqlOperationKind::Mutation),
        ["deleteLicence", "deleteLicences"]
    );
    assert_eq!(
        operations::<Seat>(GraphqlOperationKind::Mutation),
        [
            "createSeat",
            "deleteSeat",
            "deleteSeats",
            "updateSeat",
            "updateSeats"
        ]
    );
    for entity in [
        operations::<Licence>(GraphqlOperationKind::Query),
        operations::<Seat>(GraphqlOperationKind::Query),
    ] {
        assert!(!entity.is_empty());
    }
    assert_eq!(
        operations::<Licence>(GraphqlOperationKind::Subscription),
        ["licenceChanged"]
    );
}
