//! Provider-neutral conformance scenarios for resumable multipart uploads.
//!
//! Every scenario runs against fresh backend instances from `make`, so a
//! persisted handle is always continued by a different instance than the one
//! that issued it. Keys live under `prefix`, which the caller makes unique.

use std::sync::Arc;

use bytes::Bytes;
use futures_util::stream;
use graphql_orm_storage::{
    BlobPutOptions, MultipartBlobStore, MultipartErrorKind, MultipartStartOptions,
    MultipartUploadHandle, PartReceipt, StorageByteStream, StorageError, collect_storage_stream,
    sha256_hex,
};

pub type Make = Arc<dyn Fn() -> Arc<dyn MultipartBlobStore> + Send + Sync>;

/// Part content: `size` bytes derived from `seed`, delivered in 64 KiB chunks.
pub fn part(seed: u8, size: usize) -> (Vec<u8>, StorageByteStream) {
    let bytes = (0..size)
        .map(|index| seed.wrapping_add((index % 251) as u8))
        .collect::<Vec<_>>();
    let chunks = bytes
        .chunks(64 * 1024)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<Result<Bytes, StorageError>>>();
    (
        bytes,
        StorageByteStream::new(Box::pin(stream::iter(chunks))),
    )
}

fn kind(error: StorageError) -> MultipartErrorKind {
    match error {
        StorageError::Multipart { kind, .. } => kind,
        other => panic!("expected a multipart refusal, got {other:?}"),
    }
}

async fn upload(
    store: &dyn MultipartBlobStore,
    handle: &MultipartUploadHandle,
    number: u32,
    seed: u8,
    size: usize,
) -> (Vec<u8>, PartReceipt) {
    let (bytes, body) = part(seed, size);
    let receipt = store
        .upload_part(handle, number, body)
        .await
        .expect("part upload");
    assert_eq!(receipt.part_number, number);
    assert_eq!(receipt.size_bytes, size as u64);
    assert_eq!(receipt.sha256, sha256_hex(&bytes));
    (bytes, receipt)
}

async fn read(store: &dyn MultipartBlobStore, key: &str) -> Vec<u8> {
    collect_storage_stream(store.get_blob(key).await.expect("object").body)
        .await
        .expect("object bytes")
        .to_vec()
}

/// Run every scenario. `part_size` must meet the backend's minimum part size.
pub async fn run_all(make: Make, prefix: &str, part_size: usize) {
    three_parts_complete_and_read_back(&make, prefix, part_size).await;
    resume_from_a_persisted_handle(&make, prefix, part_size).await;
    retry_replaces_a_part(&make, prefix, part_size).await;
    mismatched_receipts_leave_the_parts(&make, prefix, part_size).await;
    abort_is_idempotent(&make, prefix, part_size).await;
    stale_uploads_are_listed_and_cleaned(&make, prefix).await;
    if_not_exists_conflicts_at_completion(&make, prefix, part_size).await;
    part_boundaries(&make, prefix, part_size).await;
    concurrent_uploads_of_one_part_leave_one(&make, prefix, part_size).await;
    completion_racing_abort_has_one_outcome(&make, prefix, part_size).await;
}

async fn three_parts_complete_and_read_back(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let key = format!("{prefix}/three.bin");
    let handle = store
        .start_multipart(
            &key,
            MultipartStartOptions {
                put: BlobPutOptions {
                    content_type: Some("application/octet-stream".into()),
                },
                ..Default::default()
            },
        )
        .await
        .expect("start");
    assert_eq!(handle.key, key);
    let mut expected = Vec::new();
    let mut receipts = Vec::new();
    for (number, size) in [(1, part_size), (2, part_size), (3, 1234)] {
        let (bytes, receipt) = upload(store.as_ref(), &handle, number, number as u8, size).await;
        expected.extend(bytes);
        receipts.push(receipt);
    }
    let listed = store.list_parts(&handle).await.expect("list parts");
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed
            .iter()
            .map(|part| part.part_number)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    // The object is not visible before completion.
    assert!(!store.blob_exists(&key).await.expect("exists"));
    let outcome = store
        .complete_multipart(&handle, &receipts)
        .await
        .expect("complete");
    assert_eq!(outcome.size_bytes, expected.len() as u64);
    // Local storage hashes while assembling; S3 reports no whole-object digest.
    if let Some(digest) = &outcome.sha256_hex {
        assert_eq!(digest, &sha256_hex(&expected));
    }
    let computed = store
        .compute_blob_sha256(&key)
        .await
        .expect("hash the object");
    assert_eq!(computed.size_bytes, expected.len() as u64);
    assert_eq!(computed.sha256_hex, sha256_hex(&expected));
    assert_eq!(read(store.as_ref(), &key).await, expected);
    let start = part_size as u64 - 10;
    let ranged = collect_storage_stream(
        store
            .get_blob_range(&key, start..start + 20)
            .await
            .expect("range")
            .body,
    )
    .await
    .expect("range bytes");
    assert_eq!(&ranged[..], &expected[start as usize..start as usize + 20]);
    // A completed upload is gone.
    assert_eq!(
        kind(store.list_parts(&handle).await.unwrap_err()),
        MultipartErrorKind::UnknownUpload
    );
    store.delete_blob(&key).await.expect("cleanup");
}

