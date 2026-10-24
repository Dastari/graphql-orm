//! Semantic field formats served two ways: a file reference projected into a
//! host output object, and an email address served as its plain scalar.
//! Run with exactly one backend and `runtime-graphql`; PostgreSQL uses owned Docker.
#[cfg(all(
    feature = "runtime-graphql",
    any(feature = "sqlite", feature = "postgres")
))]
mod example {
    use graphql_orm::{
        async_graphql::Request,
        futures::future::BoxFuture,
        graphql::{orm::*, runtime::*},
        prelude::*,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };
    #[cfg(feature = "postgres")]
    type Backend = PostgresBackend;
    #[cfg(not(feature = "postgres"))]
    type Backend = SqliteBackend;

    /// Example host projector; a real host would read its own catalogue here.
    struct FileProjector {
        contract: RuntimeProjectionContract,
        names: BTreeMap<uuid::Uuid, String>,
    }
    impl RuntimeFieldProjector for FileProjector {
        fn contract(&self) -> &RuntimeProjectionContract {
            &self.contract
        }
        fn resolve<'a>(
            &'a self,
            batch: RuntimeProjectionBatch<'a>,
        ) -> BoxFuture<'a, Result<RuntimeProjectionResults, RuntimeProjectionError>> {
            Box::pin(async move {
                let mut results = RuntimeProjectionResults::new();
                for value in batch.values {
                    let RuntimeValue::Uuid(id) = value else {
                        return Err(RuntimeProjectionError::Invalid);
                    };
                    results.insert(
                        value.clone(),
                        match self.names.get(id) {
                            Some(name) => RuntimeProjected::Object(BTreeMap::from([
                                ("id".into(), value.clone()),
                                ("name".into(), RuntimeValue::String(name.clone())),
                            ])),
                            None => RuntimeProjected::Absent,
                        },
                    );
                }
                Ok(results)
            })
        }
    }

    /// The example releases its isolated public fixture; it is not a policy.
    struct PublicRead;
    impl RuntimeReadAuthority for PublicRead {
        fn authorize<'a>(
            &'a self,
            _check: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
        }
    }

    fn field(id: &str, kind: RuntimeValueKind, format: Option<RuntimeFieldFormat>) -> RuntimeField {
        RuntimeField {
            description: None,
            classification: None,
            export: None,
            id: FieldId::new(id).unwrap(),
            api_name: id.into(),
            physical_column: id.into(),
            value_kind: kind,
            nullable: id != "id",
            unique: false,
            filterable: false,
            sortable: id == "id",
            generated: false,
            default: None,
            format,
        }
    }

    pub async fn run(database: Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
        let file_ref = RuntimeFormatNamespace::new("example.file_ref")?;
        let email = RuntimeFormatNamespace::new("example.email")?;
        let definition = RuntimeSchema {
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: vec![RuntimeCollection {
                description: None,
                classification: None,
                id: CollectionId::new("members")?,
                api_type_name: "Member".into(),
                api_plural_name: "Members".into(),
                physical_table: "example_members".into(),
                primary_key: vec![FieldId::new("id")?],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    field("id", RuntimeValueKind::Integer, None),
                    field(
                        "email",
                        RuntimeValueKind::String,
                        Some(RuntimeFieldFormat::new(
                            email.clone(),
                            1,
                            RuntimeFormatPayload::empty(),
                        )?),
                    ),
                    field(
                        "avatar",
                        RuntimeValueKind::Uuid,
                        Some(RuntimeFieldFormat::new(
                            file_ref.clone(),
                            1,
                            RuntimeFormatPayload::empty(),
                        )?),
                    ),
                ],
                relations: vec![],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![],
                label_fields: Vec::new(),
            }],
        };
        let schema = Arc::new(definition.validate()?);

        // Formats never change the physical target: plan and apply as usual.
        let target = schema.physical_schema::<Backend>(Default::default())?;
        let ownership = ManagedTableSet::new(["example_members".into()])?;
        let plan = database
            .schema()
            .plan_owned_migration("v1", "members", &target, &ownership, PlanOptions::strict())
            .await?;
        database
            .schema()
            .apply_owned_migration(&plan, Default::default())
            .await?;
        let avatar = uuid::Uuid::from_u128(1);
        graphql_orm::sqlx::query(&format!(
            "INSERT INTO example_members (id, email, avatar) VALUES (1, 'ada@example.test', '{avatar}'), (2, NULL, NULL)"
        ))
        .execute(database.pool())
        .await?;

        let projector = Arc::new(FileProjector {
            contract: RuntimeProjectionContract {
                namespace: file_ref,
                versions: 1..=1,
                value_kinds: BTreeSet::from([RuntimeValueKind::Uuid]),
                object: RuntimeProjectedObject::new(
                    "ExampleFile",
                    vec![
                        RuntimeProjectedField::new("id", RuntimeValueKind::Uuid, false),
                        RuntimeProjectedField::new("name", RuntimeValueKind::String, false),
                    ],
                )?,
                may_be_absent: true,
            },
            names: BTreeMap::from([(avatar, "avatar.png".to_owned())]),
        });
        let module = RuntimeGraphqlModule::compile(
            schema.clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .scalar_format(email, 1..=1)
                .project_field(FieldId::new("avatar")?, projector),
        )?;
        let api = RuntimeGraphqlComposer::new(database, "Query", Default::default())?
            .install(module)?
            .finish()?;
        let sdl = api.sdl();
        assert!(sdl.contains("email: String") && sdl.contains("avatar: ExampleFile"));

        let collection = schema.resolve_collection(&CollectionId::new("members")?)?;
        let email_field = schema.resolve_field(&collection, &FieldId::new("email")?)?;
        println!(
            "email descriptor: {}",
            email_field
                .format()
                .map(RuntimeFieldFormat::canonical)
                .unwrap_or_default()
        );
        let response = api
            .execute(
                Request::new(
                    "{ members(orderBy:[{field:id}]) { edges { node { id email avatar { name } } } } }",
                )
                .data(RuntimeGraphqlRequest::<Backend>::new(
                    schema.fingerprint(),
                    Arc::new(PublicRead),
                    None,
                    Default::default(),
                )),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        println!("{}", response.data.into_json()?);
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
        let mut owned = owned_postgres::OwnedPostgres::start("runtime-projection-example")?;
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
