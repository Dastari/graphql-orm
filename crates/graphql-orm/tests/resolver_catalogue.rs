#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
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
    GRAPHQL_RESOLVER_CATALOG_VERSION, GeneratedResolverFacts,
    GraphqlHandwrittenResolverRegistration, GraphqlOperationKind, GraphqlResolverAuthorization,
    GraphqlResolverCatalog, GraphqlResolverCatalogBindingV1,
    GraphqlSemanticRelationshipCardinality, ResolverEntitySource, compose_resolver_catalog,
};
use graphql_orm::prelude::*;

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{field_name, resolver_name};

mod catalogued {
    use graphql_orm::GraphQLSemanticObject;
    use graphql_orm::async_graphql::{ComplexObject, Object, SimpleObject};
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

    /// Declares a key and a relation, but its handwritten object is an
    /// unresolvable Federation reference and serves `ledger` as a string.
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex, unresolvable = "id")]
    #[graphql_entity(
        backend = "sqlite",
        table = "tbl_cat_stubs",
        plural = "CatStubs",
        auth = "none",
        federation_key
    )]
    pub struct CatStub {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub ledger_id: String,
        #[graphql(skip)]
        #[relation(target = "CatLedger", from = "ledger_id", to = "id", emit_fk = false)]
        pub ledger: Option<CatLedger>,
        pub owner_id: String,
        #[graphql(skip)]
        #[relation(target = "CatOwner", from = "owner_id", to = "id", emit_fk = false)]
        pub owner: Option<CatOwner>,
        pub sealed_id: String,
        #[graphql(skip)]
        #[relation(target = "CatSealed", from = "sealed_id", to = "id", emit_fk = false)]
        pub sealed: Option<CatSealed>,
    }

    #[cfg_attr(
        graphql_orm_field_case = "pascal",
        ComplexObject(rename_fields = "PascalCase")
    )]
    #[cfg_attr(not(graphql_orm_field_case = "pascal"), ComplexObject)]
    impl CatStub {
        /// Shares the declared relation's name, not its type.
        async fn ledger(&self) -> String {
            self.ledger_id.clone()
        }

        async fn owner(&self) -> Option<CatOwner> {
            self.owner.clone()
        }

        async fn sealed(&self) -> Option<CatSealed> {
            self.sealed.clone()
        }
    }

    /// A relation field without `#[graphql(skip)]`: the `SimpleObject`
    /// derive serves a plain getter for it, named `ledger` by async-graphql's
    /// own rule, beside the generated relation resolver. The object is
    /// composed, so the generated resolvers live on a wrapper type; on the
    /// entity itself the two Rust methods would not compile.
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
        table = "tbl_cat_loose",
        plural = "CatLooseRows",
        auth = "none"
    )]
    #[graphql_orm(compose_complex_object)]
    pub struct CatLoose {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub ledger_id: String,
        pub sealed_id: String,
        #[relation(target = "CatLedger", from = "ledger_id", to = "id", emit_fk = false)]
        pub ledger: Option<CatLedger>,
        #[graphql(skip)]
        #[relation(target = "CatSealed", from = "sealed_id", to = "id", emit_fk = false)]
        pub sealed: Option<CatSealed>,
    }

    #[graphql_complex_object]
    impl CatLoose {}

    /// A skipped base field with a derived getter named like a relation:
    /// async-graphql serves `ledger` from `ledger_label`.
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
        table = "tbl_cat_derived",
        plural = "CatDerivedRows",
        auth = "none"
    )]
    #[graphql_orm(compose_complex_object)]
    pub struct CatDerived {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub ledger_id: String,
        pub sealed_id: String,
        #[graphql(skip, owned, derived(name = "ledger", into = "String"))]
        pub ledger_label: String,
        #[graphql(skip)]
        #[relation(target = "CatLedger", from = "ledger_id", to = "id", emit_fk = false)]
        pub ledger: Option<CatLedger>,
        #[graphql(skip)]
        #[relation(target = "CatSealed", from = "sealed_id", to = "id", emit_fk = false)]
        pub sealed: Option<CatSealed>,
    }

    #[graphql_complex_object]
    impl CatDerived {}

    #[derive(SimpleObject, GraphQLSemanticObject)]
    pub struct Badge {
        pub label: String,
    }

    #[derive(SimpleObject)]
    pub struct ShadowBadge {
        pub badge: Badge,
    }

    #[derive(SimpleObject, GraphQLSemanticObject)]
    #[graphql(name = "ShadowBadge")]
    pub struct ShadowBadgeDeclaration {
        pub badge: Badge,
    }

    #[derive(SimpleObject, GraphQLSemanticObject)]
    pub struct UnservedBadge {
        pub badge: Badge,
    }

    #[derive(SimpleObject, GraphQLSemanticObject)]
    pub struct BadgeGroup {
        pub badge: Option<Badge>,
        pub shadow: Option<ShadowBadge>,
    }

    #[derive(Default)]
    pub struct BadgeQuery;

    #[graphql_orm::graphql_orm_root_fields]
    #[Object]
    impl BadgeQuery {
        async fn badge_group(&self) -> Option<BadgeGroup> {
            None
        }
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [
            CatOwner, CatLedger, CatSealed, CatListed, CatNote, CatStub, CatLoose, CatDerived,
        ],
        extra_query_types: [BadgeQuery],
        semantic_types: [Badge, BadgeGroup, ShadowBadgeDeclaration, UnservedBadge],
    }
}

