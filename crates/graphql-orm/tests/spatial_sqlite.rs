#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "orm_spatial_places",
    plural = "SqlitePlaces",
    backend = "sqlite"
)]
struct SqlitePlace {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326, index = true))]
    #[filterable(type = "spatial")]
    pub location: graphql_orm::serde_json::Value,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "orm_spatial_regions",
    plural = "SqliteRegions",
    backend = "sqlite"
)]
struct SqliteRegion {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,

    #[graphql_orm(spatial(
        kind = "geometry",
        geometry_type = "Polygon",
        srid = 4326,
        index = true
    ))]
    #[filterable(type = "spatial")]
    pub boundary: graphql_orm::serde_json::Value,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "orm_spatial_trails",
    plural = "SqliteTrails",
    backend = "sqlite"
)]
struct SqliteTrail {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,

    #[graphql_orm(spatial(
        kind = "geometry",
        geometry_type = "LineString",
        srid = 4326,
        index = true
    ))]
    #[filterable(type = "spatial")]
    pub path: graphql_orm::serde_json::Value,
}

schema_roots! {
    query_custom_ops: [],
    entities: [SqlitePlace, SqliteRegion, SqliteTrail],
}

fn json_geometry(
    value: graphql_orm::serde_json::Value,
) -> graphql_orm::async_graphql::Json<graphql_orm::serde_json::Value> {
    graphql_orm::async_graphql::Json(value)
}

fn point_value(x: f64, y: f64) -> graphql_orm::serde_json::Value {
    graphql_orm::serde_json::json!({ "type": "Point", "coordinates": [x, y] })
}

fn point(x: f64, y: f64) -> graphql_orm::async_graphql::Json<graphql_orm::serde_json::Value> {
    json_geometry(point_value(x, y))
}

fn polygon_value(coordinates: Vec<[f64; 2]>) -> graphql_orm::serde_json::Value {
    graphql_orm::serde_json::json!({
        "type": "Polygon",
        "coordinates": [coordinates.into_iter().map(|point| vec![point[0], point[1]]).collect::<Vec<_>>()],
    })
}

fn line_string_value(coordinates: Vec<[f64; 2]>) -> graphql_orm::serde_json::Value {
    graphql_orm::serde_json::json!({
        "type": "LineString",
        "coordinates": coordinates.into_iter().map(|point| vec![point[0], point[1]]).collect::<Vec<_>>(),
    })
}

fn leak_migration(
    plan: &graphql_orm::graphql::orm::MigrationPlan,
    version: &'static str,
) -> graphql_orm::graphql::orm::Migration {
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    graphql_orm::graphql::orm::Migration {
        version,
        description: "sqlite_spatial_contract",
        statements,
    }
}

#[derive(Clone, Default)]
struct SpatialMutationHook(Arc<AtomicUsize>);

impl MutationHook<SqliteBackend> for SpatialMutationHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _hook_ctx: &'a mut MutationContext<'_, SqliteBackend>,
        _event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

async fn count_places(
    pool: &graphql_orm::sqlx::SqlitePool,
    filter: SpatialFilter,
) -> Result<usize, graphql_orm::sqlx::Error> {
    Ok(SqlitePlace::query(pool)
        .filter(SqlitePlaceWhereInput {
            location: Some(filter),
            ..Default::default()
        })
        .fetch_all()
        .await?
        .len())
}

async fn count_regions(
    pool: &graphql_orm::sqlx::SqlitePool,
    filter: SpatialFilter,
) -> Result<usize, graphql_orm::sqlx::Error> {
    Ok(SqliteRegion::query(pool)
        .filter(SqliteRegionWhereInput {
            boundary: Some(filter),
            ..Default::default()
        })
        .fetch_all()
        .await?
        .len())
}

async fn count_trails(
    pool: &graphql_orm::sqlx::SqlitePool,
    filter: SpatialFilter,
) -> Result<usize, graphql_orm::sqlx::Error> {
    Ok(SqliteTrail::query(pool)
        .filter(SqliteTrailWhereInput {
            path: Some(filter),
            ..Default::default()
        })
        .fetch_all()
        .await?
        .len())
}

