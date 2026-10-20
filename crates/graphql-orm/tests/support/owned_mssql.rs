//! Disposable SQL Server owned by these upstream tests, never an application DSN.
use std::process::{Command, Stdio};

const SQL_SERVER_IMAGE: &str = "mcr.microsoft.com/mssql/server@sha256:ba4c8329f48fb8f02e1416be6a930ebfd71268caee78aa985f3af4315e457c89";

/// Finite container bounds. CPU and memory are hard limits; the swap limit
/// equals the memory limit, so the container receives no swap.
const CONTAINER_CPUS: &str = "2";
const CONTAINER_MEMORY: &str = "3g";
const CONTAINER_MEMORY_SWAP: &str = "3g";
/// Tasks (processes and threads) inside the container.
const CONTAINER_PIDS: &str = "1024";
/// One bounded json-file log; older output is discarded rather than retained.
const CONTAINER_LOG_MAX_SIZE: &str = "max-size=16m";
const CONTAINER_LOG_MAX_FILE: &str = "max-file=1";

/// Image pull policy. Defaults to `never`, so a missing pinned image fails
/// closed instead of downloading; `missing` is an explicit opt-in for
/// disposable hosted runners.
fn pull_policy() -> Result<&'static str, String> {
    match std::env::var("GRAPHQL_ORM_TEST_MSSQL_PULL").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("never") => Ok("never"),
        Ok("missing") => Ok("missing"),
        _ => Err("GRAPHQL_ORM_TEST_MSSQL_PULL must be `never` or `missing`".to_string()),
    }
}

/// Optional writable-layer size such as `8G`, passed as `--storage-opt size=`.
/// Only some storage drivers support it; Docker rejects the run otherwise, so
/// a requested bound is never silently dropped.
fn storage_size() -> Result<Option<String>, String> {
    let Some(value) = std::env::var_os("GRAPHQL_ORM_TEST_MSSQL_STORAGE_SIZE") else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| "GRAPHQL_ORM_TEST_MSSQL_STORAGE_SIZE must be UTF-8".to_string())?;
    if !value.is_ascii() {
        return Err("GRAPHQL_ORM_TEST_MSSQL_STORAGE_SIZE must look like 8G or 4096M".to_string());
    }
    let (digits, unit) = value.split_at(value.len().saturating_sub(1));
    let valid = matches!(unit, "M" | "G")
        && (1..=5).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    if valid {
        Ok(Some(value))
    } else {
        Err("GRAPHQL_ORM_TEST_MSSQL_STORAGE_SIZE must look like 8G or 4096M".to_string())
    }
}

pub(crate) struct OwnedSqlServer {
    name: String,
    container_id: String,
    owner_token: String,
    pub(crate) connection_string: String,
    cleaned: bool,
}

impl Drop for OwnedSqlServer {
    fn drop(&mut self) {
        if !self.cleaned && self.has_exact_owned_identity() {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", &self.container_id])
                .output();
        }
    }
}

