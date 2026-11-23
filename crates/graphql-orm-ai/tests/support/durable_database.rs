//! Test-owned databases shared by the durable integration and service tests.

#[cfg(feature = "postgres")]
use graphql_orm::graphql::orm::ApplyOptions;
use graphql_orm::prelude::Database;
#[cfg(feature = "mssql")]
use graphql_orm::prelude::MssqlBackend as ParityBackend;
#[cfg(feature = "postgres")]
use graphql_orm::prelude::PostgresBackend as ParityBackend;
#[cfg(feature = "postgres")]
use std::process::{Command, Output};
#[cfg(feature = "postgres")]
use std::thread;
use std::time::Duration as StdDuration;
#[cfg(feature = "postgres")]
use std::time::Instant;
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "../../../graphql-orm/tests/support/owned_mssql.rs"]
mod owned_mssql;

#[cfg(feature = "postgres")]
const LOCAL_DOCKER_SOCKET: &str = "unix:///var/run/docker.sock";
#[cfg(feature = "postgres")]
const POSTGRES_IMAGE: &str =
    "postgres:17-alpine@sha256:d3244e8ce3f165ee3c93d9c4a4b1d0fe48b3a89e2287b717fdcbbde92d405065";

#[cfg(feature = "postgres")]
pub(crate) struct OwnedPostgres {
    container_id: String,
    owner_token: String,
    pub(crate) user: String,
    pub(crate) password: String,
    database: String,
    port: u16,
}

#[cfg(feature = "postgres")]
impl OwnedPostgres {
    pub(crate) fn start() -> Result<Option<Self>, String> {
        if !local_docker_available() {
            if std::env::var_os("CI").is_some() {
                return Err(
                    "the CI PostgreSQL parity job requires the local Docker socket".to_owned(),
                );
            }
            eprintln!("skipping PostgreSQL parity: local Docker socket is unavailable");
            return Ok(None);
        }

        let owner_token = Uuid::new_v4().simple().to_string();
        let user = format!("ai_user_{owner_token}");
        let password = format!("ai_password_{owner_token}");
        let database = format!("ai_database_{owner_token}");
        let name = format!("graphql-orm-ai-pg-{owner_token}");
        let output = local_docker()
            .args([
                "run",
                "--detach",
                "--rm",
                "--pull=missing",
                "--name",
                &name,
                "--label",
                &format!("com.dastari.graphql-orm-ai.test-owner={owner_token}"),
                "--memory",
                "4g",
                "--memory-swap",
                "4g",
                "--publish",
                "127.0.0.1::5432/tcp",
                "--tmpfs",
                "/var/lib/postgresql/data:rw,nosuid,nodev,size=1g",
                "--health-cmd",
                "pg_isready -h 127.0.0.1 -U $POSTGRES_USER -d $POSTGRES_DB",
                "--health-interval",
                "250ms",
                "--health-timeout",
                "2s",
                "--health-retries",
                "120",
                "--env",
                &format!("POSTGRES_USER={user}"),
                "--env",
                &format!("POSTGRES_PASSWORD={password}"),
                "--env",
                &format!("POSTGRES_DB={database}"),
                POSTGRES_IMAGE,
            ])
            .output()
            .map_err(|_| "failed to invoke the local Docker CLI".to_owned())?;
        if !output.status.success() {
            return Err("failed to create the owned disposable PostgreSQL container".to_owned());
        }
        let container_id = String::from_utf8(output.stdout)
            .map_err(|_| "Docker returned a non-UTF-8 container ID".to_owned())?
            .trim()
            .to_owned();
        if container_id.len() < 12 || !container_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Docker returned an invalid container ID".to_owned());
        }

