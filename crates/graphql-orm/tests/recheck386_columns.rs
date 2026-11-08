#![cfg(feature = "sqlite")]
use graphql_orm::graphql::orm::generated_relation_column_is_readable;

// These execute in const evaluation, as generated cross-crate relation proofs do.
const _: () = {
    let columns: &[(&str, bool)] = &[("id", true), ("SecretKey", false), ("secretkey", true)];
    assert!(generated_relation_column_is_readable(columns, "ID", false));
    assert!(!generated_relation_column_is_readable(columns, "ID", true));
    assert!(generated_relation_column_is_readable(
        columns,
        "secretkey",
        true
    ));
    assert!(!generated_relation_column_is_readable(
        columns,
        "SecretKey",
        true
    ));
    assert!(!generated_relation_column_is_readable(
        columns,
        "secretkey",
        false
    ));
    assert!(!generated_relation_column_is_readable(
        columns, "absent", false
    ));
};

#[test]
fn target_metadata_is_generated_for_metadata_only_entities() {
    use graphql_orm::graphql::orm::GeneratedRelationReadability;
    #[derive(
        graphql_orm::GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
    struct Record {
        #[primary_key]
        id: i32,
        #[graphql_orm(private, db_column = "SecretKey")]
        secret: String,
        #[input_only]
        input: String,
        #[graphql_orm(read = false)]
        hidden: String,
        #[skip_db]
        transient: String,
    }
    assert_eq!(
        Record::COLUMNS,
        &[
            ("id", true),
            ("SecretKey", false),
            ("input", false),
            ("hidden", false)
        ]
    );
}