#[tokio::test]
async fn sqlite_spatial_fields_and_predicates_round_trip() -> Result<(), Box<dyn std::error::Error>>
{
    use async_graphql::Request;
    use graphql_orm::graphql::orm::{
        DatabaseBackend, Entity, MigrationRunner, build_migration_plan, introspect_schema,
    };

    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <SqlitePlace as Entity>::metadata(),
        <SqliteRegion as Entity>::metadata(),
        <SqliteTrail as Entity>::metadata(),
    ]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &graphql_orm::graphql::orm::SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target_schema,
    );

    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains("location TEXT NOT NULL"))
    );
    assert!(
        !plan
            .statements
            .iter()
            .any(|statement| statement.contains("postgis"))
    );
    assert!(
        !plan
            .statements
            .iter()
            .any(|statement| statement.contains("USING GIST")
                || statement.contains("(location)")
                || statement.contains("(boundary)")
                || statement.contains("(path)"))
    );

    database
        .apply_migrations(&[leak_migration(&plan, "2026070101_sqlite_spatial")])
        .await?;

    let alpha = SqlitePlace::insert(
        &pool,
        CreateSqlitePlaceInput {
            name: "alpha".to_string(),
            location: point_value(0.0, 0.0),
        },
    )
    .await?;
    let beta = SqlitePlace::insert(
        &pool,
        CreateSqlitePlaceInput {
            name: "beta".to_string(),
            location: point_value(3.0, 3.0),
        },
    )
    .await?;
    SqlitePlace::insert(
        &pool,
        CreateSqlitePlaceInput {
            name: "gamma".to_string(),
            location: point_value(0.5, 0.5),
        },
    )
    .await?;

    let square = polygon_value(vec![
        [-1.0, -1.0],
        [1.0, -1.0],
        [1.0, 1.0],
        [-1.0, 1.0],
        [-1.0, -1.0],
    ]);
    let adjacent_square = polygon_value(vec![
        [1.0, -1.0],
        [2.0, -1.0],
        [2.0, 1.0],
        [1.0, 1.0],
        [1.0, -1.0],
    ]);
    let overlapping_square = polygon_value(vec![
        [0.0, -1.0],
        [2.0, -1.0],
        [2.0, 1.0],
        [0.0, 1.0],
        [0.0, -1.0],
    ]);
    SqliteRegion::insert(
        &pool,
        CreateSqliteRegionInput {
            boundary: square.clone(),
        },
    )
    .await?;
    SqliteTrail::insert(
        &pool,
        CreateSqliteTrailInput {
            path: line_string_value(vec![[-1.0, 0.0], [1.0, 0.0]]),
        },
    )
    .await?;

    assert_eq!(
        count_places(
            &pool,
            SpatialFilter {
                equals: Some(point(0.0, 0.0)),
                ..Default::default()
            },
        )
        .await?,
        1
    );
    assert_eq!(
        count_places(
            &pool,
            SpatialFilter {
                disjoint: Some(point(3.0, 3.0)),
                ..Default::default()
            },
        )
        .await?,
        2
    );
    assert_eq!(
        count_places(
            &pool,
            SpatialFilter {
                intersects: Some(point(0.0, 0.0)),
                ..Default::default()
            },
        )
        .await?,
        1
    );
    assert_eq!(
        count_places(
            &pool,
            SpatialFilter {
                within: Some(json_geometry(square.clone())),
                ..Default::default()
            },
        )
        .await?,
        2
    );
    assert_eq!(
        count_regions(
            &pool,
            SpatialFilter {
                contains: Some(point(0.0, 0.0)),
                ..Default::default()
            },
        )
        .await?,
        1
    );
    assert_eq!(
        count_regions(
            &pool,
            SpatialFilter {
                touches: Some(json_geometry(adjacent_square)),
                ..Default::default()
            },
        )
        .await?,
        1
    );
    assert_eq!(
        count_regions(
            &pool,
            SpatialFilter {
                overlaps: Some(json_geometry(overlapping_square)),
                ..Default::default()
            },
        )
        .await?,
        1
    );
    assert_eq!(
        count_trails(
            &pool,
            SpatialFilter {
                crosses: Some(json_geometry(line_string_value(vec![
                    [0.0, -1.0],
                    [0.0, 1.0]
                ]))),
                ..Default::default()
            },
        )
        .await?,
        1
    );

    let fallback_or_filter = SqlitePlaceWhereInput {
        or: Some(vec![
            SqlitePlaceWhereInput {
                name: Some(StringFilter {
                    eq: Some("missing".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            SqlitePlaceWhereInput {
                location: Some(SpatialFilter {
                    equals: Some(point(0.0, 0.0)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    assert_eq!(
        SqlitePlace::query(&pool)
            .filter(fallback_or_filter)
            .fetch_all()
            .await?
            .len(),
        1
    );

    let within_square_filter = SqlitePlaceWhereInput {
        location: Some(SpatialFilter {
            within: Some(json_geometry(square.clone())),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        SqlitePlace::query(&pool)
            .filter(within_square_filter.clone())
            .count()
            .await?,
        2
    );
    let paged = SqlitePlace::query(&pool)
        .filter(within_square_filter.clone())
        .order_by(SqlitePlaceOrderByInput {
            name: Some(OrderDirection::Asc),
            ..Default::default()
        })
        .offset(1)
        .limit(1)
        .fetch_all()
        .await?;
    assert_eq!(paged.len(), 1);
    assert_eq!(paged[0].name, "gamma");

    let updated = SqlitePlace::update_by_id(
        &database,
        &alpha.id,
        UpdateSqlitePlaceInput {
            location: Some(point_value(2.0, 2.0)),
            ..Default::default()
        },
    )
    .await?
    .expect("updated place should be returned");
    assert_eq!(updated.location["coordinates"][0].as_f64(), Some(2.0));

    let deleted = SqlitePlace::delete_where(
        &database,
        SqlitePlaceWhereInput {
            location: Some(SpatialFilter {
                equals: Some(point(3.0, 3.0)),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(deleted, 1);
    assert!(SqlitePlace::get(&pool, &beta.id).await?.is_none());

    let schema = schema_builder(database.clone())
        .data("sqlite-spatial-test-user".to_string())
        .finish();
    let graph_response = schema
        .execute(Request::new(
            r#"
            query {
              sqlitePlaces(where: { location: { equals: { type: "Point", coordinates: [2.0, 2.0] } } }) {
                pageInfo { totalCount }
                edges { node { name location } }
              }
            }
            "#,
        ))
        .await;
    assert!(
        graph_response.errors.is_empty(),
        "GraphQL spatial query errors: {:?}",
        graph_response.errors
    );
    let graph_json = graph_response.data.into_json()?;
    assert_eq!(graph_json["sqlitePlaces"]["pageInfo"]["totalCount"], 1);
    assert_eq!(
        graph_json["sqlitePlaces"]["edges"][0]["node"]["location"]["type"],
        "Point"
    );

    let introspected = introspect_schema(&pool).await?;
    let place_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "orm_spatial_places")
        .expect("orm_spatial_places table should be introspected");
    let location_column = place_table
        .columns
        .iter()
        .find(|column| column.name == "location")
        .expect("location column should be introspected");
    assert_eq!(location_column.sql_type, "TEXT");
    assert!(location_column.spatial.is_none());
    let second_plan = build_migration_plan(DatabaseBackend::Sqlite, &introspected, &target_schema);
    assert!(second_plan.statements.is_empty());

    Ok(())
}

#[tokio::test]
async fn sqlite_bounded_mutations_reject_residual_filters_before_side_effects()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{
        DatabaseBackend, Entity, MigrationRunner, build_migration_plan,
    };

    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let mut database = graphql_orm::db::Database::new(pool.clone());
    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <SqlitePlace as Entity>::metadata(),
    ]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &graphql_orm::graphql::orm::SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target_schema,
    );
    database
        .apply_migrations(&[leak_migration(&plan, "2026072201_bounded_residual_filter")])
        .await?;

    let protected_name = "row-secret:database-password";
    let place = SqlitePlace::insert(
        &pool,
        CreateSqlitePlaceInput {
            name: protected_name.to_string(),
            location: point_value(7.25, 9.5),
        },
    )
    .await?;
    let hook = SpatialMutationHook::default();
    database.set_mutation_hook(hook.clone());
    let mut events = database
        .ensure_event_sender::<SqlitePlaceChangedEvent>()
        .subscribe();
    let residual_filter = SqlitePlaceWhereInput {
        name: Some(StringFilter {
            eq: Some(protected_name.to_string()),
            ..Default::default()
        }),
        location: Some(SpatialFilter {
            equals: Some(point(7.25, 9.5)),
            ..Default::default()
        }),
        ..Default::default()
    };

    for error in [
        SqlitePlace::update_where_bounded(
            &database,
            residual_filter.clone(),
            UpdateSqlitePlaceInput {
                name: Some("must-not-be-written".to_string()),
                ..Default::default()
            },
            MutationLimit::new(10)?,
        )
        .await
        .expect_err("residual bounded update must fail closed"),
        SqlitePlace::delete_where_bounded(&database, residual_filter, MutationLimit::new(10)?)
            .await
            .expect_err("residual bounded delete must fail closed"),
    ] {
        let public = OrmPublicError::from_sqlx(&error);
        assert_eq!(public.code, OrmErrorCode::InvalidInput);
        let rendered = format!("{error:?}{public:?}{public}");
        for forbidden in [
            protected_name,
            "must-not-be-written",
            "orm_spatial_places",
            "7.25",
            "9.5",
            "SELECT",
            "WHERE",
        ] {
            assert!(!rendered.contains(forbidden), "leaked {forbidden:?}");
        }
    }
    assert_eq!(hook.0.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let unchanged = SqlitePlace::get(&pool, &place.id)
        .await?
        .expect("residual rejection must leave the row intact");
    assert_eq!(unchanged.name, protected_name);
    assert_eq!(unchanged.location, point_value(7.25, 9.5));
    Ok(())
}
