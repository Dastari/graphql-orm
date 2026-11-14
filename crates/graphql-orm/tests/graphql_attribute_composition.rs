//! Combined `#[graphql(...)]` options compose in any order, and generated
//! argument descriptions merge into existing argument attributes (#192).
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::async_graphql::{Schema, SimpleObject};
use graphql_orm::prelude::*;

#[path = "support/ui.rs"]
mod ui;

/// `rename_fields` before `complex`, in one attribute.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(table = "folders", plural = "Folders", default_sort = "folder_id ASC")]
#[graphql(rename_fields = "snake_case", complex)]
struct Folder {
    #[primary_key]
    #[sortable]
    folder_id: String,
    display_name: String,
    #[graphql(skip)]
    #[relation(target = "Document", from = "folder_id", to = "folder_id", multiple)]
    documents: Vec<Document>,
}

/// `complex` before `rename_fields`, in one attribute.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(
    table = "documents",
    plural = "Documents",
    default_sort = "document_id ASC"
)]
#[graphql(complex, rename_fields = "snake_case")]
struct Document {
    #[primary_key]
    #[sortable]
    document_id: String,
    #[filterable(type = "string")]
    folder_id: String,
    #[graphql(skip)]
    #[relation(target = "Label", from = "document_id", to = "document_id", multiple)]
    labels: Vec<Label>,
}

/// `complex` and `rename_fields` in separate attributes.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(table = "labels", plural = "Labels", default_sort = "label_id ASC")]
#[graphql(rename_fields = "snake_case")]
#[graphql(complex)]
struct Label {
    #[primary_key]
    #[sortable]
    label_id: String,
    #[filterable(type = "string")]
    document_id: String,
    /// A foreign `key = value` option before `name` must not hide it.
    #[graphql(deprecation = "use label_id", name = "legacy_code")]
    code: String,
    #[graphql(skip)]
    #[relation(target = "Document", from = "document_id", to = "document_id")]
    document: Option<Document>,
}

/// Valued `SimpleObject` options before or after `name` must not hide it
/// from the ORM's static API-name metadata (#405).
#[derive(GraphQLEntity, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "alias_probes", plural = "AliasProbes", schema_only = true)]
struct AliasProbe {
    #[primary_key]
    id: i64,
    #[graphql(skip = false, name = "afterSkipFalse")]
    after_skip_false: String,
    #[graphql(name = "beforeSkipFalse", skip = false)]
    before_skip_false: String,
    #[graphql(skip_output = false, name = "afterSkipOutputFalse")]
    after_skip_output_false: String,
    #[graphql(name = "beforeSkipOutputFalse", skip_output = false)]
    before_skip_output_false: String,
    #[graphql(visible = true, name = "afterVisible")]
    after_visible: String,
    #[graphql(name = "beforeVisible", visible = true)]
    before_visible: String,
    #[graphql(skip = false)]
    #[graphql(name = "separateAttributes")]
    separate_attributes: String,
}

const ALIAS_PROBE_NAMES: [(&str, &str); 7] = [
    ("after_skip_false", "afterSkipFalse"),
    ("before_skip_false", "beforeSkipFalse"),
    ("after_skip_output_false", "afterSkipOutputFalse"),
    ("before_skip_output_false", "beforeSkipOutputFalse"),
    ("after_visible", "afterVisible"),
    ("before_visible", "beforeVisible"),
    ("separate_attributes", "separateAttributes"),
];

struct AliasProbeQuery;
#[graphql_orm::async_graphql::Object]
impl AliasProbeQuery {
    async fn probe(&self) -> AliasProbe {
        AliasProbe {
            id: 1,
            after_skip_false: "a".into(),
            before_skip_false: "b".into(),
            after_skip_output_false: "c".into(),
            before_skip_output_false: "d".into(),
            after_visible: "e".into(),
            before_visible: "f".into(),
            separate_attributes: "g".into(),
        }
    }
}

#[tokio::test]
async fn static_api_names_do_not_depend_on_graphql_option_order() {
    let schema = Schema::build(
        AliasProbeQuery,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();
    let sdl = schema.sdl();
    let start = sdl.find("type AliasProbe {").expect("AliasProbe in SDL");
    let body = &sdl[start..start + sdl[start..].find('}').unwrap()];
    let metadata = AliasProbe::metadata();
    for (rust_name, api_name) in ALIAS_PROBE_NAMES {
        // The name async-graphql serves.
        assert!(
            body.contains(&format!("{api_name}: String!")),
            "{api_name} is not served:\n{body}"
        );
        // The name the ORM's static metadata records for the same field.
        let field = metadata
            .fields
            .iter()
            .find(|field| field.name == rust_name)
            .unwrap_or_else(|| panic!("{rust_name} field metadata"));
        assert_eq!(field.api_name, api_name, "{rust_name}");
        let column = <AliasProbe as graphql_orm::graphql::orm::DatabaseSchema>::columns()
            .iter()
            .find(|column| column.name == rust_name)
            .unwrap_or_else(|| panic!("{rust_name} column"));
        assert_eq!(column.api_name, api_name, "{rust_name}");
    }
    // The served values resolve under those names.
    let selection = ALIAS_PROBE_NAMES.map(|(_, api_name)| api_name).join(" ");
    let response = schema
        .execute(format!("{{ probe {{ {selection} }} }}"))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

macro_rules! batch_load {
    ($($entity:ty => $column:literal),* $(,)?) => {$(
        impl graphql_orm::graphql::loaders::BatchLoadEntity for $entity {
            fn batch_column() -> &'static str {
                $column
            }

            fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
                use sqlx::Row;
                row.try_get($column)
            }
        }
    )*};
}