async fn resume_from_a_persisted_handle(make: &Make, prefix: &str, part_size: usize) {
    let key = format!("{prefix}/resumed.bin");
    let persisted = {
        let first = make();
        let handle = first
            .start_multipart(&key, MultipartStartOptions::default())
            .await
            .expect("start");
        upload(first.as_ref(), &handle, 1, 7, part_size).await;
        serde_json::to_string(&handle).expect("handle serializes")
    };
    // A new instance continues the upload from the persisted handle alone.
    let second = make();
    let handle: MultipartUploadHandle = serde_json::from_str(&persisted).expect("handle");
    let listed = second.list_parts(&handle).await.expect("resume listing");
    assert_eq!(listed.len(), 1);
    let (_, last) = upload(second.as_ref(), &handle, 2, 8, 99).await;
    let third = make();
    let receipts = [listed[0].clone(), last];
    let outcome = third
        .complete_multipart(&handle, &receipts)
        .await
        .expect("complete");
    assert_eq!(outcome.size_bytes, part_size as u64 + 99);
    let mut expected = part(7, part_size).0;
    expected.extend(part(8, 99).0);
    assert_eq!(read(third.as_ref(), &key).await, expected);
    third.delete_blob(&key).await.expect("cleanup");
}

async fn retry_replaces_a_part(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let key = format!("{prefix}/retried.bin");
    let handle = store
        .start_multipart(&key, MultipartStartOptions::default())
        .await
        .expect("start");
    let (first, first_receipt) = upload(store.as_ref(), &handle, 1, 1, part_size).await;
    upload(store.as_ref(), &handle, 2, 2, 50).await;
    let (retried, retried_receipt) = upload(store.as_ref(), &handle, 2, 3, 60).await;
    let listed = store.list_parts(&handle).await.expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].size_bytes, 60);
    store
        .complete_multipart(&handle, &[first_receipt, retried_receipt])
        .await
        .expect("complete");
    let mut expected = first;
    expected.extend(retried);
    assert_eq!(read(store.as_ref(), &key).await, expected);
    store.delete_blob(&key).await.expect("cleanup");
}

async fn mismatched_receipts_leave_the_parts(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let key = format!("{prefix}/mismatch.bin");
    let handle = store
        .start_multipart(&key, MultipartStartOptions::default())
        .await
        .expect("start");
    let (_, one) = upload(store.as_ref(), &handle, 1, 1, part_size).await;
    let (_, two) = upload(store.as_ref(), &handle, 2, 2, 10).await;
    let missing = store
        .complete_multipart(&handle, std::slice::from_ref(&one))
        .await
        .unwrap_err();
    assert_eq!(kind(missing), MultipartErrorKind::ReceiptMismatch);
    let mut changed = two.clone();
    changed.size_bytes += 1;
    let mismatched = store
        .complete_multipart(&handle, &[one.clone(), changed])
        .await
        .unwrap_err();
    assert_eq!(kind(mismatched), MultipartErrorKind::ReceiptMismatch);
    let reordered = store
        .complete_multipart(&handle, &[two.clone(), one.clone()])
        .await
        .unwrap_err();
    assert_eq!(kind(reordered), MultipartErrorKind::ReceiptMismatch);
    // Nothing was published and every part is still stored.
    assert!(!store.blob_exists(&key).await.expect("exists"));
    assert_eq!(store.list_parts(&handle).await.expect("list").len(), 2);
    store
        .complete_multipart(&handle, &[one, two])
        .await
        .expect("a corrected completion succeeds");
    store.delete_blob(&key).await.expect("cleanup");
}

