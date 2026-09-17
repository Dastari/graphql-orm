#![cfg(feature = "sqlite")]
//! A Federation entity key must not change the operation contract.
//!
//! `#[graphql(entity)]` resolvers are not schema fields: they are reached only
//! through `_entities`. The operation catalogue and the router-protocol export
//! describe root fields, so declaring a key must leave both byte-identical.

use graphql_orm::prelude::*;

/// The keyed and unkeyed entities differ only in the attribute under test, so
/// their contracts are compared after normalising the type name away.
macro_rules! contract_entity {
    ($module:ident, $entity:ident, $table:literal, $plural:literal, $($key:tt)*) => {
        mod $module {
            use super::*;

            #[derive(
                GraphQLEntity,
                GraphQLOperations,
                serde::Serialize,
                serde::Deserialize,
                Clone,
                Debug,
                PartialEq,
            )]
            #[graphql_entity(
                backend = "sqlite",
                table = $table,
                plural = $plural,
                default_sort = "name ASC",
                auth = "required"
                $($key)*
            )]
            pub struct $entity {
                #[primary_key]
                pub id: String,

                #[filterable(type = "string")]
                #[sortable]
                pub name: String,
            }

            schema_roots! {
                backend: "sqlite",
                query_custom_ops: [],
                entities: [$entity],
            }
        }
    };
}

contract_entity!(keyed, KeyedZone, "keyed_zones", "KeyedZones", , federation_key);
contract_entity!(unkeyed, PlainZone, "plain_zones", "PlainZones",);

/// Erase the only intentional difference between the two entities: their name.
fn normalise(value: &str) -> String {
    value
        .replace("KeyedZone", "Zone")
        .replace("PlainZone", "Zone")
        .replace("keyedZone", "zone")
        .replace("plainZone", "zone")
}

#[test]
fn the_operation_catalogue_is_identical_with_and_without_a_key() {
    let keyed = keyed::graphql_orm_operation_catalog();
    let unkeyed = unkeyed::graphql_orm_operation_catalog();

    let describe = |catalog: &graphql_orm::graphql::orm::GraphqlOperationCatalog| {
        catalog
            .operations()
            .iter()
            .map(|operation| {
                normalise(&format!(
                    "{:?}|{:?}|{}|{}",
                    operation.kind(),
                    operation.category(),
                    operation.field_name(),
                    operation.is_exposed()
                ))
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(describe(&keyed), describe(&unkeyed));
    assert!(
        !describe(&keyed)
            .iter()
            .any(|entry| entry.contains("__gom_federation") || entry.contains("_entities")),
        "an entity resolver is not a catalogued root field"
    );
}

#[cfg(feature = "router-protocol")]
#[test]
fn the_router_protocol_export_is_identical_with_and_without_a_key() {
    let describe = |operations: Vec<graphql_orm_router_protocol::OperationDescriptor>| {
        operations
            .iter()
            .map(|operation| {
                normalise(&format!(
                    "{:?}|{}|{:?}|{:?}",
                    operation.root_type,
                    operation.field_name,
                    operation.arguments,
                    operation.authorization
                ))
            })
            .collect::<Vec<_>>()
    };

    let keyed = keyed::graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("protocol export");
    let unkeyed = unkeyed::graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("protocol export");

    assert_eq!(describe(keyed), describe(unkeyed));
}
