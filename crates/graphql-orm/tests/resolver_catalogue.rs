#![cfg(feature = "sqlite")]
//! The resolver catalogue (#544): relation fields and Federation entity
//! resolvers, which have no root coordinate and so appear in no operation
//! catalogue.
//!
//! It names only what the schema's SDL already publishes, holds no scope
//! name, and leaves every other catalogue and fingerprint unchanged.
//! `referenced_field_read_authority` checks its authorization references
//! against executed traversals.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::graphql::orm::{
    GRAPHQL_RESOLVER_CATALOG_VERSION, GraphqlOperationKind, GraphqlResolverAuthorization,
    GraphqlResolverCatalog, GraphqlSemanticOperationSource, GraphqlSemanticRelationshipCardinality,
};
use graphql_orm::prelude::*;

mod catalogued {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    /// Two keys and a global identity; fixed scopes on both traversal shapes.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "cat_ledgers",
        plural = "CatLedgers",
        auth = "none",
        federation_key,
        federation_key(fields = ["code"]),
        node_id
    )]
    #[graphql_orm(
        operation_authorization(categories = ["list"], all_scopes = ["cat.ledgers.list"]),
        operation_authorization(categories = ["single_read"], all_scopes = ["cat.ledgers.read"])
    )]
    pub struct CatLedger {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[unique]
        #[filterable(type = "string")]
        pub code: String,
        #[filterable(type = "string")]
        pub owner_id: String,
    }
    impl BatchLoadEntity for CatLedger {
        fn batch_column() -> &'static str {
            "owner_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "owner_id")
        }
    }

    /// The single-read scope binds a root argument, which neither a
    /// traversal nor a representation supplies under that name.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "cat_sealed",
        plural = "CatSealedRows",
        auth = "none",
        federation_key
    )]
    #[graphql_orm(operation_authorization(
        categories = ["single_read"],
        any_scope_templates = [["cat.sealed.{id}.read"], ["cat.sealed.admin"]]
    ))]
    pub struct CatSealed {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }

    /// No single-read root, so nothing to refer to for it.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "cat_listed",
        plural = "CatListedRows",
        auth = "required",
        query_shapes = "list",
        federation_key
    )]
    pub struct CatListed {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub owner_id: String,
    }
    impl BatchLoadEntity for CatListed {
        fn batch_column() -> &'static str {
            "owner_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "owner_id")
        }
    }

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex)]
    #[graphql_entity(
        backend = "sqlite",
        table = "cat_owners",
        plural = "CatOwners",
        auth = "none"
    )]
    pub struct CatOwner {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub ledger_id: String,
        pub sealed_id: String,
        pub listed_id: String,
        #[graphql(skip)]
        #[relation(
            target = "CatLedger",
            from = "id",
            to = "owner_id",
            multiple,
            emit_fk = false
        )]
        pub ledgers: Vec<CatLedger>,
        #[graphql(skip)]
        #[relation(target = "CatLedger", from = "ledger_id", to = "id", emit_fk = false)]
        pub ledger: Option<CatLedger>,
        #[graphql(skip)]
        #[relation(target = "CatSealed", from = "sealed_id", to = "id", emit_fk = false)]
        pub sealed: Option<CatSealed>,
        #[graphql(skip)]
        #[relation(
            target = "CatListed",
            from = "id",
            to = "owner_id",
            multiple,
            emit_fk = false
        )]
        pub listed_rows: Vec<CatListed>,
        #[graphql(skip)]
        #[relation(target = "CatListed", from = "listed_id", to = "id", emit_fk = false)]
        pub listed: Option<CatListed>,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [CatOwner, CatLedger, CatSealed, CatListed],
    }
}

/// The same schema with no key and no relation.
mod plain {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(backend = "sqlite", table = "cat_plain", plural = "CatPlainRows")]
    pub struct CatPlain {
        #[primary_key]
        #[sortable]
        pub id: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [CatPlain],
    }
}

fn resolvers() -> &'static GraphqlResolverCatalog {
    catalogued::graphql_orm_resolver_catalog()
}

fn same_as(root: &str) -> GraphqlResolverAuthorization {
    GraphqlResolverAuthorization::SameAsRoot {
        kind: GraphqlOperationKind::Query,
        field_name: root.to_owned(),
    }
}

async fn federation_sdl() -> String {
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    catalogued::schema_builder(database)
        .finish()
        .sdl_with_options(SDLExportOptions::new().federation())
}

