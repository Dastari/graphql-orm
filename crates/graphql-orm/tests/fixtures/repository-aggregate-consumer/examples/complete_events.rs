//! SQLite executable; PostgreSQL execution is covered by the owned portable_groups test.
//! Private models and complete enumeration, without application query SQL or GraphQL roots.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use graphql_orm::prelude::*;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "private_event_history",
        plural = "PrivateEventHistory"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "private_event_history",
        plural = "PrivateEventHistory"
    )
)]
struct PrivateEvent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    tenant: String,
    event: String,
    #[graphql_orm(private, sensitive)]
    custody: Option<String>,
}
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn run(db: &Database<repository_aggregate_consumer::Backend>) -> graphql_orm::Result<()> {
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "example",
            "owned private history",
            &[PrivateEvent::metadata()],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, Default::default())
        .await?;
    for index in 0..1225 {
        PrivateEvent::insert(
            db,
            CreatePrivateEventInput {
                id: index.to_string(),
                tenant: "tenant-a".into(),
                event: format!("Event-{index:04}"),
                custody: None,
            },
        )
        .await?;
    }
    PrivateEvent::insert(
        db,
        CreatePrivateEventInput {
            id: "other".into(),
            tenant: "tenant-b".into(),
            event: "Private-other-tenant".into(),
            custody: None,
        },
    )
    .await?;
    let options = AggregateGroupPageOptions {
        order: AggregateGroupOrder::SqliteNoCase,
        exclude_blank: true,
        context: "verified-tenant-a:public-revision-1".into(),
    };
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = PrivateEvent::aggregate(db)
            .filter(PrivateEventWhereInput {
                tenant: Some(StringFilter {
                    eq: Some("tenant-a".into()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .group_by(PrivateEventAggregateField::Event)?
            .group_limit(100)?
            .fetch_group_page(options.clone(), cursor.as_ref())
            .await?;
        for row in page.rows {
            if let AggregateValue::Text(event) = row.groups[0].value.clone() {
                events.push(event);
            }
        }
        // Wrap/protect this bounded JSON in the application's existing cursor envelope.
        cursor = page
            .end_cursor
            .map(|cursor| AggregateGroupCursor::from_json(&cursor.to_json()?))
            .transpose()?;
        if !page.has_next_page {
            break;
        }
    }
    assert_eq!(events.len(), 1225);
    assert_eq!(events[0], "Event-0000");
    assert_eq!(events.last().unwrap(), "Event-1224");
    let rows = PrivateEvent::aggregate(db)
        .filter(PrivateEventWhereInput {
            tenant: Some(StringFilter {
                eq: Some("tenant-a".into()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .count_rows()?
        .fetch()
        .await?;
    assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(1225));
    println!(
        "Enumerated {} original event types in bounded authorized pages",
        events.len()
    );
    Ok(())
}
#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> graphql_orm::Result<()> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    run(&db).await
}
#[cfg(not(feature = "sqlite"))]
fn main() {
    println!("Use the owned portable_groups PostgreSQL test; MSSQL pages are unsupported");
}
