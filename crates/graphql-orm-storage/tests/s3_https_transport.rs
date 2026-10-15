#![cfg(feature = "s3")]

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

use graphql_orm_storage::{BlobStore, S3StorageBackend, S3StorageConfig, collect_storage_stream};
use tempfile::TempDir;

struct OwnedServer(Child);

impl Drop for OwnedServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
            .expect("OpenSSL is required for the opt-in owned fixture")
            .success()
    );
}

#[test]
#[ignore = "requires OpenSSL and Python; uses only owned loopback infrastructure"]
fn owned_https_transport_fixture() {
    let temp = TempDir::new().expect("owned certificates");
    let root = temp.path();
    for (cert, key) in [("ca.pem", "ca.key"), ("untrusted.pem", "untrusted.key")] {
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
                "/CN=owned-fixture-root",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-out",
                cert,
                "-keyout",
                key,
            ],
        );
    }
    openssl(
        root,
        &[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ],
    );
    std::fs::write(root.join("server.ext"),
        "subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n").expect("certificate extensions");
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
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/s3/owned_https_fixture.py");
    let child = Command::new("python3")
        .arg("-u")
        .arg(fixture)
        .arg(root.join("server.pem"))
        .arg(root.join("server.key"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("owned Python HTTPS fixture");
    let mut server = OwnedServer(child);
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().expect("fixture stdout"))
        .read_line(&mut line)
        .expect("fixture readiness");
    let port: u16 = line.trim().parse().expect("owned loopback port");
    let empty_roots = root.join("empty-roots");
    std::fs::create_dir(&empty_roots).expect("private root directory");
    for (mode, certificate) in [("trusted", "ca.pem"), ("untrusted", "untrusted.pem")] {
        // Trust configuration is scoped to a fresh child process. Do not mutate
        // process-global certificate environment in parallel Rust tests.
        assert!(
            Command::new(std::env::current_exe().expect("test executable"))
                .args(["--ignored", "--exact", "owned_https_client", "--nocapture"])
                .env(
                    "STORAGE_OWNED_HTTPS_ENDPOINT",
                    format!("https://127.0.0.1:{port}")
                )
                .env("STORAGE_OWNED_HTTPS_TRUST", mode)
                .env("SSL_CERT_FILE", root.join(certificate))
                .env("SSL_CERT_DIR", &empty_roots)
                .status()
                .expect("isolated HTTPS client")
                .success()
        );
    }
}

#[tokio::test]
#[ignore = "child entry point; invoke owned_https_transport_fixture instead"]
async fn owned_https_client() {
    let endpoint = std::env::var("STORAGE_OWNED_HTTPS_ENDPOINT").expect("owned fixture only");
    let address: std::net::SocketAddr = endpoint
        .strip_prefix("https://")
        .expect("HTTPS endpoint")
        .parse()
        .expect("socket address");
    assert_eq!(
        address.ip(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    );
    let backend = S3StorageBackend::new(S3StorageConfig {
        endpoint_url: endpoint,
        region: "us-east-1".to_string(),
        bucket: "fixture".to_string(),
        key_prefix: None,
        access_key_id: "owned-fixture".to_string(),
        secret_access_key: "owned-fixture-secret".to_string(),
        path_style: true,
    });
    let result = tokio::time::timeout(Duration::from_secs(12), async {
        let blob = backend.get_blob("probe").await?;
        collect_storage_stream(blob.body).await
    })
    .await
    .expect("bounded HTTPS fixture request");
    match std::env::var("STORAGE_OWNED_HTTPS_TRUST")
        .expect("trust mode")
        .as_str()
    {
        "trusted" => assert_eq!(
            result.expect("trusted fixture CA").as_ref(),
            b"owned HTTPS fixture"
        ),
        "untrusted" => {
            let error = result.expect_err("untrusted certificate must be rejected");
            let diagnostic = format!("{error:?}").to_ascii_lowercase();
            assert!(
                diagnostic.contains("certificate") || diagnostic.contains("unknownissuer"),
                "expected certificate validation failure"
            );
        }
        _ => panic!("unknown fixture trust mode"),
    }
}
