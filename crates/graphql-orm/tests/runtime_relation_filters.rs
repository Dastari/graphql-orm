//! Runtime relation filters (correlated EXISTS with the target grant applied
//! inside) and to-one relation selections in mutation payloads. Each PostgreSQL
//! case owns a verified disposable server.
#![cfg(all(
    feature = "runtime-graphql",
    any(
        all(feature = "sqlite", not(feature = "postgres")),
        all(feature = "postgres", not(feature = "sqlite"))
    )
))]

use futures::future::BoxFuture;
use graphql_orm::async_graphql::{Request, Variables, dynamic::Schema};
use graphql_orm::{
    db::Database,
    graphql::{orm::*, runtime::*},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;

fn cid(s: &str) -> CollectionId {
    CollectionId::new(s).unwrap()
}
fn fid(s: &str) -> FieldId {
    FieldId::new(s).unwrap()
}
fn field(id: &str, name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: fid(id),
        api_name: name.into(),
        physical_column: name.to_lowercase(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
    }
}
fn collection(id: &str, name: &str, plural: &str, fields: Vec<RuntimeField>) -> RuntimeCollection {
    RuntimeCollection {
        description: None,
        classification: None,
        id: cid(id),
        api_type_name: name.into(),
        api_plural_name: plural.into(),
        physical_table: format!("rf_{id}"),
        primary_key: vec![fid(&format!("{id}.id"))],
        append_only: false,
        retention_purge: false,
        fields,
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        label_fields: Vec::new(),
    }
}
fn relation(id: &str, target: &str, pairs: &[(&str, &str)], many: bool) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(id).unwrap(),
        api_name: id.into(),
        target: cid(target),
        key_pairs: pairs
            .iter()
            .map(|(source, target)| RelationKeyPair {
                source: fid(source),
                target: fid(target),
            })
            .collect(),
        cardinality: if many {
            RelationCardinality::Many
        } else {
            RelationCardinality::One
        },
        enforce_foreign_key: false,
        on_delete: None,
        join: None,
    }
}
fn schema() -> Arc<ValidatedRuntimeSchema> {
    use RuntimeValueKind::{Integer, String};
    let mut parent = collection(
        "parent",
        "Parent",
        "Parents",
        vec![
            field("parent.id", "id", Integer),
            field("parent.tenant", "tenant", String),
            field("parent.label", "label", String),
            RuntimeField {
                nullable: true,
                ..field("parent.manager", "managerId", Integer)
            },
        ],
    );
    let mut child = collection(
        "child",
        "Child",
        "Children",
        vec![
            field("child.id", "id", Integer),
            field("child.tenant", "tenant", String),
            field("child.owner", "owner", Integer),
            field("child.label", "label", String),
            RuntimeField {
                nullable: true,
                ..field("child.note", "note", String)
            },
        ],
    );
    child.relations.push(relation(
        "parent",
        "parent",
        &[
            ("child.owner", "parent.id"),
            ("child.tenant", "parent.tenant"),
        ],
        false,
    ));
    // A direct self-reference.
    parent.relations.push(relation(
        "manager",
        "parent",
        &[("parent.manager", "parent.id")],
        false,
    ));
    parent.relations.push(relation(
        "children",
        "child",
        &[
            ("parent.id", "child.owner"),
            ("parent.tenant", "child.tenant"),
        ],
        true,
    ));
    Arc::new(
        RuntimeSchema {
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: vec![parent, child],
        }
        .validate()
        .unwrap(),
    )
}

