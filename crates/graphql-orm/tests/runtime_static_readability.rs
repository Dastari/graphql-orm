// Scalar output ownership is checked separately from relation implementation skips.
#![cfg(all(
    feature = "sqlite",
    feature = "runtime-graphql",
    not(feature = "postgres")
))]

use graphql_orm::{
    async_graphql::{self, EmptyMutation, EmptySubscription, Object, SimpleObject},
    graphql::{orm::*, runtime::*},
};
use graphql_orm_macros::{GraphQLEntity, GraphQLSchemaEntity, RepositoryEntity};
use std::sync::Arc;

#[derive(
    GraphQLEntity, SimpleObject, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "probe_secrets",
    plural = "ProbeSecrets",
    schema_only = true
)]
struct ProbeSecret {
    #[primary_key]
    id: i64,
    #[graphql(skip)]
    password: String,
    #[graphql(skip = true)]
    skipped_true: String,
    #[graphql(skip_output)]
    skipped_output: String,
    #[graphql(skip_output = true)]
    skipped_output_true: String,
    #[graphql(skip = false)]
    public_skip_false: String,
    #[graphql(skip_output = false)]
    public_output_false: String,
}

#[derive(
    GraphQLEntity, SimpleObject, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "complex_secrets",
    plural = "ComplexSecrets"
)]
struct ComplexSecret {
    #[primary_key]
    id: i64,
    #[filterable(type = "string")]
    #[sortable]
    public_value: String,
    #[graphql(skip)]
    password: String,
}
#[async_graphql::ComplexObject]
impl ComplexSecret {}

struct Query;
#[Object]
impl Query {
    async fn probe(&self) -> ProbeSecret {
        ProbeSecret {
            id: 1,
            password: "TOPSECRET".into(),
            skipped_true: "TOPSECRET".into(),
            skipped_output: "TOPSECRET".into(),
            skipped_output_true: "TOPSECRET".into(),
            public_skip_false: "public".into(),
            public_output_false: "public".into(),
        }
    }

    async fn complex_probe(&self) -> ComplexSecret {
        ComplexSecret {
            id: 1,
            public_value: "public".into(),
            password: "TOPSECRET".into(),
        }
    }
}

fn assert_refused(metadata: &EntityMetadata, hidden: &[&str]) {
    for name in hidden {
        assert!(
            !metadata
                .fields
                .iter()
                .find(|f| f.rust_name == *name)
                .unwrap()
                .is_readable,
            "{name}"
        );
    }
    let diagnostics = RuntimeSchema::from_static_entities(&[metadata]).unwrap_err();
    for name in hidden {
        assert!(
            diagnostics.diagnostics().iter().any(|d| d.code
                == RuntimeSchemaDiagnosticCode::UnsupportedCapability
                && d.subject.as_deref() == Some(*name)),
            "{name}: {diagnostics}"
        );
    }
    let internal = RuntimeSchema::from_static_entities_for_repository(&[metadata]).unwrap();
    assert!(internal.repository_only);
    assert_eq!(internal.collections[0].fields.len(), metadata.fields.len());
    let internal: RuntimeSchema =
        serde_json::from_str(&serde_json::to_string(&internal).unwrap()).unwrap();
    let error =
        RuntimeGraphqlModule::compile(Arc::new(internal.validate().unwrap()), Default::default())
            .err()
            .expect("internal skipped columns must never be served");
    assert_eq!(error.0[0].code(), "repository_only_schema");
}

#[tokio::test]
async fn simple_object_skips_cannot_be_reopened_by_runtime_conversion() {
    let api = async_graphql::Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    for name in [
        "password",
        "skippedTrue",
        "skippedOutput",
        "skippedOutputTrue",
    ] {
        let response = api.execute(format!("{{probe{{{name}}}}}")).await;
        assert!(!response.errors.is_empty(), "{name}");
    }
    assert!(
        !api.execute("{complexProbe{password}}")
            .await
            .errors
            .is_empty()
    );
    let public = api
        .execute("{probe{publicSkipFalse publicOutputFalse}}")
        .await;
    assert!(public.errors.is_empty(), "{:?}", public.errors);
    assert_refused(
        ProbeSecret::metadata(),
        &[
            "password",
            "skipped_true",
            "skipped_output",
            "skipped_output_true",
        ],
    );
    assert_refused(ComplexSecret::metadata(), &["password"]);
    for name in ["public_skip_false", "public_output_false"] {
        assert!(
            ProbeSecret::metadata()
                .fields
                .iter()
                .find(|f| f.rust_name == name)
                .unwrap()
                .is_readable
        );
    }
}

#[derive(GraphQLEntity, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "owned_scalars", plural = "OwnedScalars")]
struct OwnedScalar {
    #[primary_key]
    id: i64,
    #[filterable(type = "string")]
    #[sortable]
    public_value: String,
    // The ORM owns this getter and emits it; SimpleObject skip is input-only here.
    #[graphql(skip)]
    value: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "metadata_scalars",
    plural = "MetadataScalars"
)]
struct MetadataScalar {
    #[primary_key]
    id: i64,
    #[graphql(skip)]
    value: String,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "repository_scalars",
    plural = "RepositoryScalars"
)]
struct RepositoryScalar {
    public_value: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[graphql(skip)]
    value: String,
}

#[test]
fn scalar_metadata_respects_the_object_owner() {
    assert!(OwnedScalar::metadata().fields.iter().all(|f| f.is_readable));
    assert!(RuntimeSchema::from_static_entities(&[OwnedScalar::metadata()]).is_ok());
    assert_refused(MetadataScalar::metadata(), &["value"]);
    // Metadata-only repository declarations retain ordinary readable columns,
    // while explicit output restrictions still fail closed.
    assert!(
        RepositoryScalar::metadata()
            .fields
            .iter()
            .find(|f| f.name == "id")
            .unwrap()
            .is_readable
    );
    assert_refused(RepositoryScalar::metadata(), &["value"]);
}

// GraphQLRelations supplies this getter; SimpleObject must skip the storage slot.
#[derive(
    GraphQLEntity,
    graphql_orm_macros::GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "owned_relations",
    plural = "OwnedRelations"
)]
struct OwnedRelation {
    #[primary_key]
    #[sortable]
    id: i64,
    scalar_id: i64,
    #[graphql(skip)]
    #[relation(target = "OwnedScalar", from = "scalar_id", to = "id")]
    scalar: Option<OwnedScalar>,
}

struct RelationQuery;
#[Object]
impl RelationQuery {
    async fn holder(&self) -> OwnedRelation {
        OwnedRelation {
            id: 1,
            scalar_id: 2,
            scalar: None,
        }
    }
}

#[test]
fn generated_relation_implementation_skip_remains_readable() {
    let metadata = OwnedRelation::metadata();
    assert!(metadata.relations[0].is_readable);
    let api =
        async_graphql::Schema::build(RelationQuery, EmptyMutation, EmptySubscription).finish();
    assert!(api.sdl().contains("scalar: OwnedScalar"));
    let schema = RuntimeSchema::from_static_entities(&[metadata, OwnedScalar::metadata()]).unwrap();
    assert!(!schema.repository_only);
    assert_eq!(schema.collections[0].relations.len(), 1);
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::SqliteBackend> for OwnedScalar {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(
        row: &graphql_orm::sqlx::sqlite::SqliteRow,
    ) -> graphql_orm::Result<String> {
        use graphql_orm::sqlx::Row;
        Ok(row.try_get::<i64, _>("id")?.to_string())
    }
}
