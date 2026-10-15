//! Host-neutral boundary: supply authenticated current authority, authorize the
//! exact projection/key-read intent, then make one restricted read. No optional
//! filter is accepted here; hosts adding one must authorize its original intent
//! before predicate construction, not inspect folded referenced_fields.
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod checked {
    use graphql_orm::{db::Database, graphql::orm::*};
    use std::{future::Future, pin::Pin};

    #[derive(Debug)]
    pub enum HostReadError {
        Denied,
        Read(RuntimeKeyReadError),
    }

    /// Implemented by the trusted host adapter, never by an untrusted request.
    /// Authenticate and revalidate current principal/field/key-read policy and the
    /// active catalog revision here. Return coherent backend auth and a required
    /// row restriction from that same authority. Missing authority must deny.
    pub trait CurrentReadAuthority: Send + Sync {
        fn authorize<'a>(
            &'a self,
            schema: &'a ValidatedRuntimeSchema,
            key: &'a RuntimeKey,
            projection: &'a RuntimeProjection,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<(RuntimePredicate, DbAuthContext), HostReadError>>
                    + Send
                    + 'a,
            >,
        >;
    }

    pub async fn read_authorized<B: OrmBackend + RuntimeRowDecoder>(
        db: &Database<B>,
        schema: &ValidatedRuntimeSchema,
        key: &RuntimeKey,
        projection: &RuntimeProjection,
        authority: Option<&dyn CurrentReadAuthority>,
    ) -> Result<Option<RuntimeRecord>, HostReadError> {
        let authority = authority.ok_or(HostReadError::Denied)?;
        let (required_row, auth) = authority.authorize(schema, key, projection).await?;
        let request = schema
            .runtime_key_read_request(key, projection, &required_row, None, Default::default())
            .map_err(HostReadError::Read)?;
        db.execute_runtime_key_read(&request, Some(&auth))
            .await
            .map_err(HostReadError::Read)
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
pub use checked::*;

fn main() {} // The host provides its own transport/authority and owned database.