async fn abort_is_idempotent(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let key = format!("{prefix}/aborted.bin");
    let handle = store
        .start_multipart(&key, MultipartStartOptions::default())
        .await
        .expect("start");
    let (_, receipt) = upload(store.as_ref(), &handle, 1, 1, part_size).await;
    store.abort_multipart(&handle).await.expect("abort");
    make().abort_multipart(&handle).await.expect("abort again");
    assert_eq!(
        kind(store.list_parts(&handle).await.unwrap_err()),
        MultipartErrorKind::UnknownUpload
    );
    assert_eq!(
        kind(
            store
                .complete_multipart(&handle, &[receipt])
                .await
                .unwrap_err()
        ),
        MultipartErrorKind::UnknownUpload
    );
    let (_, body) = part(1, 10);
    assert_eq!(
        kind(store.upload_part(&handle, 2, body).await.unwrap_err()),
        MultipartErrorKind::UnknownUpload
    );
    assert!(!store.blob_exists(&key).await.expect("exists"));
}

async fn stale_uploads_are_listed_and_cleaned(make: &Make, prefix: &str) {
    let store = make();
    let stale = format!("{prefix}/stale");
    let mut started = Vec::new();
    for name in ["a", "b", "c"] {
        started.push(
            store
                .start_multipart(&format!("{stale}/{name}"), MultipartStartOptions::default())
                .await
                .expect("start"),
        );
    }
    let mut listed = Vec::new();
    let mut continuation = None;
    loop {
        let page = make()
            .list_incomplete_multipart(&stale, continuation, 2)
            .await
            .expect("list incomplete");
        assert!(page.uploads.len() <= 2);
        listed.extend(page.uploads);
        continuation = page.next_continuation;
        if continuation.is_none() {
            break;
        }
    }
    let mut handles = listed
        .iter()
        .map(|upload| upload.handle.clone())
        .collect::<Vec<_>>();
    handles.sort_by(|a, b| a.key.cmp(&b.key));
    started.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(handles, started);
    assert!(listed.iter().all(|upload| upload.started_at.is_some()));
    for upload in &listed {
        make()
            .abort_multipart(&upload.handle)
            .await
            .expect("cleanup");
    }
    let page = store
        .list_incomplete_multipart(&stale, None, 10)
        .await
        .expect("list after cleanup");
    assert!(page.uploads.is_empty(), "{page:?}");
}

/// Mandatory on every provider: a conditional completion against an object
/// published meanwhile must fail loudly and never overwrite it.
async fn if_not_exists_conflicts_at_completion(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let key = format!("{prefix}/exclusive.bin");
    let handle = store
        .start_multipart(
            &key,
            MultipartStartOptions {
                if_not_exists: true,
                ..Default::default()
            },
        )
        .await
        .expect("start");
    let (_, one) = upload(store.as_ref(), &handle, 1, 1, part_size).await;
    // Another writer publishes the key after the upload started.
    store
        .put_blob(
            &key,
            StorageByteStream::from_bytes(Bytes::from_static(b"winner")),
            BlobPutOptions::default(),
        )
        .await
        .expect("competing write");
    let conflict = store
        .complete_multipart(&handle, std::slice::from_ref(&one))
        .await
        .unwrap_err();
    assert!(
        matches!(conflict, StorageError::PreconditionFailed { .. }),
        "{conflict:?}"
    );
    assert_eq!(read(store.as_ref(), &key).await, b"winner");
    assert_eq!(
        store.list_parts(&handle).await.expect("parts remain").len(),
        1
    );
    store.abort_multipart(&handle).await.expect("abort");
    store.delete_blob(&key).await.expect("cleanup");
}