        let mut owned = Self {
            container_id,
            owner_token,
            user,
            password,
            database,
            port: 0,
        };
        owned.port = owned.published_port()?;
        owned.wait_until_healthy()?;
        Ok(Some(owned))
    }

    pub(crate) fn connection_url(&self) -> String {
        format!(
            "postgres://{}:{}@127.0.0.1:{}/{}?sslmode=disable",
            self.user, self.password, self.port, self.database
        )
    }

    fn published_port(&self) -> Result<u16, String> {
        let output = self.docker_output(&["port", &self.container_id, "5432/tcp"])?;
        if !output.status.success() {
            return Err("failed to read the owned PostgreSQL port".to_owned());
        }
        let value = String::from_utf8(output.stdout)
            .map_err(|_| "Docker returned a non-UTF-8 port".to_owned())?;
        let port = value
            .trim()
            .strip_prefix("127.0.0.1:")
            .ok_or_else(|| "Docker did not bind PostgreSQL to IPv4 loopback".to_owned())?
            .parse::<u16>()
            .map_err(|_| "Docker returned an invalid PostgreSQL port".to_owned())?;
        if port == 0 {
            return Err("Docker returned an invalid PostgreSQL port".to_owned());
        }
        Ok(port)
    }

    fn wait_until_healthy(&self) -> Result<(), String> {
        let deadline = Instant::now() + StdDuration::from_secs(30);
        while Instant::now() < deadline {
            let output = self.docker_output(&[
                "inspect",
                "--format={{.State.Health.Status}}",
                &self.container_id,
            ])?;
            if output.status.success() {
                let health = String::from_utf8(output.stdout)
                    .map_err(|_| "Docker returned non-UTF-8 health state".to_owned())?;
                match health.trim() {
                    "healthy" => return Ok(()),
                    "unhealthy" => {
                        return Err("the owned PostgreSQL container became unhealthy".to_owned());
                    }
                    _ => {}
                }
            }
            thread::sleep(StdDuration::from_millis(250));
        }
        Err("the owned PostgreSQL container did not become healthy in time".to_owned())
    }

    fn docker_output(&self, args: &[&str]) -> Result<Output, String> {
        local_docker()
            .args(args)
            .output()
            .map_err(|_| "failed to invoke the local Docker CLI".to_owned())
    }

    fn still_owned(&self) -> bool {
        self.docker_output(&[
            "inspect",
            "--format={{index .Config.Labels \"com.dastari.graphql-orm-ai.test-owner\"}}",
            &self.container_id,
        ])
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|value| value.trim() == self.owner_token)
    }
}

#[cfg(feature = "postgres")]
impl Drop for OwnedPostgres {
    fn drop(&mut self) {
        if self.still_owned() {
            let _ = self.docker_output(&["rm", "--force", &self.container_id]);
        }
    }
}

#[cfg(feature = "postgres")]
fn local_docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", LOCAL_DOCKER_SOCKET]);
    command
}

#[cfg(feature = "postgres")]
fn local_docker_available() -> bool {
    local_docker()
        .args(["version", "--format={{.Server.Version}}"])
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(feature = "postgres")]
pub(crate) type OwnedServer = OwnedPostgres;
#[cfg(feature = "mssql")]
pub(crate) type OwnedServer = owned_mssql::OwnedSqlServer;

#[cfg(feature = "postgres")]
pub(crate) async fn owned_database() -> Option<(OwnedServer, Database<ParityBackend>)> {
    let container = OwnedPostgres::start().expect("owned PostgreSQL should start")?;
    let database = Database::<ParityBackend>::connect_postgres(container.connection_url())
        .await
        .expect("ORM should connect only to the owned container");
    Some((container, database))
}

#[cfg(feature = "mssql")]
pub(crate) async fn owned_database() -> Option<(OwnedServer, Database<ParityBackend>)> {
    use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions};

    let server = tokio::time::timeout(
        StdDuration::from_secs(120),
        owned_mssql::OwnedSqlServer::start(),
    )
    .await
    .expect("owned SQL Server startup timed out")
    .expect("owned SQL Server should start");
    let options = MssqlPoolOptions::default().query_timeout(Some(StdDuration::from_secs(15)));
    let pool = MssqlPool::connect_ado_external_writable_with(&server.connection_string, options)
        .await
        .expect("ORM should connect only to the owned container");
    let database = Database::<ParityBackend>::builder(pool)
        .schema_policy(graphql_orm::graphql::orm::SchemaPolicy::ExternalWritable)
        .build();
    Some((server, database))
}

pub(crate) async fn apply_parity_migration(
    _server: &OwnedServer,
    _database: &Database<ParityBackend>,
    plan: &graphql_orm::graphql::orm::PlannedMigration,
) {
    #[cfg(feature = "postgres")]
    _database
        .schema()
        .apply_migration(plan, ApplyOptions::default())
        .await
        .expect("apply managed parity schema");
    #[cfg(feature = "mssql")]
    {
        for sql in &plan.statements {
            execute_sql_server_batch(_server, sql)
                .await
                .unwrap_or_else(|error| panic!("external DDL failed: {error}: {sql}"));
        }
    }
}

#[cfg(feature = "mssql")]
pub(crate) async fn execute_sql_server_batch(
    server: &OwnedServer,
    sql: &str,
) -> graphql_orm::Result<()> {
    tokio::time::timeout(
        StdDuration::from_secs(15),
        owned_mssql::execute_batch(&server.connection_string, sql),
    )
    .await
    .expect("owned SQL Server batch must finish before its timeout")
}
