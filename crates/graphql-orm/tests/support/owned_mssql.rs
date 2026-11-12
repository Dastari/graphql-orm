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

/// Optional external run token (`GRAPHQL_ORM_TEST_RUN_TOKEN`). When set, it is
/// added as the `graphql-orm.test-run` label so an external controller can
/// identify exactly the containers this run created.
fn run_token() -> Result<Option<String>, String> {
    let Some(value) = std::env::var_os("GRAPHQL_ORM_TEST_RUN_TOKEN") else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| "GRAPHQL_ORM_TEST_RUN_TOKEN must be UTF-8".to_string())?;
    let valid = (8..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(Some(value))
    } else {
        Err("GRAPHQL_ORM_TEST_RUN_TOKEN must be 8-64 of [a-z0-9-]".to_string())
    }
}

/// Records the exact container ID in `GRAPHQL_ORM_TEST_MSSQL_EVIDENCE_DIR`, when
/// set, with exclusive creation, so an existing record is never overwritten.
fn record_owned_container(
    container_id: &str,
    name: &str,
    run_token: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;

    let Some(directory) = std::env::var_os("GRAPHQL_ORM_TEST_MSSQL_EVIDENCE_DIR") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(std::path::Path::new(&directory).join(format!("{container_id}.owned")))?;
    writeln!(file, "{container_id}\n{name}\n{}", run_token.unwrap_or(""))?;
    file.sync_all()?;
    Ok(())
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

/// A private root and a server certificate for `127.0.0.1`, generated per
/// server and removed with it. Connections trust the root through
/// `TrustServerCertificateCA`, so the owned tests run with full-session TLS
/// and certificate chain and hostname validation, the ORM's default policy.
pub(crate) struct OwnedTlsMaterial {
    directory: std::path::PathBuf,
}

impl Drop for OwnedTlsMaterial {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl OwnedTlsMaterial {
    fn generate() -> Result<Self, Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!(
            "graphql-orm-mssql-tls-{}",
            graphql_orm::uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&directory)?;
        let owned = Self { directory };
        owned.openssl(&[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=graphql-orm-owned-sqlserver-root",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-out",
            "ca.pem",
            "-keyout",
            "ca.key",
        ])?;
        owned.openssl(&[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=127.0.0.1",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ])?;
        std::fs::write(
            owned.directory.join("server.ext"),
            "subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n\
             keyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n",
        )?;
        owned.openssl(&[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-days",
            "2",
            "-extfile",
            "server.ext",
            "-out",
            "server.pem",
        ])?;
        // Copied into the container as root; SQL Server runs as another user.
        // The key is synthetic and lives only as long as this disposable server.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                owned.directory.join("server.key"),
                std::fs::Permissions::from_mode(0o644),
            )?;
        }
        std::fs::write(
            owned.directory.join("mssql.conf"),
            "[network]\ntlscert = /var/opt/mssql/owned-server.pem\n\
             tlskey = /var/opt/mssql/owned-server.key\ntlsprotocols = 1.2\n\
             forceencryption = 0\n",
        )?;
        Ok(owned)
    }

    fn openssl(&self, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let status = Command::new("openssl")
            .current_dir(&self.directory)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("OpenSSL is required for owned SQL Server TLS: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("OpenSSL could not generate owned SQL Server TLS material".into())
        }
    }

    pub(crate) fn certificate_authority(&self) -> std::path::PathBuf {
        self.directory.join("ca.pem")
    }
}

pub(crate) struct OwnedSqlServer {
    name: String,
    container_id: String,
    owner_token: String,
    pub(crate) connection_string: String,
    tls: OwnedTlsMaterial,
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
        let run = run_token()?;
        let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-mssql-writes-{token}");
        let password = format!("Gom_{token}A9!");
        let label = format!("graphql-orm.test-owner={token}");
        let sa_password = format!("MSSQL_SA_PASSWORD={password}");
        let tls = OwnedTlsMaterial::generate()?;
        let mut arguments: Vec<&str> = vec![
            "create",
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
        let run_label = run
            .as_ref()
            .map(|token| format!("graphql-orm.test-run={token}"));
        if let Some(label) = &run_label {
            arguments.extend(["--label", label.as_str()]);
        }
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
            tls,
            cleaned: false,
        };
        // On failure, `owned` drops here and removes the exact container.
        record_owned_container(&owned.container_id, &owned.name, run.as_deref())?;
        for (source, destination) in [
            ("server.pem", "/var/opt/mssql/owned-server.pem"),
            ("server.key", "/var/opt/mssql/owned-server.key"),
            ("mssql.conf", "/var/opt/mssql/mssql.conf"),
        ] {
            let copied = Command::new("docker")
                .arg("cp")
                .arg(owned.tls.directory.join(source))
                .arg(format!("{}:{destination}", owned.container_id))
                .output()?;
            if !copied.status.success() {
                return Err("could not install owned SQL Server TLS material".into());
            }
        }
        let started = Command::new("docker")
            .args(["start", &owned.container_id])
            .output()?;
        if !started.status.success() {
            return Err(format!(
                "failed to start owned SQL Server: {}",
                String::from_utf8_lossy(&started.stderr)
            )
            .into());
        }
        let tls_settings = format!(
            "Encrypt=true;TrustServerCertificateCA={}",
            owned.tls.certificate_authority().display()
        );
        let port = Command::new("docker")
            .args(["port", &owned.name, "1433/tcp"])
            .output()?;
        let published = String::from_utf8(port.stdout)?;
        let port = published
            .lines()
            .find_map(|line| line.strip_prefix("127.0.0.1:"))
            .ok_or("owned SQL Server was not loopback-published")?;
        let master_connection = format!(
            "server=tcp:127.0.0.1,{port};database=master;user id=sa;password={password};{tls_settings}"
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
                    "server=tcp:127.0.0.1,{port};database={database_name};user id=sa;password={password};{tls_settings}"
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

    /// The private root that signs this server's certificate.
    #[allow(dead_code)]
    pub(crate) fn certificate_authority(&self) -> std::path::PathBuf {
        self.tls.certificate_authority()
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
