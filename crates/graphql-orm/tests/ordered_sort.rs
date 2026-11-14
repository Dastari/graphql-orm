//! Ordering precedence for Rust callers (#694): a generated `OrderByInput`
//! value with several fields set sorts in field declaration order; the typed
//! ordered form and chained calls sort in the order the caller gives.

// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use OrderDirection::{Asc, Desc};
use graphql_orm::graphql::errors::OrmPublicError;
use graphql_orm::graphql::pagination::KeysetPageInput;
use graphql_orm::prelude::*;

/// `id` is declared before `expires_at`, as in the issue.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "expiring_things",
        plural = "ExpiringThings",
        keyset = "id asc"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "expiring_things",
        plural = "ExpiringThings",
        keyset = "id asc"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "expiring_things",
        plural = "ExpiringThings",
        schema_policy = "external_writable",
        keyset = "id asc"
    )
)]
struct ExpiringThing {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    #[filterable(type = "string")]
    id: String,
    #[sortable]
    #[filterable(type = "number")]
    expires_at: i64,
    #[sortable]
    #[filterable(type = "string")]
    bucket: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [ExpiringThing] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ExpiringThing] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ExpiringThing] }

#[cfg(feature = "sqlite")]
struct FixtureOwner;
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

struct AllowEntityPolicy;
impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// Rows whose primary-key order and expiry order disagree, with one tie.
const ROWS: [(&str, i64, &str); 5] = [
    ("a", 30, "x"),
    ("b", 10, "y"),
    ("c", 20, "x"),
    ("d", 10, "x"),
    ("e", 40, "y"),
];

async fn setup() -> (Database, FixtureOwner) {
    #[cfg(feature = "sqlite")]
    let (mut db, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (mut db, owner) = {
        let owner = FixtureOwner::start("ordered-sort").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (mut db, owner) = {
        let owner = FixtureOwner::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await
        .unwrap();
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };
    #[cfg(not(feature = "mssql"))]
    let ddl = "CREATE TABLE expiring_things (id TEXT PRIMARY KEY, expires_at BIGINT NOT NULL, bucket TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE expiring_things (id NVARCHAR(40) PRIMARY KEY, expires_at BIGINT NOT NULL, bucket NVARCHAR(40) NOT NULL)";
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), ddl, &[])
        .await
        .unwrap();
    for (id, expires_at, bucket) in ROWS {
        <DefaultBackend as WriteBackend>::execute_with_binds(
            db.pool(),
            &format!("INSERT INTO expiring_things VALUES ('{id}', {expires_at}, '{bucket}')"),
            &[],
        )
        .await
        .unwrap();
    }
    db.set_entity_policy(AllowEntityPolicy);
    (db, owner)
}

fn ids(rows: &[ExpiringThing]) -> String {
    rows.iter().map(|row| row.id.as_str()).collect()
}

fn by(field: ExpiringThingOrderField) -> ExpiringThingOrderByInput {
    DatabaseOrderField::order(field, Asc)
}

#[tokio::test]
async fn the_ordered_form_and_chained_calls_sort_in_caller_order() {
    use ExpiringThingOrderField as Field;
    let (db, mut owner) = setup().await;
    let query = EntityQuery::<ExpiringThing>::new;

    // One value with both fields set sorts by `id` first, because `id` is
    // declared first: the documented behaviour the ordered form exists for.
    let declared = query()
        .order_by(&ExpiringThingOrderByInput {
            expires_at: Some(Asc),
            id: Some(Asc),
            ..Default::default()
        })
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(ids(&declared), "abcde");

    // The ordered form: `expires_at` first, then `id`.
    let ordered = query()
        .order_by_fields([(Field::ExpiresAt, Asc), (Field::Id, Asc)])
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(ids(&ordered), "bdcae");

    // Chained single-field calls give the same rows.
    let chained = query()
        .order_by(&by(Field::ExpiresAt))
        .order_by(&by(Field::Id))
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(ids(&chained), "bdcae");

    // Directions are per term, and the tie on `expires_at` follows the second.
    let mixed = query()
        .order_by_fields([(Field::ExpiresAt, Asc), (Field::Id, Desc)])
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(ids(&mixed), "dbcae");

    // It composes with other ordering calls in call order.
    let composed = query()
        .order_by(&by(Field::Bucket))
        .order_by_fields([(Field::ExpiresAt, Desc), (Field::Id, Asc)])
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(ids(&composed), "acdeb");

    // The "earliest one" read the issue describes.
    let earliest = query()
        .order_by_fields([(Field::ExpiresAt, Asc), (Field::Id, Asc)])
        .fetch_one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(earliest.id, "b");

    // The transaction query builder takes the same form.
    let in_transaction = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.query::<ExpiringThing>()
                    .order_by_fields([(Field::ExpiresAt, Desc), (Field::Id, Asc)])
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .unwrap();
    assert_eq!(ids(&in_transaction), "eacbd");
    owner.cleanup().unwrap();
}

#[tokio::test]
async fn keyset_pages_follow_the_ordered_sort_and_its_cursor() {
    use ExpiringThingOrderField as Field;
    let (db, mut owner) = setup().await;
    let order = || {
        Some(
            ExpiringThingKeysetOrderByInput::ordered([(Field::ExpiresAt, Asc), (Field::Id, Asc)])
                .unwrap(),
        )
    };
    // The ordered form builds exactly the list a caller writes by hand.
    assert_eq!(order().unwrap().len(), 2);
    assert_eq!(order().unwrap()[0].expires_at, Some(Asc));
    assert_eq!(order().unwrap()[1].id, Some(Asc));

    let mut seen = String::new();
    let mut after = None;
    loop {
        let page = ExpiringThing::keyset_page_ordered(
            &db,
            Default::default(),
            order(),
            KeysetPageInput {
                after: after.clone(),
                limit: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for edge in &page.edges {
            seen.push_str(&edge.node.id);
        }
        if !page.page_info.has_next_page {
            break;
        }
        after = page.page_info.end_cursor.clone();
        assert!(after.is_some());
    }
    // Two keys, three pages: the cursor resumes after (expires_at, id).
    assert_eq!(seen, "bdcae");

    // The other precedence pages differently from the same rows.
    let by_id_first = ExpiringThing::keyset_page_ordered(
        &db,
        Default::default(),
        Some(
            ExpiringThingKeysetOrderByInput::ordered([(Field::Id, Asc), (Field::ExpiresAt, Asc)])
                .unwrap(),
        ),
        KeysetPageInput {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let first: String = by_id_first
        .edges
        .iter()
        .map(|edge| edge.node.id.as_str())
        .collect();
    assert_eq!(first, "ab");
    owner.cleanup().unwrap();
}
