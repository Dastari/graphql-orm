#![cfg(feature = "s3")]

use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, S3StorageBackend, S3StorageConfig, StorageByteStream,
    collect_storage_stream, sha256_hex,
};
use uuid::Uuid;

#[tokio::test]
async fn s3_blob_store_round_trip_when_env_is_configured() {
    let Some(backend) = backend_from_env() else {
        return;
    };

    let key = "objects/test.txt";
    let copy_key = "objects/test-copy.txt";

    let outcome = backend
        .put_blob(
            key,
            StorageByteStream::from_bytes(Bytes::from_static(b"hello s3 storage")),
            BlobPutOptions {
                content_type: Some("text/plain".to_string()),
            },
        )
        .await
        .expect("put object");
    assert_eq!(outcome.size_bytes, 16);
    assert_eq!(outcome.sha256_hex, sha256_hex(b"hello s3 storage"));

    assert!(backend.blob_exists(key).await.expect("exists"));

    let metadata = backend
        .head_blob(key)
        .await
        .expect("head")
        .expect("metadata");
    assert_eq!(metadata.key, key);
    assert_eq!(metadata.size_bytes, Some(16));

    let body = backend.get_blob(key).await.expect("get object");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"hello s3 storage"));

    let ranged = backend
        .get_blob_range(key, 6..8)
        .await
        .expect("range object");
    let ranged_bytes = collect_storage_stream(ranged.body)
        .await
        .expect("collect range");
    assert_eq!(ranged_bytes, Bytes::from_static(b"s3"));

    let conditional = backend
        .put_blob_if_not_exists(
            key,
            StorageByteStream::from_bytes(Bytes::from_static(b"replacement")),
            BlobPutOptions::default(),
        )
        .await
        .expect("conditional write");
    assert_eq!(conditional, None);

    backend.copy_blob(key, copy_key).await.expect("copy object");
    let page = backend
        .list_blobs_page("objects", None, 1)
        .await
        .expect("list page");
    assert_eq!(page.keys.len(), 1);
    assert!(page.next_continuation.is_some());

    backend.delete_blob(key).await.expect("delete object");
    backend
        .delete_blob(copy_key)
        .await
        .expect("delete copied object");
}

fn backend_from_env() -> Option<S3StorageBackend> {
    let endpoint_url = std::env::var("S3_TEST_ENDPOINT").ok()?;
    let bucket = std::env::var("S3_TEST_BUCKET").ok()?;
    let region = std::env::var("S3_TEST_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let access_key_id =
        std::env::var("S3_TEST_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".to_string());
    let secret_access_key =
        std::env::var("S3_TEST_SECRET_KEY").unwrap_or_else(|_| "minioadmin".to_string());
    let path_style = std::env::var("S3_TEST_PATH_STYLE")
        .map(|value| value != "false")
        .unwrap_or(true);

    Some(S3StorageBackend::new(S3StorageConfig {
        endpoint_url,
        region,
        bucket,
        key_prefix: Some(format!("graphql-orm-storage-tests/{}", Uuid::new_v4())),
        access_key_id,
        secret_access_key,
        path_style,
    }))
}

#[tokio::test]
async fn single_attempt_delete_does_not_replay_after_lost_response_and_replacement() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("owned loopback listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    let deletes = Arc::new(AtomicUsize::new(0));
    let replacement_alive = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let server_deletes = Arc::clone(&deletes);
    let server_replacement = Arc::clone(&replacement_alive);
    let server_done = Arc::clone(&done);
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !server_done.load(Ordering::SeqCst) && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("accept: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).expect("request headers");
                request.push(byte[0]);
                assert!(request.len() < 32_768, "bounded headers");
            }
            let headers = String::from_utf8(request).expect("HTTP headers");
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().expect("content length"))
                .unwrap_or(0);
            assert!(content_length < 65_536, "bounded fixture body");
            let mut body = vec![0; content_length];
            stream.read_exact(&mut body).expect("request body");
            if headers.starts_with("DELETE /fixture/locks/repository.lock ") {
                let attempt = server_deletes.fetch_add(1, Ordering::SeqCst);
                // The server applies the deletion, but the first response is lost.
                server_replacement.store(false, Ordering::SeqCst);
                if attempt == 0 {
                    drop(stream);
                    continue;
                }
                stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").expect("delete response");
            } else if headers.starts_with("PUT /fixture/locks/repository.lock ") {
                assert!(headers.to_ascii_lowercase().contains("if-none-match: *"));
                server_replacement.store(true, Ordering::SeqCst);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"replacement\"\r\nConnection: close\r\n\r\n").expect("create response");
            } else {
                panic!("unexpected request method/path");
            }
        }
    });
    let backend = S3StorageBackend::new(S3StorageConfig {
        endpoint_url: endpoint,
        region: "us-east-1".to_string(),
        bucket: "fixture".to_string(),
        key_prefix: None,
        access_key_id: "owned-fixture".to_string(),
        secret_access_key: "owned-fixture-secret".to_string(),
        path_style: true,
    });
    let (release, replacement) = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::join!(backend.delete_blob_once("locks/repository.lock"), async {
            while deletes.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            backend
                .put_blob_if_not_exists(
                    "locks/repository.lock",
                    StorageByteStream::from_bytes(Bytes::from_static(b"new-owner")),
                    BlobPutOptions::default(),
                )
                .await
        })
    })
    .await
    .expect("bounded fixture execution");
    done.store(true, Ordering::SeqCst);
    server.join().expect("fixture thread");
    release.expect_err("lost response must remain ambiguous, without retry");
    assert!(replacement.expect("replacement create").is_some());
    assert_eq!(deletes.load(Ordering::SeqCst), 1);
    assert!(replacement_alive.load(Ordering::SeqCst));
}