/// Every layer sees `tenant = north AND label <> 'secret'`. Relation filter
/// targets of `deny_filter_target` are denied. Every check is recorded.
struct Authority {
    deny_filter_target: Option<&'static str>,
    hide_secret: bool,
    checks: Mutex<Vec<String>>,
    /// Field IDs the caller may not filter on, including through relation keys.
    deny_fields: &'static [&'static str],
    /// Selected relation layers of this collection are denied outright.
    deny_select_relation: Option<&'static str>,
}
impl Authority {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            deny_filter_target: None,
            hide_secret: true,
            checks: Mutex::new(Vec::new()),
            deny_fields: &[],
            deny_select_relation: None,
        })
    }
    fn checks(&self) -> Vec<String> {
        self.checks.lock().unwrap().clone()
    }
}
impl RuntimeReadAuthority for Authority {
    fn authorize_mutation_result<'a>(
        &'a self,
        _check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async move {
            let collection = check.collection.id().as_str().to_owned();
            self.checks.lock().unwrap().push(format!(
                "{collection}:{:?}:{}:{}:[{}]:[{}]",
                check.purpose,
                check.relation.map_or("-", |r| r.id().as_str()),
                check.projection.len(),
                check
                    .filter_operations
                    .iter()
                    .map(|(f, op)| format!("{}.{op}", f.id()))
                    .collect::<Vec<_>>()
                    .join(","),
                check
                    .relation_filters
                    .iter()
                    .map(|(r, q)| format!("{}.{}", r.id(), q.as_str()))
                    .collect::<Vec<_>>()
                    .join(","),
            ));
            if matches!(check.purpose, RuntimeReadPurpose::RelationFilter(_))
                && self.deny_filter_target == Some(collection.as_str())
            {
                return Err(RuntimeGraphqlError::denied());
            }
            if check
                .filter_operations
                .iter()
                .any(|(f, _)| self.deny_fields.contains(&f.id().as_str()))
                || (check.purpose == RuntimeReadPurpose::Select
                    && check.relation.is_some()
                    && self.deny_select_relation == Some(collection.as_str()))
            {
                return Err(RuntimeGraphqlError::denied());
            }
            let field = |name: &str| {
                check
                    .schema
                    .resolve_field(check.collection, &fid(&format!("{collection}.{name}")))
                    .unwrap()
            };
            let limits = RuntimeQueryLimits::default();
            let mut parts = vec![
                check
                    .schema
                    .runtime_compare(
                        check.collection,
                        &field("tenant"),
                        RuntimeScalarOperator::Eq,
                        RuntimeValue::String("north".into()),
                        limits,
                    )
                    .unwrap(),
            ];
            if self.hide_secret {
                parts.push(
                    check
                        .schema
                        .runtime_compare(
                            check.collection,
                            &field("label"),
                            RuntimeScalarOperator::Ne,
                            RuntimeValue::String("secret".into()),
                            limits,
                        )
                        .unwrap(),
                );
            }
            Ok(RuntimeReadGrant::new(Some(
                check
                    .schema
                    .runtime_and(check.collection, parts, limits)
                    .unwrap(),
            )))
        })
    }
}

struct Allow(Arc<ValidatedRuntimeSchema>);
impl RuntimeWriteAuthority<Backend> for Allow {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            let handle = check.collection();
            let fields = self
                .0
                .schema()
                .collections
                .iter()
                .find(|c| c.id == *handle.id())
                .unwrap()
                .fields
                .iter()
                .map(|f| self.0.resolve_field(handle, &f.id).unwrap())
                .collect::<Vec<_>>();
            RuntimeWriteGrant::new(self.0.resolve_projection(handle, &fields).unwrap(), None)
        })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _: RuntimePreimageCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
    }
}

