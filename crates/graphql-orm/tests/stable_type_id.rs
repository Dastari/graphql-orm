#![cfg(feature = "sqlite")]
use graphql_orm::prelude::*;
mod original {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "stable_records", plural = "StableRecords")]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
    }
}
mod moved {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "stable_records", plural = "StableRecords")]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
    }
}
mod stable_a {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "stable_records",
        plural = "StableRecords",
        stable_type_id = "stable_type_id::original::Record"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
    }
}
mod stable_b {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "stable_records",
        plural = "StableRecords",
        stable_type_id = "stable_type_id::original::Record"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
    }
}

fn catalog(ops: &'static [GeneratedGraphqlOperationDescriptor]) -> GraphqlOperationCatalog {
    GraphqlOperationCatalog::compose([(ops, true, true)])
}
#[test]
fn default_fingerprint_from_main() {
    // Captured before implementation at origin/main 0c53b384, default SQLite profile.
    let ops = original::Record::generated_graphql_operations();
    assert_eq!(
        ops[0].entity_rust_type(),
        "stable_type_id::original::Record"
    );
    assert_eq!(
        ops[0].fingerprint(),
        "ce2af58f85324bf49aa949f1b4326fbb3e3bcb481df6e074c921e96f84f836c7"
    );
    assert_eq!(
        catalog(ops).fingerprint(),
        "7e62b13d0c914d59bd071830ab3dc3e513195adb72f5cbd6be3c1afe80a6c6a6"
    );
}
#[test]
fn stable_identity_survives_module_moves() {
    let original = original::Record::generated_graphql_operations();
    let moved = moved::Record::generated_graphql_operations();
    assert_ne!(original[0].fingerprint(), moved[0].fingerprint());
    assert_eq!(
        original[0].entity_rust_type(),
        original[0].diagnostic_rust_type()
    );
    let a = stable_a::Record::generated_graphql_operations();
    let b = stable_b::Record::generated_graphql_operations();
    assert_eq!(
        a[0].diagnostic_rust_type(),
        "stable_type_id::stable_a::Record"
    );
    assert_ne!(
        catalog(original).fingerprint(),
        catalog(moved).fingerprint()
    );
    for ops in [a, b] {
        assert_eq!(catalog(original).fingerprint(), catalog(ops).fingerprint());
        for (before, after) in original.iter().zip(ops) {
            assert_eq!(before.fingerprint(), after.fingerprint());
            assert_eq!(
                before.router_export_fingerprint(),
                after.router_export_fingerprint()
            );
        }
    }
}
#[test]
#[should_panic(expected = "duplicate entity identity")]
fn composition_rejects_duplicate_identity() {
    GraphqlOperationCatalog::compose([
        (stable_a::Record::generated_graphql_operations(), true, true),
        (stable_b::Record::generated_graphql_operations(), true, true),
    ]);
}

#[test]
fn tool_binding_survives_move() {
    use graphql_orm_ai_tool_profiles::{GraphqlExecutionTargetId, GraphqlOperationContract};
    let before = catalog(stable_a::Record::generated_graphql_operations());
    let after = catalog(stable_b::Record::generated_graphql_operations());
    let operation = before
        .operations()
        .iter()
        .find(|op| op.kind() == GraphqlOperationKind::Query)
        .unwrap();
    let field = operation.field_name();
    let document = format!("query Stable {{ {field} {{ __typename }} }}");
    let bind = |catalog: &GraphqlOperationCatalog| {
        GraphqlOperationContract::new(
            GraphqlExecutionTargetId::parse("local").unwrap(),
            "schema",
            "Stable",
            &document,
            "projection",
            "disclosure",
        )
        .unwrap()
        .with_generated_operation(catalog, GraphqlOperationKind::Query, field, &document)
        .unwrap()
    };
    let old = bind(&before);
    let new = bind(&after);
    assert_eq!(old.generated_operation(), new.generated_operation());
    assert_eq!(
        old.generated_operation().unwrap().operation_fingerprint(),
        operation.fingerprint()
    );
}

mod repository {
    use graphql_orm::prelude::*;
    #[derive(RepositoryEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
    #[repository_entity(
        table = "repository_records",
        plural = "RepositoryRecords",
        stable_type_id = "stable::RepositoryRecord"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
    }
}
#[test]
fn repository_surface_accepts_stable_identity() {
    // RepositoryEntity strips GraphQL descriptors, but shares entity option validation.
    use graphql_orm::graphql::orm::Entity;
    assert_eq!(
        repository::Record::metadata().table_name,
        "repository_records"
    );
}

#[test]
fn declared_identity_cannot_collide_with_default_identity() {
    let error = std::panic::catch_unwind(|| {
        GraphqlOperationCatalog::compose([
            (original::Record::generated_graphql_operations(), true, true),
            (stable_a::Record::generated_graphql_operations(), true, true),
        ])
    })
    .unwrap_err();
    let message = error.downcast_ref::<String>().unwrap();
    assert!(message.contains("stable_type_id::original::Record"));
    assert!(message.contains("stable_type_id::stable_a::Record"));
}
