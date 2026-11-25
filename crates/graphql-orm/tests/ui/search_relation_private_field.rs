use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "ui_labels", plural = "UiLabels")]
struct UiLabel {
    #[primary_key]
    #[sortable]
    id: String,
    note_id: String,
    #[graphql_orm(private)]
    label: String,
    #[graphql_orm(read = false)]
    hidden: String,
    #[input_only]
    input: String,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "ui_notes", plural = "UiNotes")]
#[graphql_orm(search(index = true))]
struct UiNote {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
    #[graphql_orm(search_relation(fields = ["label", "hidden", "input"]))]
    #[relation(target = "UiLabel", from = "id", to = "note_id", multiple, emit_fk = false)]
    labels: Vec<UiLabel>,
}

fn main() {}