struct Fixture {
    db: Database<Backend>,
    schema: Arc<ValidatedRuntimeSchema>,
    environment: Arc<RuntimeMutationEnvironment>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn new() -> Self {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            "sqlite::memory:",
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("relation-filters").unwrap();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        let schema = schema();
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership =
            ManagedTableSet::new(["rf_parent".to_owned(), "rf_child".to_owned()]).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "relation filters",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
        db.schema()
            .apply_owned_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema.clone(), &target, &ownership)
                .await
                .unwrap(),
        );
        for sql in [
            "INSERT INTO rf_parent (id, tenant, label, managerid) VALUES (1, 'north', 'first', NULL), (2, 'north', 'second', 1), (3, 'south', 'private', 1), (4, 'north', 'secret', 2)",
            "INSERT INTO rf_child (id, tenant, owner, label) VALUES (11, 'north', 1, 'a'), (12, 'north', 1, 'b'), (13, 'north', 1, 'secret'), (21, 'north', 2, 'c'), (22, 'north', 2, 'd'), (31, 'south', 3, 'c'), (41, 'north', 99, 'orphan'), (51, 'north', 4, 'x')",
        ] {
            Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
        }
        Self {
            db,
            schema,
            environment,
            #[cfg(feature = "postgres")]
            owned,
        }
    }
    fn api(&self, relation_filters: bool) -> Schema {
        self.api_with(|options| {
            if relation_filters {
                options.with_relation_filters()
            } else {
                options
            }
        })
    }
    fn api_with(
        &self,
        configure: impl FnOnce(RuntimeGraphqlOptions) -> RuntimeGraphqlOptions,
    ) -> Schema {
        let options = configure(
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default()),
        );
        RuntimeGraphqlComposer::new(self.db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(RuntimeGraphqlModule::compile(self.schema.clone(), options).unwrap())
            .unwrap()
            .finish()
            .unwrap()
    }
    async fn run(
        &self,
        api: &Schema,
        authority: Arc<Authority>,
        limits: RuntimeGraphqlLimits,
        query: &str,
        variables: Value,
    ) -> Value {
        let mut variables = variables;
        variables["schema"] = json!(self.schema.fingerprint().to_string());
        let response = api
            .execute(
                Request::new(query)
                    .variables(Variables::from_json(variables))
                    .data(
                        RuntimeGraphqlRequest::<Backend>::new(
                            self.schema.fingerprint(),
                            authority,
                            None,
                            limits,
                        )
                        .with_mutation_environment(self.environment.clone())
                        .with_write_authority(Arc::new(Allow(self.schema.clone()))),
                    ),
            )
            .await;
        serde_json::to_value(&response).unwrap()
    }
    /// Ids of a root connection for a `where` input, ordered by id.
    async fn ids(
        &self,
        api: &Schema,
        authority: Arc<Authority>,
        root: &str,
        filter: Value,
    ) -> Vec<String> {
        let object = if root == "parents" { "Parent" } else { "Child" };
        let response = self
            .run(
                api,
                authority,
                Default::default(),
                &format!(
                    "query($where: {object}WhereInput) {{ {root}(where: $where, orderBy: [{{field: id}}]) {{ edges {{ node {{ id }} }} }} }}"
                ),
                json!({ "where": filter }),
            )
            .await;
        assert!(response.get("errors").is_none(), "{response}");
        response["data"][root]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
            .collect()
    }
    async fn sql(&self, sql: &str) {
        Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
    }
    async fn count(&self, sql: &str) -> i64 {
        let rows = Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
    }
    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

