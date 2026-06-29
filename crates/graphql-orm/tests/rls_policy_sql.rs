use graphql_orm::graphql::orm::{
    DatabaseBackend, RlsEntityMetadata, RlsOperation, RlsOperationPolicy, RlsSchemaModel,
    SchemaPolicy, build_rls_policy_plan, postgres_rls_helper_sql, render_policy_predicate,
    render_postgres_entity_rls_sql,
};

static RLS_POLICIES: &[RlsOperationPolicy] = &[
    RlsOperationPolicy::generated(
        RlsOperation::Select,
        Some("users.read"),
        Some("tenant_id"),
        Some("owner_id"),
    ),
    RlsOperationPolicy::generated(RlsOperation::Insert, None, None, None),
    RlsOperationPolicy::custom(
        RlsOperation::Delete,
        "graphql_orm.has_scope('users.delete') AND owner_id = graphql_orm.current_user_id()",
    ),
];

static RLS_ENTITY: RlsEntityMetadata = RlsEntityMetadata {
    entity_name: "User",
    table_name: "\"app\".\"users\"",
    force: true,
    policies: RLS_POLICIES,
};

#[test]
fn helper_function_sql_is_deterministic() {
    let first = postgres_rls_helper_sql();
    let second = postgres_rls_helper_sql();

    assert_eq!(first, second);
    assert_eq!(
        first.first().map(String::as_str),
        Some("CREATE SCHEMA IF NOT EXISTS graphql_orm")
    );
    assert!(
        first
            .iter()
            .any(|sql| sql.contains("graphql_orm.has_scope(scope text)"))
    );
}

#[test]
fn default_predicates_are_conservative_and_ordered() {
    let policy = RlsOperationPolicy::generated(
        RlsOperation::Select,
        Some("users.read"),
        Some("tenant_id"),
        Some("owner_id"),
    );

    assert_eq!(
        render_policy_predicate(DatabaseBackend::Postgres, &policy).as_deref(),
        Some(
            "graphql_orm.has_scope('users.read') AND \"tenant_id\" = graphql_orm.current_tenant_id() AND \"owner_id\" = graphql_orm.current_user_id()"
        )
    );
}

#[test]
fn custom_predicates_are_used_exactly() {
    let predicate =
        "graphql_orm.has_scope('users.delete') AND owner_id = graphql_orm.current_user_id()";
    let policy = RlsOperationPolicy::custom(RlsOperation::Delete, predicate);

    assert_eq!(
        render_policy_predicate(DatabaseBackend::Postgres, &policy).as_deref(),
        Some(predicate)
    );
}

#[test]
fn operation_without_conditions_emits_no_policy() {
    let policy = RlsOperationPolicy::generated(RlsOperation::Insert, None, None, None);

    assert!(render_policy_predicate(DatabaseBackend::Postgres, &policy).is_none());

    let sql = render_postgres_entity_rls_sql(&RLS_ENTITY);
    assert!(!sql.iter().any(|statement| statement.contains("FOR INSERT")));
}

#[test]
fn policy_sql_is_deterministic_and_quotes_schema_qualified_tables() {
    let first = render_postgres_entity_rls_sql(&RLS_ENTITY);
    let second = render_postgres_entity_rls_sql(&RLS_ENTITY);

    assert_eq!(first, second);
    assert_eq!(
        first,
        vec![
            "ALTER TABLE \"app\".\"users\" ENABLE ROW LEVEL SECURITY",
            "ALTER TABLE \"app\".\"users\" FORCE ROW LEVEL SECURITY",
            "DROP POLICY IF EXISTS \"graphql_orm_app_users_select\" ON \"app\".\"users\"",
            "CREATE POLICY \"graphql_orm_app_users_select\" ON \"app\".\"users\" FOR SELECT USING (graphql_orm.has_scope('users.read') AND \"tenant_id\" = graphql_orm.current_tenant_id() AND \"owner_id\" = graphql_orm.current_user_id())",
            "DROP POLICY IF EXISTS \"graphql_orm_app_users_delete\" ON \"app\".\"users\"",
            "CREATE POLICY \"graphql_orm_app_users_delete\" ON \"app\".\"users\" FOR DELETE USING (graphql_orm.has_scope('users.delete') AND owner_id = graphql_orm.current_user_id())",
        ]
    );
}

#[test]
fn schema_policy_controls_rls_planning() {
    let model = RlsSchemaModel {
        entities: vec![RLS_ENTITY.clone()],
    };

    let managed = build_rls_policy_plan(DatabaseBackend::Postgres, SchemaPolicy::Managed, &model);
    assert!(!managed.statements.is_empty());

    let plan_only =
        build_rls_policy_plan(DatabaseBackend::Postgres, SchemaPolicy::PlanOnly, &model);
    assert_eq!(managed.statements, plan_only.statements);

    let validate = build_rls_policy_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::ValidateOnly,
        &model,
    );
    assert!(validate.statements.is_empty());

    let external = build_rls_policy_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::ExternalReadOnly,
        &model,
    );
    assert!(external.statements.is_empty());
}