batch_load!(Document => "folder_id", Label => "document_id");

#[derive(Default)]
struct CatalogueQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl CatalogueQueries {
    /// Counts documents in one folder.
    async fn count_documents(
        &self,
        #[graphql(name = "folderKey")] folder: String,
        #[graphql(name = "pageLimit", desc = "Most documents to count.")] limit: i32,
        #[graphql(default = 1, name = "minimumSize")] minimum: i32,
        offset: i32,
    ) -> i32 {
        let _ = folder;
        limit + minimum + offset
    }
}

#[derive(Default)]
struct CatalogueMutations;

#[graphql_orm_custom_operations(kind = "mutation", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl CatalogueMutations {
    /// Renames one folder.
    async fn rename_folder(
        &self,
        #[graphql(name = "folderKey")] folder: String,
        #[graphql(name = "newName")]
        #[graphql(validator(min_length = 1))]
        name: String,
    ) -> String {
        format!("{folder}:{name}")
    }
}

schema_roots! {
    entities: [Folder, Document, Label],
    described_query_types: [CatalogueQueries],
    described_mutation_types: [CatalogueMutations],
}

fn sdl() -> String {
    Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish()
    .sdl()
}

#[test]
fn combined_complex_options_emit_relation_fields_in_any_order() {
    let sdl = sdl();
    for (object, relation) in [
        ("Folder", "documents"),
        ("Document", "labels"),
        ("Label", "document"),
    ] {
        let start = sdl
            .find(&format!("type {object} {{"))
            .unwrap_or_else(|| panic!("{object} in SDL:\n{sdl}"));
        let body = &sdl[start..start + sdl[start..].find('}').unwrap()];
        assert!(
            body.contains(relation),
            "{object} lacks {relation}:\n{body}"
        );
    }
    assert!(sdl.contains("legacy_code"), "{sdl}");
    let label = Label::metadata();
    let code = label
        .fields
        .iter()
        .find(|column| column.name == "code")
        .expect("code column");
    assert_eq!(code.api_name, "legacy_code");
}

#[test]
fn generated_argument_descriptions_merge_into_existing_attributes() {
    let sdl = sdl();
    for argument in [
        "\"\"\"\n\t\tFolder\n\t\t\"\"\"\n\t\tfolderKey: String!",
        "\"\"\"\n\t\tMost documents to count.\n\t\t\"\"\"\n\t\tpageLimit: Int!",
        "\"\"\"\n\t\tMinimum\n\t\t\"\"\"\n\t\tminimumSize: Int! = 1",
        "\"\"\"\n\t\tOffset\n\t\t\"\"\"\n\t\toffset: Int!",
        "\"\"\"\n\t\tName\n\t\t\"\"\"\n\t\tnewName: String!",
    ] {
        assert!(sdl.contains(argument), "missing {argument:?} in:\n{sdl}");
    }

    let catalog = graphql_orm_semantic_catalog();
    catalog.validate().expect("catalogue validates");
    let arguments = |field: &str| {
        catalog
            .operations
            .iter()
            .find(|operation| operation.field_name == field)
            .unwrap_or_else(|| panic!("{field} operation"))
            .arguments
            .iter()
            .map(|argument| {
                (
                    argument.graphql_name.as_str(),
                    argument.description.as_str(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        arguments("countDocuments"),
        [
            ("folderKey", "Folder"),
            ("pageLimit", "Most documents to count."),
            ("minimumSize", "Minimum"),
            ("offset", "Offset"),
        ]
    );
    assert_eq!(
        arguments("renameFolder"),
        [("folderKey", "Folder"), ("newName", "Name")]
    );
}

#[test]
fn malformed_or_unknown_attribute_options_are_rejected() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/graphql_attribute_composition_pass.rs");
    cases.compile_fail("tests/ui/graphql_complex_malformed.rs");
    cases.compile_fail("tests/ui/custom_argument_graphql_malformed.rs");
    cases.compile_fail("tests/ui/field_owned_attribute_unknown.rs");
}
