use super::core::{
    RlsEntityMetadata, RlsOperation, RlsOperationPolicy, RlsPolicyPlan, RlsSchemaModel,
    SchemaDiagnostic, SchemaDiagnosticKind, SchemaDiagnosticSeverity, SchemaPolicy,
};
use super::dialect::{DatabaseBackend, SqlDialect};

/// Render deterministic PostgreSQL helper function SQL for RLS predicates.
///
/// The helpers read transaction-local `app.*` settings set from
/// [`DbAuthContext`](crate::graphql::orm::DbAuthContext).
pub fn postgres_rls_helper_sql() -> Vec<String> {
    vec![
        "CREATE SCHEMA IF NOT EXISTS graphql_orm".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_user_id() RETURNS text LANGUAGE sql STABLE AS $$ SELECT NULLIF(current_setting('app.user_id', true), '') $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_subject() RETURNS text LANGUAGE sql STABLE AS $$ SELECT NULLIF(current_setting('app.subject', true), '') $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_tenant_id() RETURNS text LANGUAGE sql STABLE AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '') $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_roles() RETURNS text[] LANGUAGE sql STABLE AS $$ SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(NULLIF(current_setting('app.roles', true), '')::jsonb, '[]'::jsonb))) $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_scopes() RETURNS text[] LANGUAGE sql STABLE AS $$ SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(NULLIF(current_setting('app.scopes', true), '')::jsonb, '[]'::jsonb))) $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.claims() RETURNS jsonb LANGUAGE sql STABLE AS $$ SELECT COALESCE(NULLIF(current_setting('app.claims', true), '')::jsonb, '{}'::jsonb) $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.has_scope(scope text) RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT scope = ANY(graphql_orm.current_scopes()) $$".to_string(),
    ]
}

/// Build the RLS portion of a full schema target plan.
///
/// Statements are produced only for PostgreSQL under `Managed` or `PlanOnly`.
/// External policies and non-Postgres backends intentionally produce an empty
/// plan.
pub fn build_rls_policy_plan(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    target: &RlsSchemaModel,
) -> RlsPolicyPlan {
    let include_rls = backend == DatabaseBackend::Postgres
        && matches!(policy, SchemaPolicy::Managed | SchemaPolicy::PlanOnly)
        && !target.entities.is_empty();
    let mut statements = Vec::new();

    if include_rls {
        statements.extend(postgres_rls_helper_sql());
        for entity in sorted_entities(target) {
            statements.extend(render_postgres_entity_rls_sql(entity));
        }
    }

    RlsPolicyPlan {
        backend: backend.name(),
        target_rls_hash: target.stable_hash(),
        statements,
    }
}

/// Render deterministic RLS statements for one PostgreSQL entity.
///
/// Output includes `ENABLE ROW LEVEL SECURITY`, optional `FORCE ROW LEVEL
/// SECURITY`, and create-or-replace policy statements for operations with a
/// configured predicate.
pub fn render_postgres_entity_rls_sql(entity: &RlsEntityMetadata) -> Vec<String> {
    let mut statements = vec![format!(
        "ALTER TABLE {} ENABLE ROW LEVEL SECURITY",
        entity.table_name
    )];
    if entity.force {
        statements.push(format!(
            "ALTER TABLE {} FORCE ROW LEVEL SECURITY",
            entity.table_name
        ));
    }

    for policy in sorted_policies(entity) {
        let Some(predicate) = render_policy_predicate(DatabaseBackend::Postgres, policy) else {
            continue;
        };
        let policy_name = DatabaseBackend::Postgres
            .quote_identifier(&policy_name(entity.table_name, policy.operation));
        statements.push(format!(
            "DROP POLICY IF EXISTS {} ON {}",
            policy_name, entity.table_name
        ));
        statements.push(render_create_policy_statement(
            &policy_name,
            entity.table_name,
            policy.operation,
            &predicate,
        ));
    }

    statements
}

