use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "read_notes", plural = "ReadNotes", read_policy = "notes.read")]
struct ReadNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "written_notes", plural = "WrittenNotes", write_policy = "notes.write")]
struct WrittenNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

// Deliberately open to every subject the schema admits.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "open_tags", plural = "OpenTags", policy = "open")]
struct OpenTag {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

// Explicitly public.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "public_pages", plural = "PublicPages", auth = "none")]
struct PublicPage {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

mod required {
    use super::*;
    schema_roots! {
        backend: "sqlite",
        require_policies: true,
        query_custom_ops: [],
        entities: [ReadNote, WrittenNote, OpenTag, PublicPage],
    }
}

mod undecided {
    use graphql_orm::prelude::*;

    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(backend = "sqlite", table = "plain_tags", plural = "PlainTags")]
    pub struct PlainTag {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }

    // Without the setting, and with it explicitly off, nothing changes.
    pub mod unset {
        use super::*;
        schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [PlainTag] }
    }
    pub mod off {
        use super::*;
        schema_roots! { backend: "sqlite", require_policies: false, query_custom_ops: [], entities: [PlainTag] }
    }
}

fn main() {
    use graphql_orm::graphql::orm::Entity;
    assert!(ReadNote::POLICY_DECISION_DECLARED);
    assert!(WrittenNote::POLICY_DECISION_DECLARED);
    assert!(OpenTag::POLICY_DECISION_DECLARED);
    assert!(PublicPage::POLICY_DECISION_DECLARED);
    assert!(!undecided::PlainTag::POLICY_DECISION_DECLARED);
    // The marker records a decision and nothing else.
    assert_eq!(OpenTag::metadata().read_policy, None);
    assert_eq!(OpenTag::metadata().write_policy, None);
}
