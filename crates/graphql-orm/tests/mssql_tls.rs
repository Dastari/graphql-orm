#![cfg(feature = "mssql")]

#[path = "support/owned_mssql.rs"]
mod owned_mssql;

use graphql_orm::db::mssql::MssqlPool;

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
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    let trusted = server.connection_string.clone();
    let root = format!(
        ";TrustServerCertificateCA={}",
        server.certificate_authority().display()
    );
    assert!(
        trusted.contains(&root),
        "owned servers trust their private CA"
    );

    // Without the private root the platform store cannot validate the chain.
    let untrusted = trusted.replace(&root, "");
    let error = encrypted_query(&untrusted)
        .await
        .expect_err("untrusted private CA must deny");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate"),
        "expected certificate rejection"
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
