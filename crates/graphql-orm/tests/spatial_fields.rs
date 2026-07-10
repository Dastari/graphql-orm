#![cfg(feature = "postgres")]

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "spatial_places", plural = "Places", backend = "postgres")]
struct Place {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,

    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326, index = true))]
    #[filterable(type = "spatial")]
    pub location: graphql_orm::serde_json::Value,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "spatial_regions", plural = "Regions", backend = "postgres")]
struct Region {
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
#[graphql_entity(table = "spatial_trails", plural = "Trails", backend = "postgres")]
struct Trail {
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
    entities: [Place, Region, Trail],
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

#[tokio::test]
async fn generated_sdl_includes_spatial_filter() {
    let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test")
        .expect("lazy postgres pool");
    let database = graphql_orm::db::Database::<graphql_orm::PostgresBackend>::new(pool);
    let schema = schema_builder(database).finish();
    let sdl = schema.sdl();

    assert!(sdl.contains("input SpatialFilter"));
    assert!(sdl.contains("location: SpatialFilter"));
    assert!(sdl.contains("contains: JSON"));
    assert!(sdl.contains("overlaps: JSON"));
}

#[test]
fn spatial_filter_renders_all_predicates_and_values() {
    let filter = PlaceWhereInput {
        location: Some(SpatialFilter {
            equals: Some(point(1.0, 2.0)),
            disjoint: Some(point(3.0, 4.0)),
            intersects: Some(point(5.0, 6.0)),
            touches: Some(point(7.0, 8.0)),
            crosses: Some(point(9.0, 10.0)),
            within: Some(point(11.0, 12.0)),
            contains: Some(point(13.0, 14.0)),
            overlaps: Some(point(15.0, 16.0)),
            is_null: None,
        }),
        ..Default::default()
    };

    let (conditions, values) = filter.to_sql_conditions();
    assert_eq!(conditions.len(), 8);
    assert_eq!(values.len(), 8);
    assert_eq!(
        conditions[0],
        "ST_Equals(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))"
    );
    assert_eq!(
        conditions[1],
        "NOT ST_Intersects(location, ST_SetSRID(ST_GeomFromGeoJSON($2::jsonb), 4326))"
    );
    assert!(conditions[2].starts_with("ST_Intersects(location"));
    assert!(conditions[3].starts_with("ST_Touches(location"));
    assert!(conditions[4].starts_with("ST_Crosses(location"));
    assert!(conditions[5].starts_with("ST_Within(location"));
    assert!(conditions[6].starts_with("ST_Contains(location"));
    assert!(conditions[7].starts_with("ST_Overlaps(location"));
}

#[test]
fn spatial_filter_null_check_does_not_bind_values() {
    let filter = PlaceWhereInput {
        location: Some(SpatialFilter {
            is_null: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };

    let (conditions, values) = filter.to_sql_conditions();
    assert_eq!(conditions, vec!["location IS NOT NULL"]);
    assert!(values.is_empty());
}

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn leak_migration(
    plan: &graphql_orm::graphql::orm::MigrationPlan,
    version: String,
    description: &'static str,
) -> graphql_orm::graphql::orm::Migration {
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    graphql_orm::graphql::orm::Migration {
        version: Box::leak(version.into_boxed_str()),
        description,
        statements,
    }
}

async fn setup_postgres_pool() -> Result<graphql_orm::sqlx::PgPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test".to_string()
    });
    let pool = graphql_orm::sqlx::PgPool::connect(&database_url).await?;
    for table in ["spatial_places", "spatial_regions", "spatial_trails"] {
        graphql_orm::sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(&pool)
            .await?;
    }
    Ok(pool)
}

async fn count_places(
    pool: &graphql_orm::sqlx::PgPool,
    filter: SpatialFilter,
) -> Result<usize, graphql_orm::sqlx::Error> {
    Ok(Place::query(pool)
        .filter(PlaceWhereInput {
            location: Some(filter),
            ..Default::default()
        })
        .fetch_all()
        .await?
        .len())
}

async fn count_regions(
    pool: &graphql_orm::sqlx::PgPool,
    filter: SpatialFilter,
) -> Result<usize, graphql_orm::sqlx::Error> {
    Ok(Region::query(pool)
        .filter(RegionWhereInput {
            boundary: Some(filter),
            ..Default::default()
        })
        .fetch_all()
        .await?
        .len())
}

async fn count_trails(
    pool: &graphql_orm::sqlx::PgPool,
    filter: SpatialFilter,
) -> Result<usize, graphql_orm::sqlx::Error> {
    Ok(Trail::query(pool)
        .filter(TrailWhereInput {
            path: Some(filter),
            ..Default::default()
        })
        .fetch_all()
        .await?
        .len())
}

#[tokio::test]
async fn postgis_spatial_fields_indexes_and_predicates_round_trip()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    use async_graphql::Request;
    use graphql_orm::graphql::orm::{
        DatabaseBackend, Entity, MigrationRunner, SpatialGeometryType, build_migration_plan,
        introspect_schema,
    };