#[tokio::test]
async fn relation_filters_lower_to_exists_with_target_grants_inside() {
    let f = Fixture::new().await;
    let api = f.api(true);
    let sdl = api.sdl();
    assert!(sdl.contains("input ChildToManyFilter"));
    assert!(sdl.contains("input ParentToOneFilter"));
    let ids = |root: &'static str, filter: Value| {
        let authority = Authority::new();
        let f = &f;
        let api = &api;
        async move { f.ids(api, authority, root, filter).await }
    };

    // To-many quantifiers. Parent 4 and child 13 are hidden by the grants.
    assert_eq!(
        ids(
            "parents",
            json!({"children": {"some": {"label": {"eq": "c"}}}})
        )
        .await,
        ["2"]
    );
    assert_eq!(
        ids(
            "parents",
            json!({"children": {"none": {"label": {"eq": "a"}}}})
        )
        .await,
        ["2"]
    );
    assert_eq!(
        ids(
            "parents",
            json!({"children": {"every": {"label": {"in": ["c", "d"]}}}})
        )
        .await,
        ["2"]
    );
    // A related row hidden by the target grant never matches, even when it would.
    assert!(
        ids(
            "parents",
            json!({"children": {"some": {"label": {"eq": "secret"}}}})
        )
        .await
        .is_empty()
    );
    assert_eq!(
        ids(
            "parents",
            json!({"children": {"every": {"label": {"in": ["a", "b"]}}}})
        )
        .await,
        ["1"]
    );
    let unrestricted = Arc::new(Authority {
        deny_filter_target: None,
        hide_secret: false,
        checks: Mutex::new(Vec::new()),
        deny_fields: &[],
        deny_select_relation: None,
    });
    assert_eq!(
        f.ids(
            &api,
            unrestricted,
            "parents",
            json!({"children": {"some": {"label": {"eq": "secret"}}}})
        )
        .await,
        ["1"]
    );

    // To-one quantifiers; the orphan has no parent and child 51's parent is hidden.
    assert_eq!(
        ids(
            "children",
            json!({"parent": {"is": {"label": {"eq": "second"}}}})
        )
        .await,
        ["21", "22"]
    );
    assert_eq!(
        ids(
            "children",
            json!({"parent": {"isNot": {"label": {"eq": "first"}}}})
        )
        .await,
        ["21", "22", "41", "51"]
    );
    assert!(
        ids(
            "children",
            json!({"parent": {"is": {"label": {"eq": "secret"}}}})
        )
        .await
        .is_empty()
    );

    // Composition with scalar filters, Boolean operators and nested relations.
    assert_eq!(
        ids(
            "children",
            json!({"label": {"ne": "b"}, "parent": {"is": {"children": {"some": {"label": {"eq": "a"}}}}}})
        )
        .await,
        ["11"]
    );
    assert_eq!(
        ids(
            "children",
            json!({"or": [{"label": {"eq": "orphan"}}, {"parent": {"is": {"label": {"eq": "second"}}}}]})
        )
        .await,
        ["21", "22", "41"]
    );
    assert_eq!(
        ids(
            "children",
            json!({"not": {"parent": {"is": {"label": {"startsWith": "s"}}}}})
        )
        .await,
        ["11", "12", "41", "51"]
    );
    f.finish().await;
}

#[tokio::test]
async fn target_layers_are_authorized_and_denied_targets_never_match() {
    let f = Fixture::new().await;
    let api = f.api(true);
    let authority = Authority::new();
    assert_eq!(
        f.ids(
            &api,
            authority.clone(),
            "parents",
            json!({"label": {"ne": "zzz"}, "children": {"some": {"label": {"eq": "c"}, "parent": {"is": {"label": {"eq": "second"}}}}}})
        )
        .await,
        ["2"]
    );
    assert_eq!(
        authority.checks(),
        [
            "parent:Select:-:1:[parent.id.relation,parent.tenant.relation,parent.label.ne]:[children.some]",
            "child:RelationFilter(Some):children:0:[child.label.eq,child.owner.relation,child.tenant.relation]:[parent.is]",
            "parent:RelationFilter(Is):parent:0:[parent.label.eq]:[]",
        ]
    );

    // A denied target layer is non-matching, never an error.
    let denied = || {
        Arc::new(Authority {
            deny_filter_target: Some("parent"),
            hide_secret: true,
            checks: Mutex::new(Vec::new()),
            deny_fields: &[],
            deny_select_relation: None,
        })
    };
    assert!(
        f.ids(
            &api,
            denied(),
            "children",
            json!({"parent": {"is": {"label": {"eq": "first"}}}})
        )
        .await
        .is_empty()
    );
    assert_eq!(
        f.ids(
            &api,
            denied(),
            "children",
            json!({"parent": {"isNot": {"label": {"eq": "first"}}}})
        )
        .await,
        ["11", "12", "21", "22", "41", "51"]
    );
    f.finish().await;
}

