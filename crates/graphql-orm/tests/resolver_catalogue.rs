#![cfg(feature = "sqlite")]
//! The resolver catalogue (#544): relation fields and Federation entity
//! resolvers, which have no root coordinate and so appear in no operation
//! catalogue.
//!
//! It lists the coordinates the schema serves, under the names the schema
//! registers, holds no scope name, and leaves every other catalogue and
//! fingerprint unchanged. `referenced_field_read_authority` checks its
//! authorization references against executed traversals, and
//! `client_facing_metadata` checks that declared relations the schema does
//! not serve are left out.
//!
//! Names are spelled through `graphql_case`, so the file holds under every
//! naming rule.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::graphql::orm::{
    GRAPHQL_RESOLVER_CATALOG_VERSION, GeneratedResolverFacts, GraphqlOperationKind,
    GraphqlResolverAuthorization, GraphqlResolverCatalog, GraphqlSemanticOperationSource,
    GraphqlSemanticRelationshipCardinality, ResolverEntitySource, compose_resolver_catalog,
};
use graphql_orm::prelude::*;

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{field_name, resolver_name};

mod catalogued {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    /// Two keys and a global identity; fixed scopes on both traversal shapes.
    /// One key field is renamed.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "tbl_cat_ledgers",
        plural = "CatLedgers",
        auth = "none",
        federation_key,
        federation_key(fields = ["RefCode"]),
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
        #[graphql(name = "RefCode")]
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
        table = "tbl_cat_sealed",
        plural = "CatSealedRows",
        auth = "none",
        federation_key
    )]
    // The placeholder names the single-read argument as the argument case
    // spells it.
    #[cfg_attr(
        graphql_orm_argument_case = "pascal",
        graphql_orm(operation_authorization(
            categories = ["single_read"],
            any_scope_templates = [["cat.sealed.{Id}.read"], ["cat.sealed.admin"]]
        ))
    )]
    #[cfg_attr(
        any(
            graphql_orm_argument_case = "screaming_snake",
            graphql_orm_argument_case = "upper"
        ),
        graphql_orm(operation_authorization(
            categories = ["single_read"],
            any_scope_templates = [["cat.sealed.{ID}.read"], ["cat.sealed.admin"]]
        ))
    )]
    #[cfg_attr(
        not(any(
            graphql_orm_argument_case = "pascal",
            graphql_orm_argument_case = "screaming_snake",
            graphql_orm_argument_case = "upper"
        )),
        graphql_orm(operation_authorization(
            categories = ["single_read"],
            any_scope_templates = [["cat.sealed.{id}.read"], ["cat.sealed.admin"]]
        ))
    )]
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
        table = "tbl_cat_listed",
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

    /// Served under another type name than its Rust and catalogue name.
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
    #[graphql(complex, name = "PublicOwner")]
    #[graphql_entity(
        backend = "sqlite",
        table = "tbl_cat_owners",
        plural = "CatOwners",
        auth = "none",
        federation_key
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
    impl BatchLoadEntity for CatOwner {
        fn batch_column() -> &'static str {
            "id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "id")
        }
    }

    /// Points at the renamed type.
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
        table = "tbl_cat_notes",
        plural = "CatNotes",
        auth = "none"
    )]
    pub struct CatNote {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub owner_id: String,
        #[graphql(skip)]
        #[relation(target = "CatOwner", from = "owner_id", to = "id", emit_fk = false)]
        pub owner: Option<CatOwner>,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [CatOwner, CatLedger, CatSealed, CatListed, CatNote],
    }
}

/// A schema with no key and no relation.
mod plain {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(backend = "sqlite", table = "tbl_cat_plain", plural = "CatPlainRows")]
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

/// A composite key of 17 fields, one more than a relationship key mapping
/// may hold.
// The derive is expanded through a local macro, so Clippy attributes its
// generated 17-key functions to this file.
#[allow(clippy::too_many_arguments)]
mod wide {
    use graphql_orm::prelude::*;

    macro_rules! wide_entity {
        ($($field:ident),*) => {
            #[derive(
                GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize,
                serde::Deserialize,
            )]
            #[graphql_entity(
                backend = "sqlite",
                table = "tbl_cat_wide",
                plural = "CatWideRows",
                auth = "none",
                federation_key
            )]
            pub struct CatWide {
                $(
                    #[primary_key]
                    #[graphql_orm(auto_generated = false)]
                    #[sortable]
                    pub $field: String,
                )*
            }
        };
    }
    wide_entity!(
        k00, k01, k02, k03, k04, k05, k06, k07, k08, k09, k10, k11, k12, k13, k14, k15, k16
    );

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [CatWide],
    }
}

fn resolvers() -> &'static GraphqlResolverCatalog {
    catalogued::graphql_orm_resolver_catalog().expect("resolver catalogue")
}

