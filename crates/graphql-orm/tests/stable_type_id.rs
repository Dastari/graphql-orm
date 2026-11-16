#![cfg(feature = "sqlite")]
use graphql_orm::prelude::*;
mod original {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(backend = "sqlite", table = "stable_records", plural = "StableRecords")]
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
    #[graphql_entity(backend = "sqlite", table = "stable_records", plural = "StableRecords")]
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
        backend = "sqlite",
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
        backend = "sqlite",
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
fn default_fingerprint_is_pinned_for_algorithm_v2() {
    // `graphql-orm-sha256-len-v2`, default SQLite profile. The version 1
    // values, which hashed the Rust module path, were
    // ce2af58f85324bf49aa949f1b4326fbb3e3bcb481df6e074c921e96f84f836c7 and
    // 7e62b13d0c914d59bd071830ab3dc3e513195adb72f5cbd6be3c1afe80a6c6a6.
    assert_eq!(
        GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM,
        "graphql-orm-sha256-len-v2"
    );
    let ops = original::Record::generated_graphql_operations();
    assert_eq!(
        ops[0].entity_rust_type(),
        "stable_type_id::original::Record"
    );
    assert_eq!(
        ops[0].fingerprint(),
        "43df67731872a606b06dc83153448b0e4dc56499f6c50685a3fd9c7d4d6081de"
    );
    assert_eq!(
        catalog(ops).fingerprint(),
        "57eaa5ce462f4719d04e768d58c01236d8ac010ea602d4524542ef3a79957a37"
    );
}
#[test]
fn fingerprints_survive_module_moves_without_any_declaration() {
    // The same entity declared in another module, with no `stable_type_id`.
    let original = original::Record::generated_graphql_operations();
    let moved = moved::Record::generated_graphql_operations();
    assert_ne!(original[0].entity_rust_type(), moved[0].entity_rust_type());
    assert_eq!(
        moved[0].diagnostic_rust_type(),
        "stable_type_id::moved::Record"
    );
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
    // A declared `stable_type_id` is accepted and changes no fingerprint.
    for ops in [moved, a, b] {
        assert_eq!(original.len(), ops.len());
        assert_eq!(catalog(original).fingerprint(), catalog(ops).fingerprint());
        assert_eq!(
            catalog(original).router_export_fingerprint(),
            catalog(ops).router_export_fingerprint()
        );
        for (before, after) in original.iter().zip(ops) {
            assert_eq!(before.fingerprint(), after.fingerprint());
            assert_eq!(
                before.authorization_fingerprint(),
                after.authorization_fingerprint()
            );
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
    let before = catalog(original::Record::generated_graphql_operations());
    let after = catalog(moved::Record::generated_graphql_operations());
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
        backend = "sqlite",
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

// These aliases name the same Rust/GraphQL type; only their source paths differ.
mod field_types {
    pub type Status = String;
}
mod moved_field_types {
    pub use super::field_types::Status;
}
mod qualified_before {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "spelling_records",
        plural = "SpellingRecords",
        stable_type_id = "stable::SpellingRecord"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub status: crate::field_types::Status,
    }
}
mod qualified_after {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "spelling_records",
        plural = "SpellingRecords",
        stable_type_id = "stable::SpellingRecord"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub status: crate::moved_field_types::Status,
    }
}
mod imported_before {
    use crate::field_types::Status;
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "spelling_records",
        plural = "SpellingRecords",
        stable_type_id = "stable::SpellingRecord"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub status: Status,
    }
}
mod imported_after {
    use crate::moved_field_types::Status;
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "spelling_records",
        plural = "SpellingRecords",
        stable_type_id = "stable::SpellingRecord"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub status: Status,
    }
}

#[test]
fn stable_identity_still_requires_unchanged_type_spelling() {
    let before = qualified_before::Record::generated_graphql_operations();
    let after = qualified_after::Record::generated_graphql_operations();
    assert_eq!(before.len(), after.len());
    for (before, after) in before.iter().zip(after) {
        assert_eq!(before.entity_rust_type(), after.entity_rust_type());
        assert_eq!(before.graphql_result_type(), after.graphql_result_type());
        assert_ne!(before.schema_signature(), after.schema_signature());
        assert_ne!(before.fingerprint(), after.fingerprint());
    }
    assert_ne!(catalog(before).fingerprint(), catalog(after).fingerprint());

    let before = imported_before::Record::generated_graphql_operations();
    let after = imported_after::Record::generated_graphql_operations();
    assert_eq!(before.len(), after.len());
    for (before, after) in before.iter().zip(after) {
        assert_eq!(before.schema_signature(), after.schema_signature());
        assert_eq!(before.fingerprint(), after.fingerprint());
    }
    assert_eq!(catalog(before).fingerprint(), catalog(after).fingerprint());
}
mod maximum_identity {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "spelling_records",
        plural = "SpellingRecords",
        stable_type_id = "ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss"
    )]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub status: String,
    }
}

#[test]
fn stable_type_id_accepts_exactly_1024_bytes() {
    let operations = maximum_identity::Record::generated_graphql_operations();
    assert!(!operations.is_empty());
    assert!(
        operations
            .iter()
            .all(|operation| operation.entity_rust_type().len() == 1024)
    );
}