#[tokio::test]
async fn relation_filters_are_opt_in_bounded_and_keep_cursors_stable() {
    let f = Fixture::new().await;
    // Disabled: no SDL change and relation entries are rejected.
    let plain = f.api(false);
    assert!(!plain.sdl().contains("ToManyFilter"));
    let rejected = f
        .run(
            &plain,
            Authority::new(),
            Default::default(),
            "query($where: ParentWhereInput) { parents(where: $where) { edges { node { id } } } }",
            json!({"where": {"children": {"some": {"label": {"eq": "a"}}}}}),
        )
        .await;
    assert!(rejected.get("errors").is_some(), "{rejected}");

    // Predicate depth bounds count relation nesting.
    let api = f.api(true);
    let mut limits = RuntimeGraphqlLimits::default();
    limits.query.max_predicate_depth = 3;
    let deep = f
        .run(
            &api,
            Authority::new(),
            limits,
            "query($where: ParentWhereInput) { parents(where: $where) { edges { node { id } } } }",
            json!({"where": {"children": {"some": {"parent": {"is": {"children": {"some": {"label": {"eq": "a"}}}}}}}}}),
        )
        .await;
    assert!(deep.to_string().contains("cost_exceeded"), "{deep}");

    // Keyset pages over a relation-filtered connection resume after their cursor.
    let page = |after: Option<String>| {
        let api = &api;
        let f = &f;
        async move {
            f.run(
                api,
                Authority::new(),
                Default::default(),
                "query($after: String) { parents(first: 1, after: $after, where: {children: {some: {label: {in: [\"a\", \"c\"]}}}}) { edges { node { id } } pageInfo { hasNextPage endCursor } } }",
                json!({"after": after}),
            )
            .await
        }
    };
    let first = page(None).await;
    assert_eq!(
        first["data"]["parents"]["edges"][0]["node"]["id"], "1",
        "{first}"
    );
    assert_eq!(first["data"]["parents"]["pageInfo"]["hasNextPage"], true);
    let cursor = first["data"]["parents"]["pageInfo"]["endCursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = page(Some(cursor)).await;
    assert_eq!(
        second["data"]["parents"]["edges"][0]["node"]["id"], "2",
        "{second}"
    );
    assert_eq!(second["data"]["parents"]["pageInfo"]["hasNextPage"], false);

    // CAS conditions never accept relation predicates.
    let cas = f
        .run(
            &api,
            Authority::new(),
            Default::default(),
            "mutation($schema: String!) { updateChild(expectedSchema: $schema, key: {id: \"11\"}, input: {label: \"z\"}, expected: {parent: {is: {label: {eq: \"first\"}}}}) { record { id } } }",
            json!({}),
        )
        .await;
    assert!(cas.get("errors").is_some(), "{cas}");
    f.finish().await;
}

#[tokio::test]
async fn mutation_payloads_select_to_one_relations_through_nested_read_authority() {
    let f = Fixture::new().await;
    let api = f.api(false);
    let authority = Authority::new();
    let updated = f
        .run(
            &api,
            authority.clone(),
            Default::default(),
            "mutation($schema: String!) { updateChild(expectedSchema: $schema, key: {id: \"21\"}, input: {label: \"renamed\"}) { record { label parent { label children(first: 5) { edges { node { label } } } } } } }",
            json!({}),
        )
        .await;
    assert!(updated.get("errors").is_none(), "{updated}");
    let record = &updated["data"]["updateChild"]["record"];
    assert_eq!(record["label"], "renamed");
    assert_eq!(record["parent"]["label"], "second");
    assert_eq!(
        record["parent"]["children"]["edges"],
        json!([{"node": {"label": "renamed"}}, {"node": {"label": "d"}}])
    );
    // The payload layer reads its relation keys; the related layers are authorized like queries.
    let checks = authority.checks();
    assert!(
        checks.contains(&"child:Select:-:4:[]:[]".to_owned()),
        "{checks:?}"
    );
    assert!(
        checks.contains(&"parent:Select:parent:1:[]:[]".to_owned()),
        "{checks:?}"
    );
    assert!(
        checks.contains(&"child:Select:children:1:[]:[]".to_owned()),
        "{checks:?}"
    );

    // A related row hidden by the nested grant is null, never an error.
    let hidden = f
        .run(
            &api,
            Authority::new(),
            Default::default(),
            "mutation($schema: String!) { updateChild(expectedSchema: $schema, key: {id: \"51\"}, input: {label: \"y\"}) { record { id parent { label } } } }",
            json!({}),
        )
        .await;
    assert!(hidden.get("errors").is_none(), "{hidden}");
    assert_eq!(
        hidden["data"]["updateChild"]["record"],
        json!({"id": "51", "parent": null})
    );

    // Deletes release the preimage's relation, read after commit.
    let deleted = f
        .run(
            &api,
            Authority::new(),
            Default::default(),
            "mutation($schema: String!) { deleteChild(expectedSchema: $schema, key: {id: \"11\"}) { record { id parent { label } } } }",
            json!({}),
        )
        .await;
    assert_eq!(
        deleted["data"]["deleteChild"]["record"],
        json!({"id": "11", "parent": {"label": "first"}}),
        "{deleted}"
    );

    // To-many payload relations read a page of the committed children.
    let many = f
        .run(
            &api,
            Authority::new(),
            Default::default(),
            "mutation($schema: String!) { updateParent(expectedSchema: $schema, key: {id: \"1\"}, input: {label: \"q\"}) { record { children(first: 50) { edges { node { id } } } } } }",
            json!({}),
        )
        .await;
    assert!(many.get("errors").is_none(), "{many}");
    assert!(
        many["data"]["updateParent"]["record"]["children"]["edges"]
            .as_array()
            .is_some_and(|edges| !edges.is_empty()),
        "{many}"
    );
    f.finish().await;
}

fn authority(
    deny_fields: &'static [&'static str],
    deny_select_relation: Option<&'static str>,
) -> Arc<Authority> {
    Arc::new(Authority {
        deny_filter_target: None,
        hide_secret: true,
        checks: Mutex::new(Vec::new()),
        deny_fields,
        deny_select_relation,
    })
}