fn same_as(root: &str) -> GraphqlResolverAuthorization {
    GraphqlResolverAuthorization::SameAsRoot {
        kind: GraphqlOperationKind::Query,
        field_name: resolver_name(root).to_owned(),
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
fn served_relation_fields_are_listed_under_their_registered_names() {
    let resolvers = resolvers();
    assert_eq!(resolvers.version, GRAPHQL_RESOLVER_CATALOG_VERSION);
    let semantic = catalogued::graphql_orm_semantic_catalog();
    assert_eq!(resolvers.semantic_catalog_fingerprint, semantic.fingerprint);
    resolvers.validate_against(semantic).unwrap();

    use GraphqlSemanticRelationshipCardinality::{Many, One};
    let mut expected = vec![
        ("CatNote", field_name("owner"), "PublicOwner", One),
        ("PublicOwner", field_name("ledger"), "CatLedger", One),
        ("PublicOwner", field_name("ledgers"), "CatLedger", Many),
        ("PublicOwner", field_name("listed"), "CatListed", One),
        ("PublicOwner", field_name("listedRows"), "CatListed", Many),
        ("PublicOwner", field_name("sealed"), "CatSealed", One),
    ];
    expected.sort_by_key(|(owner, field, _, _)| (*owner, *field));
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
    assert_eq!(
        listed
            .iter()
            .map(|(owner, field, target, _)| (*owner, *field, *target))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(owner, field, target, _)| (*owner, *field, *target))
            .collect::<Vec<_>>()
    );
    for (owner, field, _, cardinality) in &expected {
        let relation = resolvers.relation(owner, field).unwrap();
        assert_eq!(relation.cardinality, *cardinality);
        assert_eq!(relation.source, GraphqlSemanticOperationSource::Generated);
        assert!(!relation.has_field_policy);
        // A to-many field takes the generated connection arguments; their
        // types are in the semantic catalogue.
        assert_eq!(relation.arguments.is_empty(), *cardinality == One);
    }

    // The renamed type is listed under its registered name and carries its
    // catalogue name, as owner and as target.
    let ledgers = resolvers
        .relation("PublicOwner", field_name("ledgers"))
        .unwrap();
    assert_eq!(ledgers.semantic_entity_name.as_deref(), Some("CatOwner"));
    assert_eq!(ledgers.semantic_target, None);
    let owner = resolvers.relation("CatNote", field_name("owner")).unwrap();
    assert_eq!(owner.semantic_entity_name, None);
    assert_eq!(owner.semantic_target.as_deref(), Some("CatOwner"));
    assert!(
        resolvers
            .relation("CatOwner", field_name("ledgers"))
            .is_none()
    );
}

#[test]
fn a_root_is_named_only_where_the_resolver_enforces_that_root_requirement() {
    let resolvers = resolvers();
    let authorization = |field: &str| {
        resolvers
            .relation("PublicOwner", field_name(field))
            .unwrap_or_else(|| panic!("PublicOwner.{field} is not catalogued"))
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
    // The reference is to the root of the renamed target's catalogue entity.
    assert_eq!(
        resolvers
            .relation("CatNote", field_name("owner"))
            .unwrap()
            .authorization,
        same_as("catOwner")
    );

    let entity = |type_name: &str| resolvers.entity(type_name).unwrap().authorization.clone();
    assert_eq!(entity("CatLedger"), same_as("catLedger"));
    assert_eq!(entity("PublicOwner"), same_as("catOwner"));
    assert_eq!(
        entity("CatSealed"),
        GraphqlResolverAuthorization::SubgraphOnly
    );
    assert_eq!(
        entity("CatListed"),
        GraphqlResolverAuthorization::SubgraphOnly
    );
    assert!(resolvers.entity("CatOwner").is_none());
    assert!(resolvers.entity("CatNote").is_none());

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
    for type_name in ["CatLedger", "CatSealed", "CatListed", "PublicOwner"] {
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
    // The renamed field is a key under its registered name.
    let mut ledger = resolvers
        .entity("CatLedger")
        .unwrap()
        .keys
        .iter()
        .map(|key| key.fields.join(" "))
        .collect::<Vec<_>>();
    ledger.sort();
    let mut expected = vec![
        field_name("id").to_owned(),
        "RefCode".to_owned(),
        "NodeId".to_owned(),
    ];
    expected.sort();
    assert_eq!(ledger, expected);
    assert_eq!(
        resolvers
            .entity("PublicOwner")
            .unwrap()
            .semantic_entity_name
            .as_deref(),
        Some("CatOwner")
    );
    assert!(sdl_keys(&sdl, "CatNote").is_empty());
    assert!(!sdl.contains("type CatOwner "));
    assert_eq!(resolvers.entities.len(), 4);
}

/// Every coordinate in the document is in the served schema: each relation's
/// owning type has that field, and each listed type exists.
#[tokio::test]
async fn every_coordinate_is_in_the_served_schema() {
    let sdl = federation_sdl().await;
    let block = |type_name: &str| {
        let start = sdl
            .find(&format!("type {type_name} "))
            .unwrap_or_else(|| panic!("{type_name} is not served"));
        let end = sdl[start..].find("\n}").unwrap() + start;
        sdl[start..end].to_owned()
    };
    let resolvers = resolvers();
    for relation in &resolvers.relations {
        let served = block(&relation.type_name)
            .lines()
            .map(str::trim)
            .any(|line| {
                line.strip_prefix(relation.field_name.as_str())
                    .is_some_and(|rest| rest.starts_with(':') || rest.starts_with('('))
            });
        assert!(served, "{}.{}", relation.type_name, relation.field_name);
        let _ = block(&relation.target);
    }
    for entity in &resolvers.entities {
        let _ = block(&entity.type_name);
    }
}

/// An entity whose type the served schema does not resolve is not listed,
/// and neither are relations into or out of a type it does not serve. Here
/// the catalogued entities are composed against a query root that mounts
/// none of them.
#[test]
fn entities_the_schema_does_not_serve_are_not_listed() {
    type Keys = &'static [&'static [&'static str]];
    let facts = |keys: Keys| GeneratedResolverFacts {
        federation_keys: keys,
        to_many_root: None,
        to_one_root: None,
        entity_root: None,
    };
    let sources = || {
        vec![
            ResolverEntitySource::of::<catalogued::CatLedger>(facts(&[&["id"]]), &[]),
            ResolverEntitySource::of::<catalogued::CatOwner>(
                facts(&[&["id"]]),
                &["ledger", "ledgers"],
            ),
        ]
    };
    let semantic = catalogued::graphql_orm_semantic_catalog();
    let unserved = compose_resolver_catalog::<plain::QueryRoot>(semantic, sources()).unwrap();
    assert!(unserved.relations.is_empty());
    assert!(unserved.entities.is_empty());
    unserved.validate_against(semantic).unwrap();

    // Against its own root, a declared key the schema does not resolve is
    // dropped, and so is an entity left with none.
    let id_key: &'static [&'static str] = Box::leak(Box::new([field_name("id")]));
    let declared: Keys = Box::leak(Box::new([&["NotAKey"][..], id_key]));
    let served = compose_resolver_catalog::<catalogued::QueryRoot>(
        semantic,
        vec![
            ResolverEntitySource::of::<catalogued::CatLedger>(facts(declared), &[]),
            ResolverEntitySource::of::<catalogued::CatSealed>(facts(&[&["NotAKey"]]), &[]),
        ],
    )
    .unwrap();
    assert_eq!(served.entities.len(), 1);
    assert_eq!(served.entities[0].type_name, "CatLedger");
    assert_eq!(served.entities[0].keys.len(), 1);
}

/// The document discloses nothing new: every string in it is a fixed
/// keyword, a fingerprint, a name in the served schema, or an entity name
/// the bound semantic catalogue already publishes. No scope, policy text,
/// description, table or column appears.
#[tokio::test]
async fn the_export_names_only_what_the_schema_and_semantic_catalogue_publish() {
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
    let semantic = catalogued::graphql_orm_semantic_catalog();
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
                || names.contains(string.as_str())
                || semantic
                    .entities
                    .iter()
                    .any(|entity| entity.entity_name == string),
            "{string:?} is published nowhere else"
        );
    }
    for undisclosed in ["cat.ledgers", "cat.sealed", "tbl_cat", "sqlite"] {
        assert!(!exported.contains(undisclosed), "{undisclosed}");
    }
    // The scopes are present in the operation catalogue, so the check above
    // is not vacuous.
    let scopes = format!(
        "{:?}",
        catalogued::graphql_orm_operation_catalog()
            .resolve(GraphqlOperationKind::Query, resolver_name("catLedgers"))
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

    let plain = plain::graphql_orm_resolver_catalog().unwrap();
    assert!(plain.relations.is_empty());
    assert!(plain.entities.is_empty());
    plain
        .validate_against(plain::graphql_orm_semantic_catalog())
        .unwrap();
}

/// A composite key wider than 16 fields is catalogued, not refused: the
/// generated helper returns the document instead of panicking.
#[test]
fn a_seventeen_field_key_is_catalogued() {
    let resolvers = wide::graphql_orm_resolver_catalog().expect("a wide key is valid");
    let entity = resolvers.entity("CatWide").unwrap();
    assert_eq!(entity.keys.len(), 1);
    assert_eq!(entity.keys[0].fields.len(), 17);
    assert_eq!(entity.keys[0].fields[0], field_name("k00"));
    assert_eq!(entity.keys[0].fields[16], field_name("k16"));
    resolvers
        .validate_against(wide::graphql_orm_semantic_catalog())
        .unwrap();
    let decoded =
        GraphqlResolverCatalog::from_export_json(&resolvers.export_json().unwrap()).unwrap();
    assert_eq!(&decoded, resolvers);
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