/// Another schema whose types happen to carry the names of `catalogued`
/// entities. They are different Rust types.
mod shadow {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "tbl_shadow_ledgers",
        plural = "CatLedgers",
        auth = "none"
    )]
    pub struct CatLedger {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }
    impl BatchLoadEntity for CatLedger {
        fn batch_column() -> &'static str {
            "id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "id")
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
        table = "tbl_shadow_owners",
        plural = "CatOwners",
        auth = "none"
    )]
    pub struct CatOwner {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub ledger_id: String,
        #[graphql(skip)]
        #[relation(target = "CatLedger", from = "ledger_id", to = "id", emit_fk = false)]
        pub ledger: Option<CatLedger>,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [CatOwner, CatLedger],
    }
}

/// A schema whose semantic catalogue cannot compose: one handwritten
/// semantic type has more fields than the catalogue allows.
mod oversized {
    use graphql_orm::graphql::orm::{
        GraphqlEntitySemanticMetadata, GraphqlSemanticClassification, GraphqlSemanticExport,
        GraphqlSemanticFieldMetadata, GraphqlSemanticObjectMetadata, GraphqlSemanticTypeKind,
        GraphqlSemanticTypeRef,
    };
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(backend = "sqlite", table = "tbl_cat_small", plural = "CatSmallRows")]
    pub struct CatSmall {
        #[primary_key]
        #[sortable]
        pub id: String,
    }

    pub struct Oversized;

    impl GraphqlSemanticObjectMetadata for Oversized {
        fn graphql_semantic_object() -> &'static GraphqlEntitySemanticMetadata {
            static METADATA: std::sync::OnceLock<GraphqlEntitySemanticMetadata> =
                std::sync::OnceLock::new();
            METADATA.get_or_init(|| GraphqlEntitySemanticMetadata {
                entity_name: "Oversized".to_owned(),
                description: "An object with more fields than a catalogue entity may have."
                    .to_owned(),
                default_classification: GraphqlSemanticClassification::Internal,
                fields: (0..=2_048)
                    .map(|index| GraphqlSemanticFieldMetadata {
                        field_name: format!("f{index}"),
                        description: format!("Field {index}."),
                        type_ref: GraphqlSemanticTypeRef::named(
                            "String",
                            GraphqlSemanticTypeKind::Scalar,
                            false,
                        ),
                        selectable: true,
                        filter_operators: Vec::new(),
                        filter_input: None,
                        sortable: false,
                        groupable: false,
                        aggregate_operators: Vec::new(),
                        aggregate_value_kind: None,
                        relationship: None,
                        classification: GraphqlSemanticClassification::Internal,
                        export: GraphqlSemanticExport::Exportable,
                        has_field_policy: false,
                    })
                    .collect(),
                generated_input_members: Vec::new(),
                label_fields: Vec::new(),
                record_version: None,
            })
        }
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [CatSmall],
        semantic_types: [Oversized],
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
        ("CatLoose", field_name("sealed"), "CatSealed", One),
    ];
    // `CatLoose.ledger` is listed only where no getter takes its name.
    if field_name("ledger") != "ledger" {
        expected.push(("CatLoose", field_name("ledger"), "CatLedger", One));
    }
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