/// Render the SQL predicate for one operation policy.
///
/// Custom predicates are returned exactly. Generated predicates combine scope,
/// tenant, and owner conditions in deterministic order. Returns `None` when the
/// operation intentionally has no policy.
pub fn render_policy_predicate(
    backend: DatabaseBackend,
    policy: &RlsOperationPolicy,
) -> Option<String> {
    if let Some(predicate) = policy.predicate {
        return Some(predicate.to_string());
    }

    let mut conditions = Vec::new();
    if let Some(scope) = policy.scope {
        conditions.push(format!(
            "graphql_orm.has_scope('{}')",
            scope.replace('\'', "''")
        ));
    }
    if let Some(column) = policy.tenant_column {
        conditions.push(format!(
            "{} = graphql_orm.current_tenant_id()",
            backend.quote_identifier_path(column)
        ));
    }
    if let Some(column) = policy.owner_column {
        conditions.push(format!(
            "{} = graphql_orm.current_user_id()",
            backend.quote_identifier_path(column)
        ));
    }

    if conditions.is_empty() {
        None
    } else {
        Some(conditions.join(" AND "))
    }
}

fn render_create_policy_statement(
    policy_name: &str,
    table_name: &str,
    operation: RlsOperation,
    predicate: &str,
) -> String {
    match operation {
        RlsOperation::Select => format!(
            "CREATE POLICY {} ON {} FOR SELECT USING ({})",
            policy_name, table_name, predicate
        ),
        RlsOperation::Insert => format!(
            "CREATE POLICY {} ON {} FOR INSERT WITH CHECK ({})",
            policy_name, table_name, predicate
        ),
        RlsOperation::Update => format!(
            "CREATE POLICY {} ON {} FOR UPDATE USING ({}) WITH CHECK ({})",
            policy_name, table_name, predicate, predicate
        ),
        RlsOperation::Delete => format!(
            "CREATE POLICY {} ON {} FOR DELETE USING ({})",
            policy_name, table_name, predicate
        ),
    }
}

fn sorted_entities(target: &RlsSchemaModel) -> Vec<&RlsEntityMetadata> {
    let mut entities = target.entities.iter().collect::<Vec<_>>();
    entities.sort_by(|left, right| left.table_name.cmp(right.table_name));
    entities
}

fn sorted_policies(entity: &RlsEntityMetadata) -> Vec<&RlsOperationPolicy> {
    let mut policies = entity.policies.iter().collect::<Vec<_>>();
    policies.sort_by(|left, right| left.operation.cmp(&right.operation));
    policies
}

/// Return the deterministic generated policy name for a table operation.
///
/// Names use the `graphql_orm_<table>_<operation>` shape and are sanitized and
/// shortened deterministically to fit PostgreSQL's identifier length limit.
pub fn policy_name(table_name: &str, operation: RlsOperation) -> String {
    let mut sanitized = String::new();
    for ch in unquote_identifier_path(table_name).chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if !sanitized.ends_with('_') {
            sanitized.push('_');
        }
    }
    let sanitized = sanitized.trim_matches('_');
    let full = format!("graphql_orm_{}_{}", sanitized, operation.as_str());
    if full.len() <= 63 {
        return full;
    }
    let hash = stable_name_hash(&full);
    format!("{}_{hash}", &full[..54])
}

fn unquote_identifier_path(value: &str) -> String {
    value
        .split('.')
        .map(|part| {
            part.trim()
                .trim_matches('"')
                .trim_matches('[')
                .trim_matches(']')
                .replace("\"\"", "\"")
                .replace("]]", "]")
        })
        .collect::<Vec<_>>()
        .join("_")
}

fn stable_name_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:08x}")
}

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRlsPolicy {
    pub policy_name: String,
    pub operation: RlsOperation,
    pub using_expression: Option<String>,
    pub check_expression: Option<String>,
}

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRlsTable {
    pub table_name: String,
    pub enabled: bool,
    pub forced: bool,
    pub policies: Vec<LiveRlsPolicy>,
}

