#![cfg(feature = "mssql")]

#[path = "support/owned_mssql.rs"]
mod owned_mssql;

use graphql_orm::db::mssql::MssqlPool;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Certificates(PathBuf);
impl Drop for Certificates {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn openssl(root: &Path, args: &[&str]) {
    assert!(
        Command::new("openssl")
            .current_dir(root)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("OpenSSL is required for the owned TLS fixture")
            .success()
    );
}

fn certificates() -> Certificates {
    let root = std::env::temp_dir().join(format!(
        "graphql-orm-tls-{}",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).expect("owned certificate directory");
    let owned = Certificates(root);
    let root = &owned.0;
    openssl(
        root,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=owned-sqlserver-root",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-out",
            "ca.pem",
            "-keyout",
            "ca.key",
        ],
    );
    openssl(
        root,
        &[
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
        ],
    );
    std::fs::write(root.join("server.ext"), "subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n").unwrap();
    openssl(
        root,
        &[
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
            "1",
            "-extfile",
            "server.ext",
            "-out",
            "server.pem",
        ],
    );
    owned
}

async fn encrypted_query(connection: &str) -> graphql_orm::Result<String> {
    let pool = MssqlPool::connect_ado(connection).await?;
    let rows = pool
        .fetch_rows(
            "SELECT encrypt_option FROM sys.dm_exec_connections WHERE session_id = @@SPID",
            &[],
        )
        .await?;
    rows.first()
        .expect("owned session")
        .try_get("encrypt_option")
}

#[tokio::test]
#[ignore = "requires cached SQL Server Docker image and OpenSSL; owns its loopback server"]
async fn tls_validates_private_ca_and_hostname() -> Result<(), Box<dyn std::error::Error>> {
    let certs = certificates();
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    server
        .configure_tls(&certs.0.join("server.pem"), &certs.0.join("server.key"))
        .await?;
    let checked = server
        .connection_string
        .replace("TrustServerCertificate=true", "Encrypt=true");
    let error = encrypted_query(&checked)
        .await
        .expect_err("untrusted private CA must deny");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate"),
        "expected certificate rejection"
    );
    let trusted = format!(
        "{checked};TrustServerCertificateCA={}",
        certs.0.join("ca.pem").display()
    );
    assert_eq!(encrypted_query(&trusted).await?, "TRUE");
    // The private certificate contains only the IP SAN. Sharing its CA does not
    // authorize a different host identity, even on the same owned loopback port.
    let wrong_host = trusted.replace("server=tcp:127.0.0.1,", "server=tcp:localhost,");
    let error = encrypted_query(&wrong_host)
        .await
        .expect_err("wrong hostname must deny");
    let diagnostic = error.to_string().to_ascii_lowercase();
    assert!(
        diagnostic.contains("certificate") || diagnostic.contains("hostname"),
        "expected hostname validation rejection"
    );
    server.cleanup()?;
    Ok(())
}
