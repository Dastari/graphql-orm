//! Disposable host composition example. Keys exist only for this process.
//! Run with exactly one backend and `runtime-graphql`; PostgreSQL uses owned Docker.
#[cfg(all(
    feature = "runtime-graphql",
    any(feature = "sqlite", feature = "postgres")
))]
mod example {
    use chacha20poly1305::{
        ChaCha20Poly1305, KeyInit,
        aead::{Aead, AeadCore, OsRng, Payload},
    };
    use graphql_orm::{
        async_graphql::{
            Request,
            dynamic::{FieldFuture, FieldValue, TypeRef},
        },
        futures::future::BoxFuture,
    };
    use graphql_orm::{
        graphql::{orm::*, runtime::*},
        prelude::*,
    };
    use std::sync::Arc;
    #[cfg(feature = "postgres")]
    type Backend = PostgresBackend;
    #[cfg(not(feature = "postgres"))]
    type Backend = SqliteBackend;
    #[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
    #[repository_entity(table = "public_notes", plural = "PublicNotes")]
    struct PublicNote {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        id: String,
        exact: i64,
        label: String,
    }
    struct PublicRead;
    impl RuntimeReadAuthority for PublicRead {
        fn authorize<'a>(
            &'a self,
            _: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            // This disposable data set is intentionally public. A production host
            // checks projection/filter/order/relation/count capabilities here.
            Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
        }
    }
    struct HostCipher(ChaCha20Poly1305);
    impl RuntimeCursorProtector for HostCipher {
        fn seal<'a>(
            &'a self,
            context: &'a RuntimeCursorContext,
            plaintext: &'a str,
            limits: RuntimeCursorProtectionLimits,
        ) -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>> {
            Box::pin(async move {
                let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
                let mut bytes = nonce.to_vec();
                bytes.extend(
                    self.0
                        .encrypt(
                            &nonce,
                            Payload {
                                msg: plaintext.as_bytes(),
                                aad: context.associated_data(),
                            },
                        )
                        .map_err(|_| RuntimeCursorProtectionError::CursorUnavailable)?,
                );
                RuntimeSealedCursor::new("example-ephemeral", bytes, limits)
            })
        }
        fn open<'a>(
            &'a self,
            context: &'a RuntimeCursorContext,
            key: &'a str,
            bytes: &'a [u8],
            limits: RuntimeCursorProtectionLimits,
        ) -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>> {
            Box::pin(async move {
                if key != "example-ephemeral" {
                    return Err(RuntimeCursorProtectionError::CursorUnavailable);
                }
                if bytes.len() < 28
                    || bytes.len()
                        > limits
                            .max_plaintext_bytes
                            .saturating_add(limits.max_overhead_bytes)
                {
                    return Err(RuntimeCursorProtectionError::InvalidCursor);
                }
                let value = self
                    .0
                    .decrypt(
                        (&bytes[..12]).into(),
                        Payload {
                            msg: &bytes[12..],
                            aad: context.associated_data(),
                        },
                    )
                    .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?;
                String::from_utf8(value).map_err(|_| RuntimeCursorProtectionError::InvalidCursor)
            })
        }
    }
    pub async fn run(database: Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
        // Typed repository setup is independent of GraphQL registration.
        let schema =
            Arc::new(RuntimeSchema::from_static_entities(&[PublicNote::metadata()])?.validate()?);
        let target = schema.physical_schema::<Backend>(Default::default())?;
        let ownership = ManagedTableSet::new(["public_notes".into()])?;
        let plan = database
            .schema()
            .plan_owned_migration(
                "example",
                "disposable notes",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await?;
        database
            .schema()
            .apply_owned_migration(&plan, ApplyOptions::default())
            .await?;
        PublicNote::insert(
            &database,
            CreatePublicNoteInput {
                id: "note-one".into(),
                exact: 9_007_199_254_740_993,
                label: "Exact integer".into(),
            },
        )
        .await?;
        let module = RuntimeGraphqlModule::compile(schema.clone(), Default::default())?;
        let cipher = HostCipher(ChaCha20Poly1305::new(&ChaCha20Poly1305::generate_key(
            &mut OsRng,
        )));
        let api = RuntimeGraphqlComposer::new(database, "Query", Default::default())?
            .cursor_protection(
                Arc::new(cipher),
                RuntimeCursorAudience::new("upstream-example")?,
                Default::default(),
            )?
            .query_field(RuntimeHostField::new(
                "health",
                TypeRef::named_nn("Boolean"),
                |_| FieldFuture::new(async { Ok(Some(FieldValue::value(true))) }),
            ))?
            .install(module)?
            .finish()?;
        let response = api.execute(Request::new("{ health publicNotes(first:1) { edges { cursor node { id exact label } } totalCount } }")
            .data(RuntimeGraphqlRequest::<Backend>::new(schema.fingerprint(), Arc::new(PublicRead), None, Default::default()).with_cursor_scope("public-example"))).await;
        if !response.errors.is_empty() {
            return Err("runtime GraphQL example failed".into());
        }
        let data = response.data.into_json()?;
        assert_eq!(
            data["publicNotes"]["edges"][0]["node"]["exact"],
            "9007199254740993"
        );
        assert_eq!(data["publicNotes"]["totalCount"], "1");
        println!("authorized runtime GraphQL read/count and protected cursor passed");
        Ok(())
    }
}
#[cfg(all(feature = "runtime-graphql", feature = "postgres"))]
#[path = "../tests/support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(all(
    feature = "runtime-graphql",
    any(feature = "sqlite", feature = "postgres")
))]
#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "postgres")]
    {
        let mut owned = owned_postgres::OwnedPostgres::start("runtime-gql-example")?;
        let result =
            example::run(graphql_orm::db::Database::connect_postgres(&owned.url).await?).await;
        owned.cleanup()?;
        result
    }
    #[cfg(not(feature = "postgres"))]
    {
        example::run(graphql_orm::db::Database::connect_sqlite("sqlite::memory:").await?).await
    }
}
#[cfg(not(all(
    feature = "runtime-graphql",
    any(feature = "sqlite", feature = "postgres")
)))]
pub fn main() {
    eprintln!("enable runtime-graphql and exactly one SQLite/PostgreSQL backend");
}
