//! A request admitted for one backend cannot be used for entity admission on
//! another: the admitted request is typed by the backend whose RLS request
//! context was checked (#921). Compiled only where both backends are enabled.
use graphql_orm::async_graphql::{Context, Result};
use graphql_orm::graphql::orm::{
    __admit_generated_write_request, DatabaseRls, Entity, GeneratedWriteAdmission,
    GeneratedWriteConfig, GeneratedWriteKind,
};
use graphql_orm::{PostgresBackend, SqliteBackend};

#[allow(dead_code)]
async fn admit<T>(ctx: &Context<'_>) -> Result<()>
where
    T: DatabaseRls + Entity + GeneratedWriteConfig + 'static,
{
    let request = __admit_generated_write_request::<T, SqliteBackend>(
        ctx,
        GeneratedWriteKind::Delete,
        &[],
    )?;
    let _admission: GeneratedWriteAdmission<'_, PostgresBackend> =
        request.__admit_entity(ctx).await?;
    Ok(())
}

fn main() {}