#[tokio::test]
async fn correlation_keys_are_filter_operations_of_the_source_layer() {
    let f = Fixture::new().await;
    let api = f.api(true);
    // Direct self-reference: parent 2's manager is parent 1.
    assert_eq!(
        f.ids(
            &api,
            Authority::new(),
            "parents",
            json!({"manager": {"is": {"label": {"eq": "first"}}}})
        )
        .await,
        ["2"]
    );
    // A caller who may not use the key field cannot filter through the relation.
    let denied = f
        .run(
            &api,
            authority(&["parent.manager"], None),
            Default::default(),
            "query($where: ParentWhereInput) { parents(where: $where) { edges { node { id } } } }",
            json!({"where": {"manager": {"is": {"label": {"eq": "first"}}}}}),
        )
        .await;
    assert!(
        denied.to_string().contains("authorization_denied"),
        "{denied}"
    );
    // Unrelated filters by that caller still work.
    assert_eq!(
        f.ids(
            &api,
            authority(&["parent.manager"], None),
            "parents",
            json!({"label": {"eq": "second"}})
        )
        .await,
        ["2"]
    );
    f.finish().await;
}

#[tokio::test]
async fn every_treats_unknown_target_values_as_not_matching() {
    let f = Fixture::new().await;
    let api = f.api(true);
    // Every visible child's note is NULL, so `eq` is unknown and never satisfies `every`.
    assert!(
        f.ids(
            &api,
            Authority::new(),
            "parents",
            json!({"children": {"every": {"note": {"eq": "x"}}}})
        )
        .await
        .is_empty()
    );
    f.sql("UPDATE rf_child SET note = 'x' WHERE owner = 2")
        .await;
    assert_eq!(
        f.ids(
            &api,
            Authority::new(),
            "parents",
            json!({"children": {"every": {"note": {"eq": "x"}}}})
        )
        .await,
        ["2"]
    );
    f.finish().await;
}

