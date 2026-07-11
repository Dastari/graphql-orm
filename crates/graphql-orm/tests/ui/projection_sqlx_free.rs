use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_boundary", plural = "ProjectionBoundary")]
#[graphql_orm(projection(
    name = "BoundaryProjection",
    fields = [id, label],
    private = true
))]
struct BoundaryEntity {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    label: String,
    #[graphql_orm(private, sensitive)]
    secret: Vec<u8>,
}

#[allow(dead_code)]
async fn repository_boundary(
    database: &Database<SqliteBackend>,
) -> graphql_orm::Result<Option<BoundaryProjection>> {
    BoundaryProjection::query(database)
        .filter(BoundaryEntityWhereInput {
            label: Some(StringFilter {
                eq: Some("visible".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .order_by(BoundaryEntityOrderByInput {
            id: Some(OrderDirection::Asc),
        })
        .fetch_first()
        .await
}

#[allow(dead_code)]
async fn transaction_boundary(
    database: &Database<SqliteBackend>,
) -> Result<Option<BoundaryProjection>, TransactionError> {
    database
        .transaction(TransactionMode::Default, |transaction| {
            Box::pin(async move {
                transaction
                    .project::<BoundaryProjection>()
                    .fetch_first()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
}

fn main() {}