/// A Federation reference the subgraph marks unresolvable is not an entity
/// it resolves, whatever keys are registered for it; and a handwritten field
/// that only shares a declared relation's name is not that relationship.
#[tokio::test]
async fn an_unresolvable_type_and_a_same_named_field_of_another_type_are_not_listed() {
    let sdl = federation_sdl().await;
    let header = sdl
        .lines()
        .find(|line| line.starts_with("type CatStub "))
        .expect("CatStub is served");
    // The registered keys include the declared one, and the type is marked
    // unresolvable.
    assert!(header.contains("resolvable: false"), "{header}");
    assert!(
        sdl_keys(&sdl, "CatStub").contains(&vec![field_name("id").to_owned()]),
        "the declared key is registered: {header}"
    );
    let start = sdl.find("type CatStub ").unwrap();
    let stub = &sdl[start..start + sdl[start..].find("\n}").unwrap()];
    let ledger = field_name("ledger");
    assert!(
        stub.lines()
            .any(|line| line.trim() == format!("{ledger}: String!")),
        "{stub}"
    );
    // The semantic catalogue still describes the declared relationship.
    let semantic = catalogued::graphql_orm_semantic_catalog();
    let declared = semantic
        .entities
        .iter()
        .find(|entity| entity.entity_name == "CatStub")
        .unwrap()
        .fields
        .iter()
        .find(|field| field.field_name == ledger)
        .expect("the declared relation is catalogued");
    assert_eq!(declared.relationship.as_ref().unwrap().target, "CatLedger");

    let resolvers = resolvers();
    assert!(resolvers.entity("CatStub").is_none());
    assert!(resolvers.relation("CatStub", ledger).is_none());
    assert!(
        resolvers
            .relations
            .iter()
            .all(|relation| relation.type_name != "CatStub" && relation.target != "CatStub")
    );
}

/// A generated relation whose name a `SimpleObject` getter also serves is
/// not listed: which of the two the schema serves is not the ORM's choice,
/// so nothing claims the generated resolver's requirement for it. The getter
/// is named by async-graphql's own rule, so the two collide only where the
/// field rule gives the relation the same name.
#[tokio::test]
async fn a_relation_shadowed_by_a_simple_object_getter_is_not_listed() {
    let sdl = federation_sdl().await;
    let start = sdl.find("type CatLoose ").expect("CatLoose is served");
    let loose = &sdl[start..start + sdl[start..].find("\n}").unwrap()];
    assert!(
        loose.lines().any(|line| line.trim().starts_with("ledger:")),
        "the getter is served: {loose}"
    );
    let relation = field_name("ledger");
    let shadowed = relation == "ledger";
    assert_eq!(
        catalogued::CatLoose::__gom_shadowed_relation_fields().contains(&relation),
        shadowed
    );
    assert_eq!(
        catalogued::CatLoose::__gom_mounted_relation_fields().contains(&relation),
        !shadowed
    );
    let resolvers = resolvers();
    assert_eq!(resolvers.relation("CatLoose", relation).is_none(), shadowed);
    // The semantic catalogue still describes the declared relationship, and
    // the skipped relation beside it is listed.
    assert!(
        catalogued::graphql_orm_semantic_catalog()
            .entities
            .iter()
            .find(|entity| entity.entity_name == "CatLoose")
            .unwrap()
            .fields
            .iter()
            .any(|field| field.field_name == relation && field.relationship.is_some())
    );
    let sealed = resolvers
        .relation("CatLoose", field_name("sealed"))
        .expect("the skipped relation is listed");
    assert_eq!(sealed.target, "CatSealed");
}