impl OwnedSqlServer {
    pub(crate) async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        for variable in [
            "DATABASE_URL",
            "TEST_DATABASE_URL",
            "MSSQL_TEST_DATABASE_URL",
        ] {
            assert!(
                std::env::var_os(variable).is_none(),
                "refusing ambient database configuration"
            );
        }
        let pull = pull_policy()?;
        let storage = storage_size()?;
        let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-mssql-writes-{token}");
        let password = format!("Gom_{token}A9!");
        let label = format!("graphql-orm.test-owner={token}");
        let sa_password = format!("MSSQL_SA_PASSWORD={password}");
        let mut arguments: Vec<&str> = vec![
            "run",
            "--detach",
            "--rm",
            "--pull",
            pull,
            "--name",
            name.as_str(),
            "--label",
            label.as_str(),
            "--cpus",
            CONTAINER_CPUS,
            "--memory",
            CONTAINER_MEMORY,
            "--memory-swap",
            CONTAINER_MEMORY_SWAP,
            "--pids-limit",
            CONTAINER_PIDS,
            "--log-driver",
            "json-file",
            "--log-opt",
            CONTAINER_LOG_MAX_SIZE,
            "--log-opt",
            CONTAINER_LOG_MAX_FILE,
            "--publish",
            "127.0.0.1::1433",
            "--env",
            "ACCEPT_EULA=Y",
            "--env",
            sa_password.as_str(),
        ];
        let storage_option = storage.map(|size| format!("size={size}"));
        if let Some(option) = &storage_option {
            arguments.extend(["--storage-opt", option.as_str()]);
        }
        arguments.push(SQL_SERVER_IMAGE);
        let output = Command::new("docker").args(&arguments).output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start owned SQL Server: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let container_id = String::from_utf8(output.stdout)?.trim().to_owned();
        if container_id.is_empty() {
            return Err("docker did not return the owned SQL Server container ID".into());
        }
        let mut owned = Self {
            name,
            container_id,
            owner_token: token,
            connection_string: String::new(),
            cleaned: false,
        };
        let port = Command::new("docker")
            .args(["port", &owned.name, "1433/tcp"])
            .output()?;
        let published = String::from_utf8(port.stdout)?;
        let port = published
            .lines()
            .find_map(|line| line.strip_prefix("127.0.0.1:"))
            .ok_or("owned SQL Server was not loopback-published")?;
        let master_connection = format!(
            "server=tcp:127.0.0.1,{port};database=master;user id=sa;password={password};TrustServerCertificate=true"
        );
        for _ in 0..180 {
            if let Ok(pool) =
                graphql_orm::db::mssql::MssqlPool::connect_ado(&master_connection).await
                && pool.fetch_rows("SELECT 1 AS ready", &[]).await.is_ok()
            {
                let database_name = format!("graphql_orm_{}", owned.owner_token);
                execute_batch(
                    &master_connection,
                    &format!("CREATE DATABASE [{database_name}]"),
                )
                .await?;
                owned.connection_string = format!(
                    "server=tcp:127.0.0.1,{port};database={database_name};user id=sa;password={password};TrustServerCertificate=true"
                );
                return Ok(owned);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err("owned SQL Server did not become ready".into())
    }

    fn has_exact_owned_identity(&self) -> bool {
        let identity = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.Id}} {{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.container_id,
            ])
            .output();
        identity
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).trim()
                    == format!("{} {}", self.container_id, self.owner_token)
            })
    }

    /// Install synthetic certificates on this exact disposable server only.
    #[allow(dead_code)]
    pub(crate) async fn configure_tls(
        &mut self,
        certificate: &std::path::Path,
        private_key: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !self.has_exact_owned_identity() {
            return Err("refusing TLS configuration without exact owned identity".into());
        }
        // Open both inputs before changing the container. Keep file contents on
        // stdin and use fixed destinations rather than interpolating shell data.
        for (source, script) in [
            (
                std::fs::File::open(certificate)?,
                "umask 077; cat > /var/opt/mssql/audit-server.pem",
            ),
            (
                std::fs::File::open(private_key)?,
                "umask 077; cat > /var/opt/mssql/audit-server.key",
            ),
        ] {
            if !self.has_exact_owned_identity() {
                return Err("refusing TLS configuration without exact owned identity".into());
            }
            let output = Command::new("docker")
                .args(["exec", "--user", "0", "-i", &self.container_id])
                .args(["sh", "-c", script])
                .stdin(Stdio::from(source))
                .output()?;
            if !output.status.success() {
                return Err("could not install owned TLS fixture".into());
            }
        }
        let commands: &[&[&str]] = &[
            &[
                "chown",
                "10001:0",
                "/var/opt/mssql/audit-server.pem",
                "/var/opt/mssql/audit-server.key",
            ],
            &[
                "chmod",
                "600",
                "/var/opt/mssql/audit-server.pem",
                "/var/opt/mssql/audit-server.key",
            ],
            &[
                "/opt/mssql/bin/mssql-conf",
                "set",
                "network.tlscert",
                "/var/opt/mssql/audit-server.pem",
            ],
            &[
                "/opt/mssql/bin/mssql-conf",
                "set",
                "network.tlskey",
                "/var/opt/mssql/audit-server.key",
            ],
            &[
                "/opt/mssql/bin/mssql-conf",
                "set",
                "network.forceencryption",
                "1",
            ],
        ];
        for args in commands {
            let output = Command::new("docker")
                .args(["exec", "--user", "0", &self.container_id])
                .args(*args)
                .output()?;
            if !output.status.success() {
                return Err("owned SQL Server TLS configuration failed".into());
            }
        }
        if !Command::new("docker")
            .args(["restart", &self.container_id])
            .output()?
            .status
            .success()
        {
            return Err("owned SQL Server TLS restart failed".into());
        }
        // Docker may assign a different ephemeral host port after restart. No
        // pool is retained across TLS setup; resolve this same immutable server
        // again before constructing the certificate-validation connections.
        let published = Command::new("docker")
            .args(["port", &self.container_id, "1433/tcp"])
            .output()?;
        if !published.status.success() {
            return Err("cannot resolve restarted owned SQL Server port".into());
        }
        let published = String::from_utf8(published.stdout)?;
        let port = published
            .lines()
            .find_map(|line| line.strip_prefix("127.0.0.1:"))
            .ok_or("restarted owned SQL Server was not loopback-published")?;
        let port: u16 = port.parse()?;
        let (_, settings) = self
            .connection_string
            .split_once(';')
            .ok_or("invalid owned connection settings")?;
        self.connection_string = format!("server=tcp:127.0.0.1,{port};{settings}");
        let mut last_error = String::new();
        for _ in 0..180 {
            let pool =
                graphql_orm::db::mssql::MssqlPool::connect_ado(&self.connection_string).await?;
            match pool.fetch_rows("SELECT 1 AS ready", &[]).await {
                Ok(_) => return Ok(()),
                Err(error) => last_error = error.to_string(),
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(format!("owned SQL Server TLS listener did not become ready: {last_error}").into())
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.has_exact_owned_identity() {
            return Err("refusing to clean up a SQL Server without exact owned identity".into());
        }
        let removed = Command::new("docker")
            .args(["rm", "--force", "--volumes", &self.container_id])
            .output()?;
        if !removed.status.success() {
            return Err(format!(
                "failed to remove owned SQL Server: {}",
                String::from_utf8_lossy(&removed.stderr)
            )
            .into());
        }
        let absent = Command::new("docker")
            .args(["inspect", &self.container_id])
            .output()?;
        if absent.status.success() {
            return Err("owned SQL Server remains after cleanup".into());
        }
        self.cleaned = true;
        Ok(())
    }
}

pub(crate) async fn execute_batch(connection_string: &str, sql: &str) -> graphql_orm::Result<()> {
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)
        .map_err(|error| graphql_orm::sqlx::Error::Protocol(error.to_string()))?;
    let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write())
        .await
        .map_err(|error| graphql_orm::sqlx::Error::Protocol(error.to_string()))?;
    client
        .simple_query(sql)
        .await
        .map_err(|error| graphql_orm::sqlx::Error::Protocol(error.to_string()))?
        .into_results()
        .await
        .map_err(|error| graphql_orm::sqlx::Error::Protocol(error.to_string()))?;
    Ok(())
}
