use graphql_orm::prelude::*;

struct AuthProbe;

#[graphql_orm::async_graphql::Object]
impl AuthProbe {
    async fn subject_id(
        &self,
        ctx: &graphql_orm::async_graphql::Context<'_>,
    ) -> graphql_orm::async_graphql::Result<String> {
        ctx.auth_subject().map(|subject| subject.id)
    }

    async fn subject_summary(
        &self,
        ctx: &graphql_orm::async_graphql::Context<'_>,
    ) -> graphql_orm::async_graphql::Result<String> {
        let subject = ctx.auth_subject()?;
        Ok(format!(
            "{}|{}|{}|{}",
            subject.id,
            subject.roles.join(","),
            subject.scopes.join(","),
            subject.tenant_id.unwrap_or_default()
        ))
    }
}

#[tokio::test]
async fn auth_ext_reports_missing_auth() {
    let schema = graphql_orm::async_graphql::Schema::build(
        AuthProbe,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();

    let response = schema.execute("{ subjectId }").await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(response.errors[0].message, "unauthenticated");
    let extensions = response.errors[0].extensions.as_ref().expect("extensions");
    assert_eq!(
        extensions.get("code").map(|value| value.to_string()),
        Some("\"UNAUTHENTICATED\"".to_string())
    );
}

#[tokio::test]
async fn auth_ext_ignores_a_bare_string_in_context() -> Result<(), Box<dyn std::error::Error>> {
    // Any `String` in context data is not an authenticated subject: the
    // subject must be the typed `AuthSubject`.
    let schema = graphql_orm::async_graphql::Schema::build(
        AuthProbe,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .data("unrelated-string".to_string())
    .finish();

    let response = schema.execute("{ subjectSummary }").await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(response.errors[0].message, "unauthenticated");

    let response = schema
        .execute(
            graphql_orm::async_graphql::Request::new("{ subjectId }")
                .data("request-string".to_string()),
        )
        .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(response.errors[0].message, "unauthenticated");
    Ok(())
}

#[tokio::test]
async fn auth_ext_preserves_full_subject() -> Result<(), Box<dyn std::error::Error>> {
    let schema = graphql_orm::async_graphql::Schema::build(
        AuthProbe,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .data(AuthSubject::from_parts(
        "subject-1",
        vec!["admin".to_string()],
        vec!["records.read".to_string(), "records.write".to_string()],
        Some("tenant-a".to_string()),
    ))
    .finish();

    let response = schema.execute("{ subjectSummary }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    assert_eq!(
        json["subjectSummary"],
        "subject-1|admin|records.read,records.write|tenant-a"
    );
    Ok(())
}

#[cfg(feature = "sqlite")]
mod sqlite_auth {
    use super::*;
    use graphql_orm::graphql::orm::{EntityAccessKind, EntityAccessSurface, EntityPolicy};

    struct ScopePolicyProbe;

    #[graphql_orm::async_graphql::Object]
    impl ScopePolicyProbe {
        async fn read(
            &self,
            ctx: &graphql_orm::async_graphql::Context<'_>,
        ) -> graphql_orm::async_graphql::Result<bool> {
            let db = ctx.data_unchecked::<graphql_orm::db::Database<SqliteBackend>>();
            let policy = ctx.data_unchecked::<ScopeEntityPolicy>();
            policy
                .can_access_entity(
                    Some(ctx),
                    db,
                    "Probe",
                    None,
                    EntityAccessKind::Read,
                    EntityAccessSurface::GraphqlQuery,
                )
                .await
        }

        async fn write(
            &self,
            ctx: &graphql_orm::async_graphql::Context<'_>,
        ) -> graphql_orm::async_graphql::Result<bool> {
            let db = ctx.data_unchecked::<graphql_orm::db::Database<SqliteBackend>>();
            let policy = ctx.data_unchecked::<ScopeEntityPolicy>();
            policy
                .can_access_entity(
                    Some(ctx),
                    db,
                    "Probe",
                    None,
                    EntityAccessKind::Write,
                    EntityAccessSurface::GraphqlMutation,
                )
                .await
        }
    }

    async fn sqlite_database() -> Result<graphql_orm::db::Database<SqliteBackend>, sqlx::Error> {
        graphql_orm::db::Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await
    }

    async fn execute_scope_probe(
        policy: ScopeEntityPolicy,
        subject: Option<AuthSubject>,
    ) -> Result<graphql_orm::async_graphql::Response, sqlx::Error> {
        let database = sqlite_database().await?;
        let mut builder = graphql_orm::async_graphql::Schema::build(
            ScopePolicyProbe,
            graphql_orm::async_graphql::EmptyMutation,
            graphql_orm::async_graphql::EmptySubscription,
        )
        .data(database)
        .data(policy);
        if let Some(subject) = subject {
            builder = builder.data(subject);
        }
        let schema = builder.finish();
        Ok(schema.execute("{ read write }").await)
    }

    #[tokio::test]
    async fn scope_entity_policy_matches_read_and_write_scopes_exactly()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = ScopeEntityPolicy::new(&["records.read"], &["records.write"]);

        let read_subject =
            AuthSubject::from_parts("reader", Vec::new(), vec!["records.read".to_string()], None);
        let read_response = execute_scope_probe(policy, Some(read_subject)).await?;
        assert!(
            read_response.errors.is_empty(),
            "{:?}",
            read_response.errors
        );
        let read_json = read_response.data.into_json()?;
        assert_eq!(read_json["read"], true);
        assert_eq!(read_json["write"], false);

        let write_subject = AuthSubject::from_parts(
            "writer",
            Vec::new(),
            vec!["records.write".to_string()],
            None,
        );
        let write_response = execute_scope_probe(policy, Some(write_subject)).await?;
        assert!(
            write_response.errors.is_empty(),
            "{:?}",
            write_response.errors
        );
        let write_json = write_response.data.into_json()?;
        assert_eq!(write_json["read"], false);
        assert_eq!(write_json["write"], true);

        let wildcard_like_subject =
            AuthSubject::from_parts("wild", Vec::new(), vec!["records.*".to_string()], None);
        let wildcard_response = execute_scope_probe(policy, Some(wildcard_like_subject)).await?;
        assert!(
            wildcard_response.errors.is_empty(),
            "{:?}",
            wildcard_response.errors
        );
        let wildcard_json = wildcard_response.data.into_json()?;
        assert_eq!(wildcard_json["read"], false);
        assert_eq!(wildcard_json["write"], false);

        Ok(())
    }

    #[tokio::test]
    async fn scope_entity_policy_requires_auth_when_configured()
    -> Result<(), Box<dyn std::error::Error>> {
        let response =
            execute_scope_probe(ScopeEntityPolicy::new(&["records.read"], &[]), None).await?;
        assert_eq!(response.errors.len(), 2);
        assert!(
            response
                .errors
                .iter()
                .all(|error| error.message.contains("unauthenticated"))
        );
        Ok(())
    }

    async fn probe_json(
        policy: ScopeEntityPolicy,
        subject: Option<AuthSubject>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let response = execute_scope_probe(policy, subject).await?;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        Ok(response.data.into_json()?)
    }

    #[tokio::test]
    async fn scope_entity_policy_empty_scope_list_denies() -> Result<(), Box<dyn std::error::Error>>
    {
        let subject = || {
            Some(AuthSubject::from_parts(
                "subject",
                Vec::new(),
                vec!["records.read".to_string()],
                None,
            ))
        };
        let json = probe_json(ScopeEntityPolicy::new(&[], &[]), subject()).await?;
        assert_eq!(json["read"], false);
        assert_eq!(json["write"], false);

        let json = probe_json(ScopeEntityPolicy::new(&["records.read"], &[]), subject()).await?;
        assert_eq!(json["read"], true);
        assert_eq!(json["write"], false);

        // Without a subject, an optional policy never grants on an empty list.
        let json = probe_json(ScopeEntityPolicy::optional(&[], &[]), None).await?;
        assert_eq!(json["read"], false);
        assert_eq!(json["write"], false);
        let json = probe_json(ScopeEntityPolicy::optional(&[], &[]), subject()).await?;
        assert_eq!(json["read"], false);
        assert_eq!(json["write"], false);
        Ok(())
    }

    #[tokio::test]
    async fn scope_entity_policy_any_authenticated_is_explicit()
    -> Result<(), Box<dyn std::error::Error>> {
        let subject = || Some(AuthSubject::new("subject"));
        let json = probe_json(ScopeEntityPolicy::authenticated(), subject()).await?;
        assert_eq!(json["read"], true);
        assert_eq!(json["write"], true);

        let response = execute_scope_probe(ScopeEntityPolicy::authenticated(), None).await?;
        assert_eq!(response.errors.len(), 2);

        let policy = ScopeEntityPolicy::new(&[], &["records.write"])
            .with_read(ScopeRequirement::AnyAuthenticated);
        let json = probe_json(policy, subject()).await?;
        assert_eq!(json["read"], true);
        assert_eq!(json["write"], false);

        // `optional` with any-authenticated still refuses an absent subject.
        let policy = ScopeEntityPolicy::optional(&[], &[])
            .with_read(ScopeRequirement::AnyAuthenticated)
            .with_write(ScopeRequirement::AnyAuthenticated);
        let json = probe_json(policy, None).await?;
        assert_eq!(json["read"], false);
        assert_eq!(json["write"], false);
        Ok(())
    }

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "public_notes",
        plural = "PublicNotes",
        auth = "none"
    )]
    struct PublicNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,
        #[sortable]
        pub title: String,
    }

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "private_notes",
        plural = "PrivateNotes",
        auth = "required"
    )]
    struct PrivateNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,
        #[sortable]
        pub title: String,
    }

    schema_roots! {
        backend: "sqlite",
        schema_policy: "managed",
        auth: "none",
        query_custom_ops: [],
        entities: [PublicNote, PrivateNote],
        generated_mutations: "none",
    }

    async fn setup_notes_database()
    -> Result<graphql_orm::db::Database<SqliteBackend>, Box<dyn std::error::Error>> {
        let database =
            graphql_orm::db::Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
        let pool = database.pool();
        sqlx::query("CREATE TABLE public_notes (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE TABLE private_notes (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO public_notes (id, title) VALUES (?1, ?2)")
            .bind(graphql_orm::uuid::Uuid::new_v4().to_string())
            .bind("Public")
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO private_notes (id, title) VALUES (?1, ?2)")
            .bind(graphql_orm::uuid::Uuid::new_v4().to_string())
            .bind("Private")
            .execute(pool)
            .await?;
        Ok(database)
    }

    #[tokio::test]
    async fn generated_auth_none_stays_public_and_required_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = setup_notes_database().await?;
        let schema = schema_builder(database.clone()).finish();

        let public_response = schema
            .execute("{ publicNotes { edges { node { title } } pageInfo { totalCount } } }")
            .await;
        assert!(
            public_response.errors.is_empty(),
            "{:?}",
            public_response.errors
        );
        let public_json = public_response.data.into_json()?;
        assert_eq!(public_json["publicNotes"]["pageInfo"]["totalCount"], 1);
        assert_eq!(
            public_json["publicNotes"]["edges"][0]["node"]["title"],
            "Public"
        );

        let private_response = schema
            .execute("{ privateNotes { edges { node { title } } } }")
            .await;
        assert_eq!(private_response.errors.len(), 1);
        assert_eq!(private_response.errors[0].message, "unauthenticated");

        let authed_schema = schema_builder(database)
            .data(AuthSubject::new("subject-1"))
            .finish();
        let authed_private_response = authed_schema
            .execute("{ privateNotes { edges { node { title } } pageInfo { totalCount } } }")
            .await;
        assert!(
            authed_private_response.errors.is_empty(),
            "{:?}",
            authed_private_response.errors
        );
        let private_json = authed_private_response.data.into_json()?;
        assert_eq!(private_json["privateNotes"]["pageInfo"]["totalCount"], 1);
        assert_eq!(
            private_json["privateNotes"]["edges"][0]["node"]["title"],
            "Private"
        );

        Ok(())
    }
}