pub(crate) fn validate_rls_models(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    current: &[LiveRlsTable],
    target: &RlsSchemaModel,
) -> Vec<SchemaDiagnostic> {
    if target.entities.is_empty()
        || backend != DatabaseBackend::Postgres
        || matches!(
            policy,
            SchemaPolicy::ExternalReadOnly | SchemaPolicy::ExternalWritable
        )
    {
        return Vec::new();
    }

    let current = current
        .iter()
        .map(|table| (normalize_table_key(&table.table_name), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut diagnostics = Vec::new();

    for entity in sorted_entities(target) {
        let table_key = normalize_table_key(entity.table_name);
        let Some(current_table) = current.get(&table_key) else {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                entity.table_name,
                format!("Missing RLS table state for {}", entity.table_name),
            ));
            continue;
        };
        if !current_table.enabled {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                entity.table_name,
                format!("RLS is not enabled on {}", entity.table_name),
            ));
        }
        if entity.force && !current_table.forced {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                entity.table_name,
                format!("RLS is not forced on {}", entity.table_name),
            ));
        }

        let policies = current_table
            .policies
            .iter()
            .map(|policy| (policy.policy_name.as_str(), policy))
            .collect::<std::collections::BTreeMap<_, _>>();
        for expected in sorted_policies(entity) {
            let Some(predicate) = render_policy_predicate(DatabaseBackend::Postgres, expected)
            else {
                continue;
            };
            let expected_name = policy_name(entity.table_name, expected.operation);
            let Some(current_policy) = policies.get(expected_name.as_str()) else {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    entity.table_name,
                    format!(
                        "Missing RLS policy {} on {}",
                        expected_name, entity.table_name
                    ),
                ));
                continue;
            };
            if current_policy.operation != expected.operation {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    entity.table_name,
                    format!("RLS policy {} has unexpected operation", expected_name),
                ));
            }
            let expected = normalize_policy_expr(&predicate);
            let using_expr = current_policy
                .using_expression
                .as_deref()
                .map(normalize_policy_expr);
            let check_expr = current_policy
                .check_expression
                .as_deref()
                .map(normalize_policy_expr);
            match current_policy.operation {
                RlsOperation::Select | RlsOperation::Delete => {
                    if using_expr.as_deref() != Some(expected.as_str()) {
                        diagnostics.push(rls_diagnostic(
                            SchemaDiagnosticSeverity::Error,
                            entity.table_name,
                            format!("RLS policy {} USING expression differs", expected_name),
                        ));
                    }
                }
                RlsOperation::Insert => {
                    if check_expr.as_deref() != Some(expected.as_str()) {
                        diagnostics.push(rls_diagnostic(
                            SchemaDiagnosticSeverity::Error,
                            entity.table_name,
                            format!("RLS policy {} WITH CHECK expression differs", expected_name),
                        ));
                    }
                }
                RlsOperation::Update => {
                    if using_expr.as_deref() != Some(expected.as_str())
                        || check_expr.as_deref() != Some(expected.as_str())
                    {
                        diagnostics.push(rls_diagnostic(
                            SchemaDiagnosticSeverity::Error,
                            entity.table_name,
                            format!("RLS policy {} expression differs", expected_name),
                        ));
                    }
                }
            }
        }
    }

    diagnostics
}

fn normalize_table_key(table_name: &str) -> String {
    let unquoted = unquote_identifier_path(table_name);
    unquoted
        .strip_prefix("public_")
        .unwrap_or(&unquoted)
        .to_string()
}

fn normalize_policy_expr(value: &str) -> String {
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let trimmed = value.trim();
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            value = trimmed[1..trimmed.len() - 1].trim().to_string();
        } else {
            break;
        }
    }
    value
}

fn rls_diagnostic(
    severity: SchemaDiagnosticSeverity,
    table: &str,
    message: String,
) -> SchemaDiagnostic {
    SchemaDiagnostic {
        severity,
        kind: SchemaDiagnosticKind::RlsMismatch,
        table: Some(table.to_string()),
        column: None,
        message,
    }
}
