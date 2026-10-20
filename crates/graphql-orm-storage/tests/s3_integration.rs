#![cfg(feature = "s3")]

use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, S3StorageBackend, S3StorageConfig, StorageByteStream, StorageError,
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

/// In-memory owned S3 fixture for namespace markers on a loopback port.
/// Supports ranged GET, conditional PUT (If-None-Match: *) and records headers.
fn namespace_fixture(
    initial: Vec<(String, Vec<u8>)>,
    max_requests: usize,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("owned loopback listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    let server = std::thread::spawn(move || {
        let mut objects: BTreeMap<String, Vec<u8>> = initial.into_iter().collect();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut log = Vec::new();
        while log.len() < max_requests && Instant::now() < deadline {
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
            let header = |name: &str| {
                headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.trim().to_string())
            };
            let length = header("content-length")
                .map(|value| value.parse::<usize>().expect("length"))
                .unwrap_or(0);
            assert!(length < 65_536, "bounded fixture body");
            let mut body = vec![0; length];
            stream.read_exact(&mut body).expect("request body");
            let line = headers.lines().next().expect("request line").to_string();
            let mut parts = line.split_whitespace();
            let method = parts.next().expect("method").to_string();
            let target = parts.next().expect("target").to_string();
            let path = target
                .split('?')
                .next()
                .expect("path")
                .trim_start_matches("/fixture/")
                .to_string();
            log.push(format!(
                "{method} {path} if-none-match={}",
                header("if-none-match").unwrap_or_default()
            ));
            let response = match method.as_str() {
                "GET" => match objects.get(&path) {
                    Some(object) => {
                        let end = object.len().min(1025);
                        let range = format!("bytes 0-{}/{}", end.saturating_sub(1), object.len());
                        let mut response = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {end}\r\nContent-Range: {range}\r\nConnection: close\r\n\r\n"
                        )
                        .into_bytes();
                        response.extend_from_slice(&object[..end]);
                        response
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Type: application/xml\r\nContent-Length: 45\r\nConnection: close\r\n\r\n<Error><Code>NoSuchKey</Code></Error>\n\n\n\n\n\n\n\n".to_vec(),
                },
                "PUT" => {
                    if header("if-none-match").as_deref() == Some("*") && objects.contains_key(&path) {
                        b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                    } else {
                        objects.insert(path, body);
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n".to_vec()
                    }
                }
                other => panic!("unexpected method {other}"),
            };
            stream.write_all(&response).expect("response");
        }
        log
    });
    (endpoint, server)
}

fn namespace_backend(endpoint: String) -> S3StorageBackend {
    S3StorageBackend::new(S3StorageConfig {
        endpoint_url: endpoint,
        region: "us-east-1".to_string(),
        bucket: "fixture".to_string(),
        key_prefix: Some("tenant".to_string()),
        access_key_id: "owned-fixture".to_string(),
        secret_access_key: "owned-fixture-secret".to_string(),
        path_style: true,
    })
}

#[tokio::test]
async fn s3_namespace_initialise_is_conditional_and_reconciles_same_identity() {
    use graphql_orm_storage::{
        NamespaceId, NamespaceInitialisation, NamespaceState, NamespaceTarget, TopologyScan,
    };

    // Requests: observe GET 404; initialise GET 404, PUT, GET; retry GET.
    let (endpoint, server) = namespace_fixture(Vec::new(), 5);
    let store = namespace_backend(endpoint);
    let id = NamespaceId::new_random();
    let observed = store
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    assert_eq!(observed.state, NamespaceState::Uninitialised);
    assert_eq!(
        store
            .initialise_namespace(&NamespaceTarget::Root, id)
            .await
            .expect("init"),
        NamespaceInitialisation::Created(id)
    );
    assert_eq!(
        store
            .initialise_namespace(&NamespaceTarget::Root, id)
            .await
            .expect("retry"),
        NamespaceInitialisation::AlreadyInitialisedSame(id)
    );
    let log = server.join().expect("fixture thread");
    assert_eq!(
        log,
        vec![
            "GET tenant/.graphql-orm-namespace if-none-match=".to_string(),
            "GET tenant/.graphql-orm-namespace if-none-match=".to_string(),
            "PUT tenant/.graphql-orm-namespace if-none-match=*".to_string(),
            "GET tenant/.graphql-orm-namespace if-none-match=".to_string(),
            "GET tenant/.graphql-orm-namespace if-none-match=".to_string(),
        ]
    );
}

