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
            let request_line = headers.lines().next().expect("request line");
            let mut request_parts = request_line.split_whitespace();
            let method = request_parts.next().expect("method");
            let path = request_parts
                .next()
                .expect("target")
                .split('?')
                .next()
                .expect("path");
            if method == "DELETE" && path == "/fixture/locks/repository.lock" {
                let attempt = server_deletes.fetch_add(1, Ordering::SeqCst);
                // The server applies the deletion, but the first response is lost.
                server_replacement.store(false, Ordering::SeqCst);
                if attempt == 0 {
                    drop(stream);
                    continue;
                }
                stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").expect("delete response");
            } else if method == "PUT" && path == "/fixture/locks/repository.lock" {
                assert!(headers.to_ascii_lowercase().contains("if-none-match: *"));
                server_replacement.store(true, Ordering::SeqCst);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"replacement\"\r\nConnection: close\r\n\r\n").expect("create response");
            } else {
                panic!("unexpected request method/path: {method} {path}");
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
    let release_error = release.expect_err("lost response must remain ambiguous, without retry");
    assert!(!release_error.is_retryable());
    assert!(replacement.expect("replacement create").is_some());
    assert_eq!(deletes.load(Ordering::SeqCst), 1);
    assert!(replacement_alive.load(Ordering::SeqCst));
}

/// A `key_prefix` in the reserved internal namespace makes every operation
/// fail before any request, listings included.
#[tokio::test]
async fn a_key_prefix_in_the_reserved_namespace_is_refused_without_a_request() {
    for key_prefix in [".graphql-orm-storage", ".GRAPHQL-ORM-STORAGE/tenant"] {
        let backend = S3StorageBackend::new(S3StorageConfig {
            // Nothing listens here; a request would fail with another error.
            endpoint_url: "http://127.0.0.1:9".to_string(),
            region: "us-east-1".to_string(),
            bucket: "bucket".to_string(),
            key_prefix: Some(key_prefix.to_string()),
            access_key_id: "access".to_string(),
            secret_access_key: "secret".to_string(),
            path_style: true,
        });
        let refused = |result: Result<(), graphql_orm_storage::StorageError>| {
            assert!(
                matches!(
                    result,
                    Err(graphql_orm_storage::StorageError::InvalidStorageKey { .. })
                ),
                "{key_prefix}: {result:?}"
            );
        };
        refused(backend.list_blobs("").await.map(drop));
        refused(backend.list_blobs("a").await.map(drop));
        refused(backend.blob_exists("a").await.map(drop));
        refused(
            backend
                .put_blob(
                    "a",
                    StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                    BlobPutOptions::default(),
                )
                .await
                .map(drop),
        );
    }
}

/// One canned S3 response for a method and path.
struct CannedResponse {
    method: &'static str,
    path: &'static str,
    status: &'static str,
    code: Option<&'static str>,
    message: String,
}

/// Serves `responses` on an owned loopback listener until `done` is set,
/// answering every matching request (the SDK may retry) the same way.
fn serve_canned_s3_responses(
    responses: Vec<CannedResponse>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> (String, std::thread::JoinHandle<()>) {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("owned loopback listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("accept: {error}"),
            };
            stream.set_nonblocking(false).expect("blocking stream");
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
            let mut request_parts = headers
                .lines()
                .next()
                .expect("request line")
                .split_whitespace();
            let method = request_parts.next().expect("method");
            let path = request_parts
                .next()
                .expect("target")
                .split('?')
                .next()
                .expect("path");
            let response = responses
                .iter()
                .find(|response| response.method == method && response.path == path)
                .unwrap_or_else(|| panic!("unexpected request {method} {path}"));
            let request_id = format!("REQ-{}", response.path.trim_start_matches("/fixture/"));
            // A HEAD response carries no body, so it has no error code.
            let body = match response.code {
                Some(code) if method != "HEAD" => format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code>\
                     <Message>{}</Message><RequestId>{request_id}</RequestId></Error>",
                    response.message
                ),
                _ => String::new(),
            };
            let head = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/xml\r\nx-amz-request-id: {request_id}\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                response.status,
                body.len()
            );
            stream.write_all(head.as_bytes()).expect("response head");
            if method != "HEAD" {
                stream.write_all(body.as_bytes()).expect("response body");
            }
        }
    });
    (endpoint, server)
}

