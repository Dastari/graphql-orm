use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "workspaces", plural = "Workspaces", default_sort = "name ASC")]
struct Workspace {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql_orm(private)]
    pub owner_actor_id: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "workspace_memberships",
    plural = "WorkspaceMemberships",
    default_sort = "role ASC"
)]
struct WorkspaceMembership {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub workspace_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub role: String,

    #[graphql_orm(private)]
    pub updated_by_actor_id: Option<String>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Workspace, WorkspaceMembership],
}

#[derive(Clone, Default)]
struct TrustedWriteAudit {
    entries: Arc<Mutex<Vec<String>>>,
}

impl TrustedWriteAudit {
    fn record(&self, value: impl Into<String>) {
        self.entries
            .lock()
            .expect("audit lock poisoned")
            .push(value.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.entries.lock().expect("audit lock poisoned").clone()
    }
}

impl graphql_orm::graphql::orm::WriteInputTransform for TrustedWriteAudit {
    fn before_create_with_context<'a>(
        &'a self,
        write_ctx: &'a mut graphql_orm::graphql::orm::WriteInputContext<'_, '_>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.record(format!(
                "create:{}:{:?}",
                write_ctx.entity_name(),
                write_ctx.origin()
            ));

            match write_ctx.entity_name() {
                "Workspace" => {
                    let actor = write_ctx
                        .actor::<String>()
                        .unwrap_or_else(|| "system".to_string());
                    let input = input
                        .downcast_mut::<CreateWorkspaceInput>()
                        .ok_or_else(|| {
                            async_graphql::Error::new("unexpected workspace create input")
                        })?;
                    input.owner_actor_id = actor;
                }
                "WorkspaceMembership" => {
                    let input = input
                        .downcast_mut::<CreateWorkspaceMembershipInput>()
                        .ok_or_else(|| {
                            async_graphql::Error::new("unexpected membership create input")
                        })?;
                    let exists = write_ctx
                        .query::<Workspace>()
                        .filter(WorkspaceWhereInput {
                            id: Some(UuidFilter {
                                eq: Some(input.workspace_id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .exists()
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                    if !exists {
                        return Err(async_graphql::Error::new(
                            "workspace membership validation could not see workspace row",
                        ));
                    }
                    input.updated_by_actor_id = Some("hook-create".to_string());
                }
                _ => {}
            }

            Ok(())
        })
    }

    fn before_update_with_context<'a>(
        &'a self,
        write_ctx: &'a mut graphql_orm::graphql::orm::WriteInputContext<'_, '_>,
        existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.record(format!(
                "update:{}:{:?}",
                write_ctx.entity_name(),
                write_ctx.origin()
            ));

            if write_ctx.entity_name() == "WorkspaceMembership" {
                let existing = existing_row
                    .and_then(|row| row.downcast_ref::<WorkspaceMembership>())
                    .ok_or_else(|| async_graphql::Error::new("missing membership row"))?;
                let exists = write_ctx
                    .query::<Workspace>()
                    .filter(WorkspaceWhereInput {
                        id: Some(UuidFilter {
                            eq: Some(existing.workspace_id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .exists()
                    .await
                    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                if !exists {
                    return Err(async_graphql::Error::new(
                        "workspace membership update validation lost transaction visibility",
                    ));
                }
                let input = input
                    .downcast_mut::<UpdateWorkspaceMembershipInput>()
                    .ok_or_else(|| {
                        async_graphql::Error::new("unexpected membership update input")
                    })?;
                input.updated_by_actor_id = Some(Some("hook-update".to_string()));
            }

            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct WorkspaceLifecycleHook;

impl graphql_orm::graphql::orm::MutationHook for WorkspaceLifecycleHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            use graphql_orm::graphql::orm::{ChangeAction, MutationPhase};

            match (event.phase.clone(), event.action, event.entity_name) {
                (MutationPhase::After, ChangeAction::Created, "Workspace") => {
                    let workspace = event.after::<Workspace>()?.ok_or_else(|| {
                        async_graphql::Error::new("missing workspace after state")
                    })?;
                    hook_ctx
                        .insert::<WorkspaceMembership>(CreateWorkspaceMembershipInput {
                            workspace_id: workspace.id,
                            role: "owner".to_string(),
                            updated_by_actor_id: None,
                        })
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                }
                (MutationPhase::After, ChangeAction::Updated, "Workspace") => {
                    let workspace = event.after::<Workspace>()?.ok_or_else(|| {
                        async_graphql::Error::new("missing workspace after state")
                    })?;
                    hook_ctx
                        .update_where::<WorkspaceMembership>(
                            WorkspaceMembershipWhereInput {
                                workspace_id: Some(UuidFilter {
                                    eq: Some(workspace.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                            UpdateWorkspaceMembershipInput {
                                role: Some("owner-updated".to_string()),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                }
                _ => {}
            }

            Ok(())
        })
    }
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    Ok(sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS workspace_memberships")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS workspaces")
        .execute(&pool)
        .await?;
    Ok(pool)
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
) -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{DatabaseBackend, Entity, Migration, build_migration_plan};

    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <Workspace as Entity>::metadata(),
        <WorkspaceMembership as Entity>::metadata(),
    ]);
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            DatabaseBackend::Postgres
        } else {
            DatabaseBackend::Sqlite
        },
        &graphql_orm::graphql::orm::SchemaModel { tables: Vec::new() },
        &target_schema,
    );
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let migration = Migration {
        version: "2026040101_trusted_internal_writes",
        description: "trusted_internal_writes",
        statements,
    };
    database.apply_migrations(&[migration]).await?;
    Ok(())
}

#[tokio::test]
async fn trusted_internal_writes_have_explicit_origin_and_transaction_visible_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let audit = TrustedWriteAudit::default();
    let mut database = graphql_orm::db::Database::new(pool.clone());
    database.set_write_input_transform(audit.clone());
    database.set_mutation_hook(WorkspaceLifecycleHook);
    apply_schema(&database).await?;

    let schema = schema_builder(database.clone())
        .data("actor-1".to_string())
        .finish();

    let created = schema
        .execute(
            "mutation {
                createWorkspace(input: { name: \"Alpha\" }) {
                    success
                    workspace { id name }
                }
            }",
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created_json = created.data.into_json()?;
    let workspace_id = graphql_orm::uuid::Uuid::parse_str(
        created_json["createWorkspace"]["workspace"]["id"]
            .as_str()
            .expect("workspace id missing"),
    )?;

    let workspace = Workspace::get(&pool, &workspace_id)
        .await?
        .expect("workspace should exist");
    assert_eq!(workspace.owner_actor_id, "actor-1");

    let memberships = WorkspaceMembership::query(&pool)
        .filter(WorkspaceMembershipWhereInput {
            workspace_id: Some(UuidFilter {
                eq: Some(workspace_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].role, "owner");
    assert_eq!(
        memberships[0].updated_by_actor_id.as_deref(),
        Some("hook-create")
    );

    let updated = schema
        .execute(format!(
            "mutation {{
                updateWorkspace(id: \"{workspace_id}\", input: {{ name: \"Alpha Two\" }}) {{
                    success
                    workspace {{ id name }}
                }}
            }}"
        ))
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);

    let memberships = WorkspaceMembership::query(&pool)
        .filter(WorkspaceMembershipWhereInput {
            workspace_id: Some(UuidFilter {
                eq: Some(workspace_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].role, "owner-updated");
    assert_eq!(
        memberships[0].updated_by_actor_id.as_deref(),
        Some("hook-update")
    );

    let audit_entries = audit.snapshot();
    assert!(audit_entries.contains(&"create:Workspace:GraphqlMutation".to_string()));
    assert!(audit_entries.contains(&"create:WorkspaceMembership:InternalMutationHook".to_string()));
    assert!(audit_entries.contains(&"update:Workspace:GraphqlMutation".to_string()));
    assert!(audit_entries.contains(&"update:WorkspaceMembership:InternalMutationHook".to_string()));

    Ok(())
}