    let pool = setup_postgres_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <Place as Entity>::metadata(),
        <Region as Entity>::metadata(),
        <Trail as Entity>::metadata(),
    ]);
    assert_eq!(target_schema.extensions, vec!["postgis".to_string()]);
    let plan = build_migration_plan(
        DatabaseBackend::Postgres,
        &graphql_orm::graphql::orm::SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target_schema,
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "CREATE EXTENSION IF NOT EXISTS postgis")
    );
    assert!(plan.statements.iter().any(|statement| {
        statement == "CREATE INDEX \"idx_spatial_places_location_spatial\" ON \"spatial_places\" USING GIST (\"location\")"
    }));

    let version = format!(
        "2026062801_spatial_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    database
        .apply_migrations(&[leak_migration(&plan, version, "postgis_spatial_contract")])
        .await?;

    let created_place = Place::insert(
        &pool,
        CreatePlaceInput {
            location: point_value(0.0, 0.0),
        },
    )
    .await?;
    let fetched_place = Place::get(&pool, &created_place.id)
        .await?
        .expect("created place should be readable");
    assert_eq!(fetched_place.location["type"], "Point");
    assert_eq!(fetched_place.location["coordinates"][0].as_f64(), Some(0.0));

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
    Region::insert(
        &pool,
        CreateRegionInput {
            boundary: square.clone(),
        },
    )
    .await?;
    Trail::insert(
        &pool,
        CreateTrailInput {
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
                disjoint: Some(point(10.0, 10.0)),
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
        1
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

    let updated = Place::update_by_id(
        &database,
        &created_place.id,
        UpdatePlaceInput {
            location: Some(point_value(2.0, 2.0)),
        },
    )
    .await?
    .expect("updated place should be returned");
    assert_eq!(updated.location["coordinates"][0].as_f64(), Some(2.0));
    assert_eq!(
        count_places(
            &pool,
            SpatialFilter {
                equals: Some(point(2.0, 2.0)),
                ..Default::default()
            },
        )
        .await?,
        1
    );

    let schema = schema_builder(database.clone())
        .data("spatial-test-user".to_string())
        .finish();
    let graph_response = schema
        .execute(Request::new(
            r#"
            query {
              places(where: { location: { equals: { type: "Point", coordinates: [2.0, 2.0] } } }) {
                edges { node { id location } }
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
    assert_eq!(graph_json["places"]["edges"].as_array().unwrap().len(), 1);
    assert_eq!(
        graph_json["places"]["edges"][0]["node"]["location"]["type"],
        "Point"
    );

    let introspected = introspect_schema(&pool).await?;
    assert!(
        introspected
            .extensions
            .iter()
            .any(|extension| extension == "postgis")
    );
    let places_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "spatial_places")
        .expect("spatial_places table should be introspected");
    let location_column = places_table
        .columns
        .iter()
        .find(|column| column.name == "location")
        .expect("location column should be introspected");
    assert_eq!(
        location_column.spatial.map(|spatial| spatial.geometry_type),
        Some(SpatialGeometryType::Point)
    );
    assert!(
        places_table
            .indexes
            .iter()
            .any(|index| index.name == "idx_spatial_places_location_spatial" && index.is_spatial)
    );

    Ok(())
}
