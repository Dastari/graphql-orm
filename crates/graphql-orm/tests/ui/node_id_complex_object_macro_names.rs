// Names that reach `graphql_complex_object` through `macro_rules!` fragments
// arrive wrapped in invisible groups; the shadowing guard still reads them.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

macro_rules! entity {
    ($name:ident, $table:literal, $plural:literal) => {
        #[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
        #[graphql(complex)]
        #[graphql_orm(compose_complex_object)]
        #[graphql_entity(backend = "sqlite", table = $table, plural = $plural, node_id)]
        struct $name {
            #[primary_key]
            #[sortable]
            id: String,
            name: String,
            #[graphql_orm(version = "managed", default = "0")]
            version: i64,
        }
    };
}

entity!(Named, "named_records", "NamedRows");
entity!(Renamed, "renamed_records", "RenamedRows");

macro_rules! named {
    ($field:expr) => {
        #[graphql_complex_object]
        impl Named {
            #[graphql(name = $field)]
            async fn handwritten(&self) -> i64 {
                self.version
            }
        }
    };
}

macro_rules! renamed {
    ($rule:expr) => {
        #[graphql_complex_object(rename_fields = $rule)]
        impl Renamed {
            #[allow(non_snake_case)]
            async fn NODeId(&self) -> String {
                self.id.clone()
            }
        }
    };
}

named!("RecordVersion");
renamed!("PascalCase");

fn main() {}
