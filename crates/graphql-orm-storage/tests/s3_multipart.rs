#![cfg(feature = "s3")]
//! Resumable multipart uploads on a test-owned S3-compatible server.
//!
//! The ignored test starts a digest-pinned RustFS container with random
//! credentials on a loopback port, creates its own bucket, runs the shared
//! conformance suite and removes the container. It never reads ambient S3
//! configuration:
//!
//! ```sh
//! cargo test -p graphql-orm-storage --no-default-features --features s3 --test s3_multipart -- --ignored
//! ```

#[path = "support/multipart_conformance.rs"]
mod multipart_conformance;

use std::{process::Command, sync::Arc, time::Duration};

use graphql_orm_storage::{MultipartBlobStore, S3StorageBackend, S3StorageConfig};
use multipart_conformance::Make;
use uuid::Uuid;

/// RustFS, an Apache-2.0 S3-compatible server, pinned by digest.
const S3_IMAGE: &str =
    "rustfs/rustfs@sha256:ed9210d6411d77ffceb70d94b964a3cb7d225983368bc25bebe01e995293dcc2";
const PART: usize = 5 * 1024 * 1024;

struct OwnedS3 {
    container_id: String,
    config: S3StorageConfig,
}

impl OwnedS3 {
    fn start() -> Self {
        for variable in ["S3_TEST_ENDPOINT", "S3_TEST_BUCKET", "AWS_ENDPOINT_URL"] {
            assert!(
                std::env::var_os(variable).is_none_or(|value| value.is_empty()),
                "refusing ambient {variable}; this test owns its S3 server"
            );
        }
        let token = Uuid::new_v4().simple().to_string();
        let access_key = format!("owner{}", &token[..12]);
        let secret_key = format!("secret{token}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &format!("graphql-orm-storage-s3-{token}"),
                "--label",
                &format!("graphql-orm.test-owner={token}"),
                "--publish",
                "127.0.0.1::9000",
                "--env",
                &format!("RUSTFS_ACCESS_KEY={access_key}"),
                "--env",
                &format!("RUSTFS_SECRET_KEY={secret_key}"),
                S3_IMAGE,
            ])
            .output()
            .expect("run docker");
        assert!(
            output.status.success(),
            "failed to start the owned S3 server: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let container_id = String::from_utf8(output.stdout)
            .expect("id")
            .trim()
            .to_string();
        let port = Command::new("docker")
            .args(["port", &container_id, "9000/tcp"])
            .output()
            .expect("docker port");
        let address = String::from_utf8(port.stdout).expect("port");
        let address = address
            .lines()
            .next()
            .expect("published port")
            .trim()
            .to_string();
        Self {
            container_id,
            config: S3StorageConfig {
                endpoint_url: format!("http://{address}"),
                region: "us-east-1".to_string(),
                bucket: format!("multipart-{}", &token[..16]),
                key_prefix: Some(format!("conformance-{}", &token[..8])),
                access_key_id: access_key,
                secret_access_key: secret_key,
                path_style: true,
            },
        }
    }

    async fn create_bucket(&self) {
        let credentials = aws_credential_types::Credentials::new(
            self.config.access_key_id.clone(),
            self.config.secret_access_key.clone(),
            None,
            None,
            "owned-s3",
        );
        let client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::config::Builder::new()
                .region(aws_sdk_s3::config::Region::new(self.config.region.clone()))
                .credentials_provider(credentials)
                .endpoint_url(self.config.endpoint_url.clone())
                .force_path_style(true)
                .build(),
        );
        for _ in 0..120 {
            if client
                .create_bucket()
                .bucket(&self.config.bucket)
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("the owned S3 server did not become ready");
    }
}

impl Drop for OwnedS3 {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.container_id])
            .output();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "starts a test-owned S3-compatible container"]
async fn owned_s3_passes_the_multipart_conformance_suite() {
    let server = OwnedS3::start();
    server.create_bucket().await;
    let config = server.config.clone();
    let make: Make = Arc::new(move || {
        Arc::new(
            S3StorageBackend::new(config.clone()).with_multipart_max_part_size(PART as u64 + 1024),
        ) as Arc<dyn MultipartBlobStore>
    });
    multipart_conformance::run_all(make, "uploads", PART).await;
}
