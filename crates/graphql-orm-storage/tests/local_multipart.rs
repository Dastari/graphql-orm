#![cfg(feature = "local")]
//! Resumable multipart uploads on local storage.

#[path = "support/multipart_conformance.rs"]
mod multipart_conformance;

use std::sync::Arc;

use graphql_orm_storage::{
    BlobStore, LocalStorageBackend, MultipartBlobStore, MultipartStartOptions, StorageService,
};
use multipart_conformance::{Make, part};
use tempfile::TempDir;

const PART: usize = 256 * 1024;

fn backend(root: &std::path::Path) -> LocalStorageBackend {
    LocalStorageBackend::new(root).with_multipart_max_part_size(2 * PART as u64)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_storage_passes_the_multipart_conformance_suite() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path().to_path_buf();
    let make: Make = Arc::new(move || Arc::new(backend(&root)) as Arc<dyn MultipartBlobStore>);
    multipart_conformance::run_all(make, "uploads", PART).await;
}

#[tokio::test]
async fn staged_parts_survive_dropping_the_backend_and_stay_out_of_listings() {
    let temp = TempDir::new().expect("temp dir");
    let handle = {
        let first = backend(temp.path());
        let handle = first
            .start_multipart("videos/a.webm", MultipartStartOptions::default())
            .await
            .expect("start");
        first
            .upload_part(&handle, 1, part(1, PART).1)
            .await
            .expect("part one");
        handle
    };
    // A new instance, as after a restart, sees the staged part.
    let second = backend(temp.path());
    let listed = second.list_parts(&handle).await.expect("list");
    assert_eq!(listed.len(), 1);
    let last = second
        .upload_part(&handle, 2, part(2, 10).1)
        .await
        .expect("part two");
    // Staged parts are never visible as blobs.
    let page = second.list_blobs_page("", None, 100).await.expect("blobs");
    assert!(page.keys.is_empty(), "{page:?}");
    second
        .complete_multipart(&handle, &[listed[0].clone(), last])
        .await
        .expect("complete");
    let page = second.list_blobs_page("", None, 100).await.expect("blobs");
    assert_eq!(page.keys, ["videos/a.webm"]);
    // Completion removes the staging area for the upload.
    let staging = temp.path().join(".graphql-orm-storage/multipart");
    let left = std::fs::read_dir(&staging)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0);
}

#[tokio::test]
async fn forged_upload_ids_name_no_upload() {
    let temp = TempDir::new().expect("temp dir");
    let store = backend(temp.path());
    for upload_id in [
        "../../escape",
        "not-a-uuid",
        "",
        "00000000-0000-0000-0000-000000000000",
    ] {
        let handle = graphql_orm_storage::MultipartUploadHandle {
            backend: graphql_orm_storage::StorageBackend::Local,
            key: "a.bin".into(),
            upload_id: upload_id.into(),
            if_not_exists: false,
        };
        assert!(store.list_parts(&handle).await.is_err());
        store
            .abort_multipart(&handle)
            .await
            .expect("aborting an unknown upload succeeds");
    }
    let wrong_backend = graphql_orm_storage::MultipartUploadHandle {
        backend: graphql_orm_storage::StorageBackend::S3,
        key: "a.bin".into(),
        upload_id: uuid_like(),
        if_not_exists: false,
    };
    assert!(matches!(
        store.list_parts(&wrong_backend).await,
        Err(graphql_orm_storage::StorageError::Multipart {
            kind: graphql_orm_storage::MultipartErrorKind::BackendMismatch,
            ..
        })
    ));
}

fn uuid_like() -> String {
    "6f1c1f3e-55c1-4b44-9a0c-3c6c3b1a9f10".to_string()
}

#[tokio::test]
async fn storage_service_exposes_multipart_only_where_supported() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(backend(temp.path())));
    let multipart = service.multipart().expect("local multipart");
    let handle = multipart
        .start_multipart("doc.bin", MultipartStartOptions::default())
        .await
        .expect("start");
    multipart.abort_multipart(&handle).await.expect("abort");
}

#[tokio::test]
async fn a_retry_after_a_crash_past_publishing_recognises_its_own_object() {
    let temp = TempDir::new().expect("temp dir");
    let store = backend(temp.path());
    let start = |store: &LocalStorageBackend, key: &'static str| {
        let store = store.clone();
        async move {
            store
                .start_multipart(
                    key,
                    MultipartStartOptions {
                        if_not_exists: true,
                        ..Default::default()
                    },
                )
                .await
                .expect("start")
        }
    };
    let handle = start(&store, "scans/a.tar").await;
    let (first, body) = part(1, PART);
    let one = store.upload_part(&handle, 1, body).await.expect("part one");
    let (last, body) = part(2, 77);
    let two = store.upload_part(&handle, 2, body).await.expect("part two");
    // The crash happened after the object was published and before the staged
    // parts were removed.
    let mut published = first.clone();
    published.extend(&last);
    std::fs::create_dir_all(temp.path().join("scans")).expect("dir");
    std::fs::write(temp.path().join("scans/a.tar"), &published).expect("publish");
    let outcome = store
        .complete_multipart(&handle, &[one, two])
        .await
        .expect("the retry recognises its own object");
    assert_eq!(outcome.size_bytes, published.len() as u64);
    assert_eq!(
        outcome.sha256_hex.as_deref(),
        Some(graphql_orm_storage::sha256_hex(&published).as_str())
    );
    assert!(
        store.list_parts(&handle).await.is_err(),
        "the staged parts are removed"
    );

    // A different object at the key is still a conflict.
    let other = start(&store, "scans/b.tar").await;
    let (_, body) = part(3, 10);
    let receipt = store.upload_part(&other, 1, body).await.expect("part");
    std::fs::write(temp.path().join("scans/b.tar"), b"someone else").expect("publish");
    assert!(matches!(
        store.complete_multipart(&other, &[receipt]).await,
        Err(graphql_orm_storage::StorageError::PreconditionFailed { .. })
    ));
    assert_eq!(
        store.list_parts(&other).await.expect("parts remain").len(),
        1
    );
}