#[tokio::test]
async fn relation_filters_are_admitted_per_relation_and_budgeted() {
    let f = Fixture::new().await;
    let api = f.api_with(|o| o.with_relation_filters_for([RelationId::new("children").unwrap()]));
    let sdl = api.sdl();
    assert!(sdl.contains("input ChildToManyFilter"), "{sdl}");
    assert!(!sdl.contains("ToOneFilter"), "{sdl}");
    assert_eq!(
        f.ids(
            &api,
            Authority::new(),
            "parents",
            json!({"children": {"some": {"label": {"eq": "c"}}}})
        )
        .await,
        ["2"]
    );
    let rejected = f
        .run(
            &api,
            Authority::new(),
            Default::default(),
            "query($where: ChildWhereInput) { children(where: $where) { edges { node { id } } } }",
            json!({"where": {"parent": {"is": {"label": {"eq": "first"}}}}}),
        )
        .await;
    assert!(rejected.get("errors").is_some(), "{rejected}");
    let module = RuntimeGraphqlModule::compile(
        f.schema.clone(),
        RuntimeGraphqlOptions::default()
            .with_relation_filters_for([RelationId::new("children").unwrap()]),
    )
    .unwrap();
    assert_eq!(
        module.relation_filter_quantifiers(&RelationId::new("children").unwrap()),
        [
            RuntimeRelationQuantifier::Some,
            RuntimeRelationQuantifier::None,
            RuntimeRelationQuantifier::Every
        ]
    );
    assert!(
        module
            .relation_filter_quantifiers(&RelationId::new("parent").unwrap())
            .is_empty()
    );
    assert!(
        RuntimeGraphqlModule::compile(
            f.schema.clone(),
            RuntimeGraphqlOptions::default()
                .with_relation_filters_for([RelationId::new("missing").unwrap()]),
        )
        .is_err()
    );

    // Count and nesting budgets.
    let tight = f.api_with(|o| o.with_relation_filters().with_relation_filter_limits(2, 1));
    let query =
        "query($where: ParentWhereInput) { parents(where: $where) { edges { node { id } } } }";
    let some = |label: &str| json!({"children": {"some": {"label": {"eq": label}}}});
    let within = f
        .run(
            &tight,
            Authority::new(),
            Default::default(),
            query,
            json!({"where": {"or": [some("a"), some("c")]}}),
        )
        .await;
    assert!(within.get("errors").is_none(), "{within}");
    for filter in [
        json!({"or": [some("a"), some("c"), some("d")]}),
        json!({"children": {"some": {"parent": {"is": {"label": {"eq": "first"}}}}}}),
    ] {
        let exceeded = f
            .run(
                &tight,
                Authority::new(),
                Default::default(),
                query,
                json!({"where": filter}),
            )
            .await;
        assert!(exceeded.to_string().contains("cost_exceeded"), "{exceeded}");
    }
    // The default operation budget is RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTERS.
    let api = f.api(true);
    let many = (0..=RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTERS)
        .map(|i| some(&i.to_string()))
        .collect::<Vec<_>>();
    let exceeded = f
        .run(
            &api,
            Authority::new(),
            Default::default(),
            query,
            json!({"where": {"or": many}}),
        )
        .await;
    assert!(exceeded.to_string().contains("cost_exceeded"), "{exceeded}");
    f.finish().await;
}

#[tokio::test]
async fn payload_relation_layers_denied_outright_refuse_before_the_write() {
    let f = Fixture::new().await;
    let api = f.api(false);
    let denied = f
        .run(
            &api,
            authority(&[], Some("parent")),
            Default::default(),
            "mutation($schema: String!) { updateChild(expectedSchema: $schema, key: {id: \"21\"}, input: {label: \"never\"}) { record { id parent { label } } } }",
            json!({}),
        )
        .await;
    assert!(
        denied.to_string().contains("authorization_denied"),
        "{denied}"
    );
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM rf_child WHERE label = 'never'")
            .await,
        0
    );
    f.finish().await;
}