/// S3 errors are classified from the response's error code and HTTP status,
/// not from text in the SDK's debug output, and their message is a short
/// summary rather than an SDK dump.
#[tokio::test]
async fn s3_errors_are_classified_from_structured_metadata() {
    use graphql_orm_storage::StorageError;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let canned = |method, path, status, code, message: &str| CannedResponse {
        method,
        path,
        status,
        code,
        message: message.to_string(),
    };
    let done = Arc::new(AtomicBool::new(false));
    let (endpoint, server) = serve_canned_s3_responses(
        vec![
            canned(
                "GET",
                "/fixture/missing-key.bin",
                "404 Not Found",
                Some("NoSuchKey"),
                "The specified key does not exist.",
            ),
            canned(
                "GET",
                "/fixture/missing-bucket.bin",
                "404 Not Found",
                Some("NoSuchBucket"),
                "The specified bucket does not exist",
            ),
            canned(
                "HEAD",
                "/fixture/missing-key.bin",
                "404 Not Found",
                None,
                "",
            ),
            canned(
                "GET",
                "/fixture/forbidden.bin",
                "403 Forbidden",
                Some("AccessDenied"),
                "Access Denied",
            ),
            // A message that names transient conditions, but a client error.
            canned(
                "GET",
                "/fixture/misleading.bin",
                "400 Bad Request",
                Some("InvalidArgument"),
                &format!(
                    "connection timed out; status: 503; temporarily throttled {}",
                    "x".repeat(8_000)
                ),
            ),
            canned(
                "GET",
                "/fixture/throttled.bin",
                "503 Service Unavailable",
                Some("SlowDown"),
                "Please reduce your request rate.",
            ),
            canned(
                "PUT",
                "/fixture/taken.bin",
                "412 Precondition Failed",
                Some("PreconditionFailed"),
                "At least one of the pre-conditions you specified did not hold",
            ),
        ],
        Arc::clone(&done),
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

    let provider_error = |result: Result<graphql_orm_storage::BlobBody, StorageError>| {
        let error = result.map(drop).expect_err("a provider error");
        assert!(
            matches!(error, StorageError::Provider { .. }),
            "expected a provider error, got {error:?}"
        );
        let message = error.to_string();
        for dump in [
            "Response {",
            "SdkBody",
            "HeaderMap",
            "headers",
            "raw:",
            "source:",
        ] {
            assert!(!message.contains(dump), "SDK dump in {message:?}");
        }
        assert!(message.len() < 1_024, "unbounded message: {message:?}");
        error
    };

    assert!(matches!(
        backend.get_blob("missing-key.bin").await.map(drop),
        Err(StorageError::MissingBlob { .. })
    ));
    assert!(
        backend
            .head_blob("missing-key.bin")
            .await
            .expect("head missing")
            .is_none()
    );
    // A missing bucket is a provider failure, not a missing key.
    let missing_bucket = provider_error(backend.get_blob("missing-bucket.bin").await);
    assert!(!missing_bucket.is_retryable(), "{missing_bucket}");
    assert!(missing_bucket.to_string().contains("NoSuchBucket"));
    // A response header such as `Connection: close` never makes an error
    // retryable.
    let forbidden = provider_error(backend.get_blob("forbidden.bin").await);
    assert!(!forbidden.is_retryable(), "{forbidden}");
    let message = forbidden.to_string();
    assert!(message.contains("AccessDenied"), "{message}");
    assert!(message.contains("403"), "{message}");
    assert!(message.contains("REQ-forbidden.bin"), "{message}");
    // Nor does the server's message text.
    let misleading = provider_error(backend.get_blob("misleading.bin").await);
    assert!(!misleading.is_retryable(), "{misleading}");
    assert!(misleading.to_string().contains("InvalidArgument"));
    let throttled = provider_error(backend.get_blob("throttled.bin").await);
    assert!(throttled.is_retryable(), "{throttled}");
    assert!(throttled.to_string().contains("SlowDown"));
    // A failed condition is a collision, not an error.
    assert!(
        backend
            .put_blob_if_not_exists(
                "taken.bin",
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .expect("conditional put")
            .is_none()
    );

    done.store(true, Ordering::SeqCst);
    server.join().expect("fixture thread");
}
