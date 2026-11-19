#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
//! The relation policy SQL that one statement's generated relation filters
//! add is budgeted together, not per filter: a where input that repeats a
//! relation filter cannot multiply a large policy into an oversized statement.
#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use graphql_orm::prelude::*;
use serde_json::{Value, json};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "budget_parents", plural = "BudgetParents", keyset = "id asc")]
struct BudgetParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[relation(target = "BudgetLink", from = "id", to = "parent_id", multiple, filter)]
    links: Vec<BudgetLink>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "budget_links", plural = "BudgetLinks")]
struct BudgetLink {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    parent_id: String,
    #[filterable(type = "string")]
    label: Option<String>,
}
schema_roots! { query_custom_ops: [], entities: [BudgetParent, BudgetLink], }

/// About 290 KB of policy SQL: every link with a label, spelled as a wide
/// `or` of terms that never match plus one that does.
fn large_link_policy() -> async_graphql::Result<ReadPredicate> {
    let mut policy = ReadPredicate::is_not_null::<SqliteBackend, BudgetLink>("label")?;
    for _ in 0..16_000 {
        policy = policy.or(ReadPredicate::is_null::<SqliteBackend, BudgetLink>("id")?)?;
    }
    Ok(policy)
}

struct LargePolicy;
impl RowPolicy for LargePolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match entity {
                "BudgetLink" => ReadVisibility::Complete(large_link_policy()?),
                _ => ReadVisibility::Unrestricted,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn schema() -> AppSchema {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for statement in [
        "CREATE TABLE budget_parents (id TEXT PRIMARY KEY)",
        "CREATE TABLE budget_links (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL, label TEXT NULL)",
        "INSERT INTO budget_parents VALUES ('a'), ('b'), ('c')",
        "INSERT INTO budget_links VALUES ('1', 'a', 'x'), ('2', 'b', NULL), ('3', 'c', 'y')",
    ] {
        sqlx::query(statement).execute(db.pool()).await.unwrap();
    }
    db.set_row_policy(LargePolicy);
    schema_builder(db).data(AuthSubject::new("reader")).finish()
}

/// Parents with a visible link labelled `x` or `y`, as `filters` relation
/// filters joined by `or`.
async fn parents(schema: &AppSchema, filters: usize) -> async_graphql::Response {
    let alternatives = (0..filters)
        .map(|index| {
            json!({ f("links"): [{ f("label"): { f("eq"): if index % 2 == 0 { "x" } else { "y" } } }] })
        })
        .collect::<Vec<Value>>();
    schema
        .execute(
            async_graphql::Request::new(format!(
                "query($w: BudgetParentWhereInput!) {{ rows: {}({}: $w) {{ edges {{ node {{ {} }} }} }} }}",
                r("budgetParents"),
                a("where"),
                f("id")
            ))
            .variables(async_graphql::Variables::from_json(
                json!({ "w": { f("or"): alternatives } }),
            )),
        )
        .await
}

#[tokio::test]
async fn relation_filters_share_one_policy_budget_per_statement() {
    assert_eq!(READ_PREDICATE_MAX_STATEMENT_RELATION_BYTES, 1 << 20);
    let schema = schema().await;
    // Three relation filters add about 870 KB of policy SQL: within budget.
    let response = parents(&schema, 3).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let mut ids = response.data.into_json().unwrap()["rows"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"][f("id")].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["a", "c"]);
    // A fourth crosses it: refused before the statement is rendered, though
    // each filter alone is within the per-predicate budget.
    let response = parents(&schema, 4).await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .as_deref(),
        Some("\"AUTHORIZATION_MISCONFIGURED\""),
        "{:?}",
        response.errors
    );
}
