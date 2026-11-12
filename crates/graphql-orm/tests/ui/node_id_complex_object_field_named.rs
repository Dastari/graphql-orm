// `SimpleObject` resolves struct and derived fields before complex fields, so
// none of them may be published under a generated identity field's name.
// Names follow async-graphql: an explicit `name` names the field and its
// derived copies, otherwise `rename_fields` (camelCase by default) renames the
// identifier, and derived copies exist even when the base field is skipped.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

macro_rules! entity {
    ($name:ident, $table:literal, $plural:literal, [$($rename:tt)*], { $($fields:tt)* }) => {
        #[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
        #[graphql(complex $($rename)*)]
        #[graphql_orm(compose_complex_object)]
        #[graphql_entity(backend = "sqlite", table = $table, plural = $plural, node_id)]
        struct $name {
            #[primary_key]
            #[sortable]
            id: String,
            $($fields)*
        }
    };
}

// A plain field renamed to `NodeId`.
entity!(FieldNamed, "field_records", "FieldNamedRows", [, rename_fields = "PascalCase"], {
    node_id: String,
});

#[graphql_complex_object(rename_fields = "PascalCase")]
impl FieldNamed {
    async fn label(&self) -> String {
        self.id.clone()
    }
}

fn main() {}
