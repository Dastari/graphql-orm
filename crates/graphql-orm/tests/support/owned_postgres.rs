use std::process::Command;

const POSTGRES_IMAGE: &str = "postgres:17-alpine";

pub(crate) struct OwnedPostgres {
    container_id: String,
    owner_token: String,
    pub(crate) url: String,
    cleaned: bool,
}

impl OwnedPostgres {
    pub(crate) fn start(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let owner_token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-{label}-{owner_token}");
        let password = format!("pg_{owner_token}");
        let database = format!("graphql_orm_{owner_token}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--label",
                &format!("graphql-orm.test-owner={owner_token}"),
                "--publish",
                "127.0.0.1::5432",
                "--env",
                "POSTGRES_USER=graphql_orm_owner",
                "--env",
                &format!("POSTGRES_PASSWORD={password}"),
                "--env",
                &format!("POSTGRES_DB={database}"),
                POSTGRES_IMAGE,
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start owned PostgreSQL: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let container_id = String::from_utf8(output.stdout)?.trim().to_owned();
        if container_id.is_empty() {
            return Err("docker did not return the owned PostgreSQL container ID".into());
        }
        let mut owned = Self {
            container_id,
            owner_token,
            url: String::new(),
            cleaned: false,
        };
        for _ in 0..120 {
            let ready = Command::new("docker")
                .args([
                    "exec",
                    &owned.container_id,
                    "pg_isready",
                    "-U",
                    "graphql_orm_owner",
                    "-d",
                    &database,
                ])
                .output()?;
            if ready.status.success() {
                let port_output = Command::new("docker")
                    .args(["port", &owned.container_id, "5432/tcp"])
                    .output()?;
                let ports = String::from_utf8(port_output.stdout)?;
                let port = ports
                    .lines()
                    .find_map(|line| line.strip_prefix("127.0.0.1:"))
                    .ok_or("owned PostgreSQL was not loopback-published")?;
                owned.url =
                    format!("postgres://graphql_orm_owner:{password}@127.0.0.1:{port}/{database}");
                std::thread::sleep(std::time::Duration::from_millis(500));
                return Ok(owned);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err("owned PostgreSQL did not become ready".into())
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.has_exact_owned_identity() {
            return Err("refusing to clean up PostgreSQL without exact owned identity".into());
        }
        let removed = Command::new("docker")
            .args(["rm", "--force", "--volumes", &self.container_id])
            .output()?;
        if !removed.status.success() {
            return Err(format!(
                "failed to remove owned PostgreSQL: {}",
                String::from_utf8_lossy(&removed.stderr)
            )
            .into());
        }
        let absent = Command::new("docker")
            .args(["inspect", &self.container_id])
            .output()?;
        if absent.status.success() {
            return Err("owned PostgreSQL remains after cleanup".into());
        }
        self.cleaned = true;
        Ok(())
    }

    fn has_exact_owned_identity(&self) -> bool {
        Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.Id}} {{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.container_id,
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).trim()
                    == format!("{} {}", self.container_id, self.owner_token)
            })
    }
}

impl Drop for OwnedPostgres {
    fn drop(&mut self) {
        if !self.cleaned && self.has_exact_owned_identity() {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", &self.container_id])
                .output();
        }
    }
}