/// A `derived(...)` getter is served under its own name even when its base
/// field is skipped. The relations derive does not compute those names, so
/// an entity that declares one has no relation in the mount witness: neither
/// the relation the getter shadows nor the one beside it is listed.
#[tokio::test]
async fn relations_of_an_entity_with_a_derived_getter_are_not_listed() {
    let sdl = federation_sdl().await;
    let start = sdl.find("type CatDerived ").expect("CatDerived is served");
    let derived = &sdl[start..start + sdl[start..].find("\n}").unwrap()];
    assert!(
        derived
            .lines()
            .any(|line| line.trim().starts_with("ledger:")),
        "the derived getter is served: {derived}"
    );
    assert!(catalogued::CatDerived::__gom_mounted_relation_fields().is_empty());
    let mut shadowed = catalogued::CatDerived::__gom_shadowed_relation_fields().to_vec();
    shadowed.sort();
    let mut declared = vec![field_name("ledger"), field_name("sealed")];
    declared.sort();
    assert_eq!(shadowed, declared);

    let resolvers = resolvers();
    assert!(
        resolvers
            .relation("CatDerived", field_name("ledger"))
            .is_none()
    );
    assert!(
        resolvers
            .relations
            .iter()
            .all(|relation| relation.type_name != "CatDerived")
    );
    // The semantic catalogue still describes both declared relationships.
    let semantic = catalogued::graphql_orm_semantic_catalog()
        .entities
        .iter()
        .find(|entity| entity.entity_name == "CatDerived")
        .unwrap();
    assert_eq!(
        semantic
            .fields
            .iter()
            .filter(|field| field.relationship.is_some())
            .count(),
        2
    );
}

/// An entity the schema does not register is not matched to another type
/// that carries its name. `shadow` serves `CatOwner.ledger: CatLedger`, with
/// the names and shape the `catalogued` semantic catalogue describes, but
/// its types are not the catalogued entities.
#[tokio::test]
async fn an_unregistered_entity_is_not_matched_to_a_same_named_type() {
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let sdl = shadow::schema_builder(database).finish().sdl();
    let start = sdl
        .find("type CatOwner ")
        .expect("shadow serves a CatOwner");
    let owner = &sdl[start..start + sdl[start..].find("\n}").unwrap()];
    let ledger = field_name("ledger");
    assert!(
        owner
            .lines()
            .any(|line| line.trim() == format!("{ledger}: CatLedger")),
        "{owner}"
    );

    let sources = vec![
        ResolverEntitySource::of::<catalogued::CatOwner>(
            catalogued::CatOwner::__GOM_RESOLVER_FACTS,
            catalogued::CatOwner::__gom_mounted_relation_fields(),
        ),
        ResolverEntitySource::of::<catalogued::CatLedger>(
            catalogued::CatLedger::__GOM_RESOLVER_FACTS,
            &[],
        ),
    ];
    let semantic = catalogued::graphql_orm_semantic_catalog();
    let resolvers = compose_resolver_catalog::<shadow::QueryRoot>(semantic, sources).unwrap();
    assert!(resolvers.entities.is_empty());
    assert!(resolvers.relations.iter().all(|relation| {
        !["CatOwner", "PublicOwner", "CatLedger"].contains(&relation.type_name.as_str())
            && !["CatOwner", "PublicOwner", "CatLedger"].contains(&relation.target.as_str())
    }));

    // The same schema, catalogued as itself, lists that relation.
    let own = shadow::graphql_orm_resolver_catalog().unwrap();
    let relation = own.relation("CatOwner", ledger).expect("its own relation");
    assert_eq!(relation.target, "CatLedger");
}