#[test]
fn engine_relation_predicates_validate_handles_quantifiers_and_offline_matching() {
    let schema = schema();
    let limits = RuntimeQueryLimits::default();
    let child = schema.resolve_collection(&cid("child")).unwrap();
    let parent = schema.resolve_collection(&cid("parent")).unwrap();
    let to_parent = schema
        .resolve_relation(&child, &RelationId::new("parent").unwrap())
        .unwrap();
    let label = schema.resolve_field(&child, &fid("child.label")).unwrap();
    let child_filter = schema
        .runtime_compare(
            &child,
            &label,
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("a".into()),
            limits,
        )
        .unwrap();
    use RuntimeRelationQuantifier as Q;
    assert_eq!(
        schema
            .runtime_relation_filter(&child, &to_parent, Q::Some, None, None, limits)
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::UnsupportedOperator
    );
    for quantifier in [Q::Some, Q::None, Q::Every] {
        assert!(quantifier.applies_to(RelationCardinality::Many));
        assert!(!quantifier.applies_to(RelationCardinality::One));
    }
    // The relation's source must be the predicate's collection; target predicates
    // must belong to the relation's target.
    assert!(
        schema
            .runtime_relation_filter(&parent, &to_parent, Q::Is, None, None, limits)
            .is_err()
    );
    assert!(
        schema
            .runtime_relation_filter(
                &child,
                &to_parent,
                Q::Is,
                Some(child_filter.clone()),
                None,
                limits
            )
            .is_err()
    );
    assert!(
        schema
            .runtime_relation_filter(&child, &to_parent, Q::Is, None, Some(child_filter), limits)
            .is_err()
    );
    let mut tight = limits;
    tight.max_predicate_depth = 1;
    let parent_label = schema.resolve_field(&parent, &fid("parent.label")).unwrap();
    let parent_filter = schema
        .runtime_compare(
            &parent,
            &parent_label,
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("first".into()),
            limits,
        )
        .unwrap();
    assert_eq!(
        schema
            .runtime_relation_filter(
                &child,
                &to_parent,
                Q::Is,
                Some(parent_filter.clone()),
                None,
                tight
            )
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::ResourceLimit
    );
    let predicate = schema
        .runtime_relation_filter(&child, &to_parent, Q::Is, Some(parent_filter), None, limits)
        .unwrap();
    assert!(predicate.has_relation_filters());
    // Only the source correlation keys are this collection's referenced fields.
    assert_eq!(
        predicate
            .referenced_fields()
            .iter()
            .map(|f| f.id().as_str())
            .collect::<Vec<_>>(),
        ["child.owner", "child.tenant"]
    );

    // Checked construction records the occurrence; offline matching cannot evaluate it.
    let builder = schema.runtime_filter_builder(&child, limits).unwrap();
    let node = builder.relation(&to_parent, Q::IsNot, None, None).unwrap();
    let checked = builder.finish(node).unwrap();
    struct Seen(Vec<String>);
    impl RuntimeFilterIntentVisitor for Seen {
        fn enter_boolean(&mut self, _: RuntimeFilterBoolean, _: bool) {}
        fn leave_boolean(&mut self, _: RuntimeFilterBoolean, _: bool) {}
        fn field(&mut self, _: &RuntimeFieldHandle, _: RuntimeFilterOperator, _: bool) {}
        fn relation(
            &mut self,
            relation: &RuntimeRelationHandle,
            quantifier: RuntimeRelationQuantifier,
        ) {
            self.0
                .push(format!("{}.{}", relation.id(), quantifier.as_str()));
        }
    }
    let mut seen = Seen(Vec::new());
    checked.visit_intent(&mut seen);
    assert_eq!(seen.0, ["parent.isNot"]);
    assert_eq!(
        checked
            .prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
            .unwrap_err()
            .code(),
        RuntimeFilterErrorCode::UnsupportedSemantics
    );
}