/// The `@key(fields: "...")` sets of one type in a Federation SDL.
fn sdl_keys(sdl: &str, type_name: &str) -> Vec<Vec<String>> {
    let header = sdl
        .lines()
        .find(|line| line.starts_with(&format!("type {type_name} ")))
        .unwrap_or_else(|| panic!("{type_name} is not in the SDL"));
    header
        .split("@key(fields: \"")
        .skip(1)
        .map(|rest| {
            rest.split('"')
                .next()
                .unwrap()
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

#[test]
fn every_relation_field_is_listed_with_its_target_and_cardinality() {
    let resolvers = resolvers();
    assert_eq!(resolvers.version, GRAPHQL_RESOLVER_CATALOG_VERSION);
    let semantic = catalogued::graphql_orm_semantic_catalog();
    assert_eq!(resolvers.semantic_catalog_fingerprint, semantic.fingerprint);
    resolvers.validate_against(semantic).unwrap();

    let listed = resolvers
        .relations
        .iter()
        .map(|relation| {
            (
                relation.type_name.as_str(),
                relation.field_name.as_str(),
                relation.target.as_str(),
                relation.cardinality,
            )
        })
        .collect::<Vec<_>>();
    use GraphqlSemanticRelationshipCardinality::{Many, One};
    assert_eq!(
        listed,
        [
            ("CatOwner", "ledger", "CatLedger", One),
            ("CatOwner", "ledgers", "CatLedger", Many),
            ("CatOwner", "listed", "CatListed", One),
            ("CatOwner", "listedRows", "CatListed", Many),
            ("CatOwner", "sealed", "CatSealed", One),
        ]
    );
    for relation in &resolvers.relations {
        assert_eq!(relation.source, GraphqlSemanticOperationSource::Generated);
        assert!(!relation.has_field_policy);
        // A to-many field takes the generated connection arguments; their
        // types are in the semantic catalogue.
        assert_eq!(relation.arguments.is_empty(), relation.cardinality == One);
    }
}

#[test]
fn a_root_is_named_only_where_the_resolver_enforces_that_root_requirement() {
    let resolvers = resolvers();
    let authorization = |field: &str| {
        resolvers
            .relation("CatOwner", field)
            .unwrap_or_else(|| panic!("CatOwner.{field} is not catalogued"))
            .authorization
            .clone()
    };
    // Fixed scopes on the mapped category: the traversal enforces them.
    assert_eq!(authorization("ledgers"), same_as("catLedgers"));
    assert_eq!(authorization("ledger"), same_as("catLedger"));
    // No read category declared: the traversal enforces the auth mode only,
    // as the list root does.
    assert_eq!(authorization("listedRows"), same_as("catListedRows"));
    // The target exposes no single-read root.
    assert_eq!(
        authorization("listed"),
        GraphqlResolverAuthorization::SubgraphOnly
    );
    // Scope templates refuse a traversal; the root does not.
    assert_eq!(
        authorization("sealed"),
        GraphqlResolverAuthorization::SubgraphOnly
    );

    let entity = |type_name: &str| resolvers.entity(type_name).unwrap().authorization.clone();
    assert_eq!(entity("CatLedger"), same_as("catLedger"));
    assert_eq!(
        entity("CatSealed"),
        GraphqlResolverAuthorization::SubgraphOnly
    );
    assert_eq!(
        entity("CatListed"),
        GraphqlResolverAuthorization::SubgraphOnly
    );
    assert!(resolvers.entity("CatOwner").is_none());

    // Every reference resolves to one exposed root of the operation catalogue.
    let operations = catalogued::graphql_orm_operation_catalog();
    for authorization in resolvers
        .relations
        .iter()
        .map(|relation| &relation.authorization)
        .chain(
            resolvers
                .entities
                .iter()
                .map(|entity| &entity.authorization),
        )
    {
        if let GraphqlResolverAuthorization::SameAsRoot { kind, field_name } = authorization {
            assert!(
                operations.resolve(*kind, field_name).is_some(),
                "{field_name}"
            );
        }
    }
}

#[tokio::test]
async fn entity_keys_are_exactly_the_resolvable_keys_of_the_federation_sdl() {
    let sdl = federation_sdl().await;
    let resolvers = resolvers();
    for type_name in ["CatLedger", "CatSealed", "CatListed"] {
        let mut catalogued = resolvers
            .entity(type_name)
            .unwrap()
            .keys
            .iter()
            .map(|key| key.fields.clone())
            .collect::<Vec<_>>();
        let mut published = sdl_keys(&sdl, type_name);
        catalogued.sort();
        published.sort();
        assert_eq!(catalogued, published, "{type_name}");
    }
    assert_eq!(resolvers.entity("CatLedger").unwrap().keys.len(), 3);
    assert!(sdl_keys(&sdl, "CatOwner").is_empty());
    assert_eq!(resolvers.entities.len(), 3);
}

/// The document discloses nothing the SDL does not: every string in it is a
/// fixed keyword, a fingerprint, or a name in the served schema. No scope,
/// policy text, description, table or column appears.
#[tokio::test]
async fn the_export_names_only_what_the_sdl_publishes() {
    fn strings(value: &serde_json::Value, output: &mut Vec<String>) {
        match value {
            serde_json::Value::String(string) => output.push(string.clone()),
            serde_json::Value::Array(values) => {
                values.iter().for_each(|value| strings(value, output));
            }
            serde_json::Value::Object(members) => {
                members.values().for_each(|value| strings(value, output));
            }
            _ => {}
        }
    }
    let sdl = federation_sdl().await;
    let names = sdl
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .collect::<std::collections::BTreeSet<_>>();
    let resolvers = resolvers();
    let exported = resolvers.export_json().unwrap();
    let mut found = Vec::new();
    strings(&serde_json::from_str(&exported).unwrap(), &mut found);
    assert!(found.len() > 20);
    let keywords = [
        "generated",
        "one",
        "many",
        "query",
        "same_as_root",
        "subgraph_only",
    ];
    for string in found {
        assert!(
            keywords.contains(&string.as_str())
                || string == resolvers.fingerprint
                || string == resolvers.semantic_catalog_fingerprint
                || names.contains(string.as_str()),
            "{string:?} is not in the SDL"
        );
    }
    for undisclosed in [
        "cat.ledgers",
        "cat.sealed",
        "cat_ledgers",
        "owner_id",
        "ledger_id",
        "sqlite",
    ] {
        assert!(!exported.contains(undisclosed), "{undisclosed}");
    }
    // The scopes are present in the operation catalogue, so the check above
    // is not vacuous.
    let scopes = format!(
        "{:?}",
        catalogued::graphql_orm_operation_catalog()
            .resolve(GraphqlOperationKind::Query, "catLedgers")
            .unwrap()
            .authorization()
    );
    assert!(scopes.contains("cat.ledgers.list"));
}

#[test]
fn the_export_round_trips_and_an_unkeyed_schema_is_empty() {
    let resolvers = resolvers();
    let exported = resolvers.export_json().unwrap();
    let decoded = GraphqlResolverCatalog::from_export_json(&exported).unwrap();
    assert_eq!(&decoded, resolvers);
    decoded
        .validate_against(catalogued::graphql_orm_semantic_catalog())
        .unwrap();
    assert!(
        decoded
            .validate_against(plain::graphql_orm_semantic_catalog())
            .is_err()
    );

    let plain = plain::graphql_orm_resolver_catalog();
    assert!(plain.relations.is_empty());
    assert!(plain.entities.is_empty());
    plain
        .validate_against(plain::graphql_orm_semantic_catalog())
        .unwrap();
}

/// Composing the resolver catalogue reads the other catalogues and changes
/// none of them.
#[test]
fn composing_it_leaves_the_other_catalogues_unchanged() {
    let operations = catalogued::graphql_orm_operation_catalog();
    let semantic = catalogued::graphql_orm_semantic_catalog();
    let before = (
        operations.fingerprint().to_owned(),
        operations.authorization_fingerprint().to_owned(),
        operations.router_export_fingerprint().to_owned(),
        serde_json::to_vec(semantic).unwrap(),
    );
    let _ = resolvers();
    assert_eq!(operations.fingerprint(), before.0);
    assert_eq!(operations.authorization_fingerprint(), before.1);
    assert_eq!(operations.router_export_fingerprint(), before.2);
    assert_eq!(serde_json::to_vec(semantic).unwrap(), before.3);
    assert!(
        !operations
            .operations()
            .iter()
            .any(|operation| operation.field_name().starts_with('_')),
        "an entity resolver is not a catalogued root field"
    );
}