/// When the semantic catalogue itself cannot compose, the helper returns
/// that error instead of panicking. The semantic accessor keeps its
/// signature, and still panics.
#[test]
fn a_semantic_catalogue_that_cannot_compose_is_an_error_not_a_panic() {
    for _ in 0..2 {
        let error = oversized::graphql_orm_resolver_catalog().expect_err("over the field limit");
        assert_eq!(error.to_string(), "semantic entity has too many fields");
    }
    assert!(std::panic::catch_unwind(oversized::graphql_orm_semantic_catalog).is_err());
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
    let keywords = ["one", "many", "query", "same_as_root", "subgraph_only"];
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
fn schema_root_registers_only_explicit_handwritten_relationships() {
    let cached = resolvers();
    let cached_export = cached.export_json().unwrap();
    let owner = field_name("owner");
    assert!(cached.relation("CatStub", owner).is_none());

    let register = |field, target| {
        GraphqlHandwrittenResolverRegistration::new(
            "CatStub",
            field,
            target,
            GraphqlResolverAuthorization::SubgraphOnly,
        )
    };
    let compose = catalogued::graphql_orm_resolver_catalog_with_registrations;
    let registered = compose([register(owner, "CatOwner")]).unwrap();
    let relation = registered.relation("CatStub", owner).unwrap();
    assert_eq!(relation.target, "PublicOwner");
    assert_eq!(relation.semantic_target.as_deref(), Some("CatOwner"));
    assert_eq!(
        relation.authorization,
        GraphqlResolverAuthorization::SubgraphOnly
    );
    assert!(
        registered
            .relation("CatStub", field_name("ledger"))
            .is_none()
    );
    assert_eq!(registered.relations.len(), cached.relations.len() + 1);
    assert_eq!(registered.entities, cached.entities);

    let export = registered.export_json().unwrap();
    let binding = GraphqlResolverCatalogBindingV1::new(&registered.fingerprint).unwrap();
    let semantic = catalogued::graphql_orm_semantic_catalog();
    assert_eq!(
        binding.decode_export_json(&export, semantic).unwrap(),
        registered
    );
    assert_eq!(
        catalogued::graphql_orm_resolver_catalog_with_registrations([])
            .unwrap()
            .export_json()
            .unwrap(),
        cached_export,
    );
    assert!(compose([register(field_name("ledger"), "CatLedger")]).is_err());
    assert!(
        registered
            .relation("CatStub", field_name("sealed"))
            .is_none()
    );
    let register_as_root = |field, target, root| {
        GraphqlHandwrittenResolverRegistration::new(
            "CatStub",
            field,
            target,
            GraphqlResolverAuthorization::SameAsRoot {
                kind: GraphqlOperationKind::Query,
                field_name: resolver_name(root).into(),
            },
        )
    };
    assert!(compose([register_as_root(owner, "CatOwner", "catOwner")]).is_ok());
    assert!(
        compose([register_as_root(
            field_name("sealed"),
            "CatSealed",
            "catSealed"
        )])
        .is_err()
    );
    assert!(compose([register(field_name("sealed"), "CatSealed")]).is_ok());
    assert_eq!(resolvers().export_json().unwrap(), cached_export);
}

#[test]
fn schema_root_requires_the_declared_rust_type_for_handwritten_relationships() {
    let compose = catalogued::graphql_orm_resolver_catalog_with_registrations;
    let register = |owner, field, target| {
        GraphqlHandwrittenResolverRegistration::new(
            owner,
            field,
            target,
            GraphqlResolverAuthorization::SubgraphOnly,
        )
    };
    assert!(resolvers().relation("BadgeGroup", "badge").is_none());
    let registered = compose([register("BadgeGroup", "badge", "Badge")]).unwrap();
    assert_eq!(
        registered.relation("BadgeGroup", "badge").unwrap().target,
        "Badge"
    );
    assert!(compose([register("ShadowBadge", "badge", "Badge")]).is_err());
    assert!(compose([register("BadgeGroup", "shadow", "ShadowBadge")]).is_err());
    assert!(compose([register("UnservedBadge", "badge", "Badge")]).is_err());
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