async fn part_boundaries(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let limits = store.multipart_limits();
    let key = format!("{prefix}/boundaries.bin");
    let handle = store
        .start_multipart(&key, MultipartStartOptions::default())
        .await
        .expect("start");
    for number in [0, limits.max_parts + 1] {
        let (_, body) = part(1, 1);
        assert_eq!(
            kind(store.upload_part(&handle, number, body).await.unwrap_err()),
            MultipartErrorKind::PartNumberOutOfRange
        );
    }
    let (_, body) = part(1, usize::try_from(limits.max_part_size).unwrap() + 1);
    assert_eq!(
        kind(store.upload_part(&handle, 1, body).await.unwrap_err()),
        MultipartErrorKind::PartTooLarge
    );
    // The maximum itself is accepted.
    let (_, max) = upload(
        store.as_ref(),
        &handle,
        limits.max_parts,
        2,
        usize::try_from(limits.max_part_size).unwrap(),
    )
    .await;
    if limits.min_part_size > 1 {
        // Every part but the last must reach the minimum.
        let (_, small) = upload(
            store.as_ref(),
            &handle,
            1,
            3,
            usize::try_from(limits.min_part_size).unwrap() - 1,
        )
        .await;
        assert_eq!(
            kind(
                store
                    .complete_multipart(&handle, &[small, max.clone()])
                    .await
                    .unwrap_err()
            ),
            MultipartErrorKind::PartTooSmall
        );
        let (_, exact) = upload(
            store.as_ref(),
            &handle,
            1,
            3,
            usize::try_from(limits.min_part_size).unwrap(),
        )
        .await;
        store
            .complete_multipart(&handle, &[exact, max])
            .await
            .expect("a minimum-size part completes");
        store.delete_blob(&key).await.expect("cleanup");
    } else {
        let _ = part_size;
        store.abort_multipart(&handle).await.expect("abort");
    }
}

async fn concurrent_uploads_of_one_part_leave_one(make: &Make, prefix: &str, part_size: usize) {
    let store = make();
    let key = format!("{prefix}/concurrent.bin");
    let handle = store
        .start_multipart(&key, MultipartStartOptions::default())
        .await
        .expect("start");
    let tasks = (0..4_u8)
        .map(|seed| {
            let store = make();
            let handle = handle.clone();
            tokio::spawn(async move {
                let (bytes, body) = part(seed, part_size);
                store
                    .upload_part(&handle, 1, body)
                    .await
                    .map(|receipt| (bytes, receipt))
            })
        })
        .collect::<Vec<_>>();
    let mut uploaded = Vec::new();
    for task in tasks {
        uploaded.push(task.await.expect("task").expect("concurrent upload"));
    }
    let listed = store.list_parts(&handle).await.expect("list");
    assert_eq!(listed.len(), 1);
    let (bytes, _) = uploaded
        .iter()
        // Listings may omit checksums; the provider tag identifies the part.
        .find(|(_, receipt)| {
            receipt.provider_tag == listed[0].provider_tag
                && receipt.size_bytes == listed[0].size_bytes
        })
        .expect("the stored part is one complete upload");
    store
        .complete_multipart(&handle, &listed)
        .await
        .expect("complete");
    assert_eq!(&read(store.as_ref(), &key).await, bytes);
    store.delete_blob(&key).await.expect("cleanup");
}

async fn completion_racing_abort_has_one_outcome(make: &Make, prefix: &str, part_size: usize) {
    for round in 0..4 {
        let store = make();
        let key = format!("{prefix}/race-{round}.bin");
        let handle = store
            .start_multipart(&key, MultipartStartOptions::default())
            .await
            .expect("start");
        let (bytes, receipt) = upload(store.as_ref(), &handle, 1, round, part_size).await;
        let completing = {
            let store = make();
            let handle = handle.clone();
            tokio::spawn(async move { store.complete_multipart(&handle, &[receipt]).await })
        };
        let aborting = {
            let store = make();
            let handle = handle.clone();
            tokio::spawn(async move { store.abort_multipart(&handle).await })
        };
        let completed = completing.await.expect("task");
        aborting.await.expect("task").expect("abort never fails");
        match completed {
            Ok(_) => assert_eq!(read(store.as_ref(), &key).await, bytes),
            Err(error) => {
                assert_eq!(kind(error), MultipartErrorKind::UnknownUpload);
                assert!(!store.blob_exists(&key).await.expect("exists"));
            }
        }
        // Either way, nothing is left to resume.
        assert_eq!(
            kind(store.list_parts(&handle).await.unwrap_err()),
            MultipartErrorKind::UnknownUpload
        );
        let _ = store.delete_blob(&key).await;
    }
}