#[tokio::test]
async fn s3_namespace_foreign_marker_is_conflict_and_reserved_keys_never_reach_the_provider() {
    use graphql_orm_storage::{
        NamespaceId, NamespaceInitialisation, NamespaceState, NamespaceTarget, TopologyScan,
    };

    let foreign = (
        String::from("tenant/.graphql-orm-namespace"),
        b"{\"format\":\"other\"}".to_vec(),
    );
    let (endpoint, server) = namespace_fixture(vec![foreign], 2);
    let store = namespace_backend(endpoint);
    let observed = store
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    assert!(matches!(observed.state, NamespaceState::Conflict { .. }));
    assert!(matches!(
        store
            .initialise_namespace(&NamespaceTarget::Root, NamespaceId::new_random())
            .await
            .expect("init"),
        NamespaceInitialisation::Conflict { .. }
    ));
    for key in [
        ".graphql-orm-namespace",
        "data/.graphql-orm-namespace",
        ".graphql-orm-namespace.publishing",
    ] {
        let body = StorageByteStream::from_bytes(Bytes::from_static(b"overwrite"));
        assert!(matches!(
            store.put_blob(key, body, BlobPutOptions::default()).await,
            Err(StorageError::InvalidStorageKey { .. })
        ));
        assert!(store.delete_blob(key).await.is_err());
    }
    // Only the two marker reads reached the fixture; no PUT or DELETE did.
    let log = server.join().expect("fixture thread");
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|line| line.starts_with("GET ")));
}

#[tokio::test]
async fn s3_fences_and_scan_limits_fail_before_any_request() {
    use graphql_orm_storage::{
        AncestorScan, NamespaceFence, NamespaceId, NamespaceTarget, NestedScan, TopologyScan,
    };
    use std::time::Duration;

    // Nothing listens here; every assertion below must fail locally.
    let store = namespace_backend("http://127.0.0.1:9".to_string());
    let fence = NamespaceFence {
        target: NamespaceTarget::SubPrefix("a".to_string()),
        namespace: NamespaceId::new_random(),
        epoch: store.observation_epoch(),
    };
    for outside in ["b/object", "a-sibling/object", "a", "object"] {
        let body = || StorageByteStream::from_bytes(Bytes::from_static(b"x"));
        assert!(
            matches!(
                store
                    .put_blob_fenced(outside, body(), BlobPutOptions::default(), &fence)
                    .await,
                Err(StorageError::PreconditionFailed { .. })
            ),
            "{outside}"
        );
        assert!(
            matches!(
                store
                    .put_blob_if_not_exists_fenced(
                        outside,
                        body(),
                        BlobPutOptions::default(),
                        &fence
                    )
                    .await,
                Err(StorageError::PreconditionFailed { .. })
            ),
            "{outside}"
        );
    }
    let other_instance = NamespaceFence {
        epoch: namespace_backend("http://127.0.0.1:9".to_string()).observation_epoch(),
        ..fence.clone()
    };
    assert!(matches!(
        store
            .put_blob_if_not_exists_fenced(
                "a/object",
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
                &other_instance,
            )
            .await,
        Err(StorageError::PreconditionFailed { .. })
    ));
    let overflowing = TopologyScan {
        ancestors: Some(AncestorScan {
            max_depth: usize::MAX,
            max_duration: Duration::from_secs(1),
        }),
        nested: Some(NestedScan {
            max_depth: 1,
            max_pages: usize::MAX,
            max_keys: usize::MAX,
            max_bytes: usize::MAX,
            max_results: usize::MAX,
            max_duration: Duration::MAX,
        }),
    };
    assert!(matches!(
        store
            .observe_namespace(&NamespaceTarget::Root, &overflowing)
            .await,
        Err(StorageError::PreconditionFailed { .. })
    ));
}
