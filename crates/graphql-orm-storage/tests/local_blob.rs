use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, LocalStorageBackend, StorageByteStream, StorageError,
    collect_storage_stream, sha256_hex,
};
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

#[path = "support/listing_contract.rs"]
mod listing_contract;

#[tokio::test]
async fn local_listing_follows_the_provider_prefix_contract() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    // A sibling sharing the base's string prefix stays outside `base/`.
    backend
        .put_blob(
            "base-sibling/a",
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put sibling");
    listing_contract::prefixes_are_string_prefixes(&backend, "contract").await;
    assert_eq!(
        backend.list_blobs("base").await.expect("list"),
        ["base-sibling/a"]
    );
}

#[tokio::test]
async fn local_reserves_the_internal_namespace() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    std::fs::create_dir_all(temp.path().join(".graphql-orm-storage/multipart"))
        .expect("internal dir");
    std::fs::write(
        temp.path().join(".graphql-orm-storage/multipart/x"),
        b"internal",
    )
    .expect("internal file");
    listing_contract::the_internal_namespace_is_reserved(&backend).await;
    assert_eq!(
        std::fs::read(temp.path().join(".graphql-orm-storage/multipart/x")).expect("read"),
        b"internal"
    );
    assert!(backend.list_blobs("").await.expect("list").is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn local_listing_skips_names_that_are_not_valid_keys() {
    use std::os::unix::ffi::OsStrExt;

    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    backend
        .put_blob(
            "dir/kept",
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put");
    let dir = temp.path().join("dir");
    // Two distinct non-UTF-8 names would otherwise decode to one lossy key.
    std::fs::write(dir.join(std::ffi::OsStr::from_bytes(b"bad-\xff")), b"x").expect("write");
    std::fs::write(dir.join(std::ffi::OsStr::from_bytes(b"bad-\xfe")), b"x").expect("write");
    std::fs::write(dir.join("back\\slash"), b"x").expect("write");
    std::fs::create_dir_all(temp.path().join(".GRAPHQL-ORM-STORAGE")).expect("mkdir");
    std::fs::write(temp.path().join(".GRAPHQL-ORM-STORAGE/x"), b"x").expect("write");
    let undecodable = temp
        .path()
        .join(std::ffi::OsStr::from_bytes(b"undecodable-\xff"));
    std::fs::create_dir_all(&undecodable).expect("mkdir");
    std::fs::write(undecodable.join("child"), b"x").expect("write");

    assert_eq!(backend.list_blobs("").await.expect("list"), ["dir/kept"]);
    assert_eq!(
        backend.list_blobs("dir/").await.expect("list"),
        ["dir/kept"]
    );
    assert_eq!(
        backend
            .list_blobs_page("", None, 1)
            .await
            .expect("page")
            .keys,
        ["dir/kept"]
    );
}

#[tokio::test]
async fn local_blob_put_get_delete_round_trip() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let outcome = backend
        .put_blob(
            "snapshots/a/manifest.json",
            StorageByteStream::from_bytes(Bytes::from_static(b"manifest")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put blob");

    assert_eq!(outcome.size_bytes, 8);
    assert_eq!(outcome.sha256_hex, sha256_hex(b"manifest"));
    assert!(
        backend
            .blob_exists("snapshots/a/manifest.json")
            .await
            .expect("exists")
    );

    let body = backend
        .get_blob("snapshots/a/manifest.json")
        .await
        .expect("get blob");
    assert_eq!(body.key, "snapshots/a/manifest.json");
    assert_eq!(
        collect_storage_stream(body.body)
            .await
            .expect("collect body"),
        Bytes::from_static(b"manifest")
    );

    backend
        .delete_blob("snapshots/a/manifest.json")
        .await
        .expect("delete blob");
    assert!(
        !backend
            .blob_exists("snapshots/a/manifest.json")
            .await
            .expect("exists")
    );
}

#[tokio::test]
async fn local_blob_head_and_exists_handle_present_and_missing_blobs() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    assert!(
        !backend
            .blob_exists("objects/sha256/aa/bb/hash")
            .await
            .expect("exists")
    );
    assert_eq!(
        backend
            .head_blob("objects/sha256/aa/bb/hash")
            .await
            .expect("head missing"),
        None
    );

    backend
        .put_blob(
            "objects/sha256/aa/bb/hash",
            StorageByteStream::from_bytes(Bytes::from_static(b"object")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put blob");

    let metadata = backend
        .head_blob("objects/sha256/aa/bb/hash")
        .await
        .expect("head blob")
        .expect("metadata");
    assert_eq!(metadata.key, "objects/sha256/aa/bb/hash");
    assert_eq!(metadata.size_bytes, Some(6));
    assert_eq!(metadata.sha256_hex, None);
}

#[tokio::test]
async fn local_blob_list_blobs_supports_empty_and_non_empty_prefixes() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_blob(
            "snapshots/a/manifest.json",
            StorageByteStream::from_bytes(Bytes::from_static(b"manifest")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put manifest");
    backend
        .put_blob(
            "objects/sha256/aa/bb/hash",
            StorageByteStream::from_bytes(Bytes::from_static(b"object")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put object");

    assert_eq!(
        backend.list_blobs("").await.expect("list all"),
        vec![
            "objects/sha256/aa/bb/hash".to_string(),
            "snapshots/a/manifest.json".to_string()
        ]
    );
    assert_eq!(
        backend.list_blobs("snapshots").await.expect("list prefix"),
        vec!["snapshots/a/manifest.json".to_string()]
    );
    assert!(
        backend
            .list_blobs("missing")
            .await
            .expect("list missing")
            .is_empty()
    );
}

#[tokio::test]
async fn local_blob_paged_listing_returns_continuation_tokens() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    for key in ["prefix/a", "prefix/b", "prefix/c"] {
        backend
            .put_blob(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .expect("put blob");
    }

    let first_page = backend
        .list_blobs_page("prefix", None, 2)
        .await
        .expect("first page");
    assert_eq!(
        first_page.keys,
        vec!["prefix/a".to_string(), "prefix/b".to_string()]
    );
    assert_eq!(first_page.next_continuation, Some("prefix/b".to_string()));

    let second_page = backend
        .list_blobs_page("prefix", first_page.next_continuation, 2)
        .await
        .expect("second page");
    assert_eq!(second_page.keys, vec!["prefix/c".to_string()]);
    assert_eq!(second_page.next_continuation, None);
}

#[tokio::test]
async fn local_blob_range_reads_return_requested_bytes() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_blob(
            "objects/ranged",
            StorageByteStream::from_bytes(Bytes::from_static(b"0123456789")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put blob");

    let body = backend
        .get_blob_range("objects/ranged", 2..6)
        .await
        .expect("range read");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"2345"));
}

#[tokio::test]
async fn local_blob_conditional_write_returns_none_when_key_exists() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let first = backend
        .put_blob_if_not_exists(
            "objects/dedup",
            StorageByteStream::from_bytes(Bytes::from_static(b"first")),
            BlobPutOptions::default(),
        )
        .await
        .expect("first conditional write");
    assert!(first.is_some());

    let second = backend
        .put_blob_if_not_exists(
            "objects/dedup",
            StorageByteStream::from_bytes(Bytes::from_static(b"second")),
            BlobPutOptions::default(),
        )
        .await
        .expect("second conditional write");
    assert_eq!(second, None);

    let body = backend.get_blob("objects/dedup").await.expect("get blob");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"first"));
}

#[tokio::test]
async fn local_blob_copy_promotes_without_download_api() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_blob(
            "temp/object",
            StorageByteStream::from_bytes(Bytes::from_static(b"copy me")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put source");

    backend
        .copy_blob("temp/object", "originals/object")
        .await
        .expect("copy blob");

    let body = backend
        .get_blob("originals/object")
        .await
        .expect("get copied");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"copy me"));
}

#[tokio::test]
async fn local_blob_delete_missing_succeeds() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .delete_blob("objects/sha256/aa/bb/hash")
        .await
        .expect("delete missing");
}

#[tokio::test]
async fn local_blob_get_missing_returns_missing_blob() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let err = backend
        .get_blob("objects/sha256/aa/bb/hash")
        .await
        .expect_err("missing blob");

    assert!(matches!(err, StorageError::MissingBlob { .. }));
}

#[tokio::test]
async fn local_blob_rejects_invalid_keys_for_all_operations() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    assert_invalid(
        backend
            .put_blob(
                "../escape",
                StorageByteStream::from_bytes(Bytes::new()),
                BlobPutOptions::default(),
            )
            .await,
    );
    assert_invalid(
        backend
            .put_blob_if_not_exists(
                "../escape",
                StorageByteStream::from_bytes(Bytes::new()),
                BlobPutOptions::default(),
            )
            .await,
    );
    assert_invalid(backend.get_blob("../escape").await);
    assert_invalid(backend.get_blob_range("../escape", 0..1).await);
    assert_invalid(backend.blob_exists("../escape").await);
    assert_invalid(backend.head_blob("../escape").await);
    assert_invalid(backend.list_blobs("../escape").await);
    assert_invalid(backend.list_blobs_page("../escape", None, 100).await);
    assert_invalid(backend.copy_blob("../escape", "objects/safe").await);
    assert_invalid(backend.copy_blob("objects/safe", "../escape").await);
    assert_invalid(backend.delete_blob("../escape").await);
}

#[tokio::test]
async fn local_blob_list_blobs_ignores_uploading_temp_files() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let temp_path = temp
        .path()
        .join(format!("snapshots/a/.{}.uploading", Uuid::new_v4()));
    tokio::fs::create_dir_all(temp_path.parent().expect("temp parent"))
        .await
        .expect("create parent");
    tokio::fs::write(&temp_path, b"partial")
        .await
        .expect("write temp");

    assert!(backend.list_blobs("").await.expect("list all").is_empty());
}

#[tokio::test]
async fn local_blob_sweep_temp_files_removes_stale_uploads() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let temp_path = temp
        .path()
        .join(format!("snapshots/a/.{}.uploading", Uuid::new_v4()));
    tokio::fs::create_dir_all(temp_path.parent().expect("temp parent"))
        .await
        .expect("create parent");
    tokio::fs::write(&temp_path, b"partial")
        .await
        .expect("write temp");

    let removed = backend
        .sweep_temp_files(Duration::ZERO)
        .await
        .expect("sweep temp files");

    assert_eq!(removed, 1);
    assert!(!temp_path.exists());
}

#[tokio::test]
async fn local_blob_sweep_temp_files_keeps_recent_uploads() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let temp_path = temp
        .path()
        .join(format!("snapshots/a/.{}.uploading", Uuid::new_v4()));
    tokio::fs::create_dir_all(temp_path.parent().expect("temp parent"))
        .await
        .expect("create parent");
    tokio::fs::write(&temp_path, b"partial")
        .await
        .expect("write temp");

    let removed = backend
        .sweep_temp_files(Duration::from_secs(86_400))
        .await
        .expect("sweep temp files");

    assert_eq!(removed, 0);
    assert!(temp_path.exists());
}

fn assert_invalid<T>(result: Result<T, StorageError>) {
    assert!(matches!(
        result,
        Err(StorageError::InvalidStorageKey { .. })
    ));
}

#[tokio::test]
async fn single_attempt_local_delete_failure_is_not_retryable() {
    let temp = TempDir::new().expect("owned fixture");
    tokio::fs::create_dir(temp.path().join("directory"))
        .await
        .expect("directory");
    let backend = LocalStorageBackend::new(temp.path());
    let ordinary = backend
        .delete_blob("directory")
        .await
        .expect_err("cannot unlink directory");
    assert!(ordinary.is_retryable(), "ordinary I/O taxonomy is retained");
    let once = backend
        .delete_blob_once("directory")
        .await
        .expect_err("cannot unlink directory");
    assert!(
        !once.is_retryable(),
        "lock deletion must not invite a retry"
    );
    assert!(matches!(once, StorageError::SingleAttemptFailed { .. }));
}

#[cfg(unix)]
#[tokio::test]
async fn local_blob_sweep_skips_linked_files_directories_and_cycles() {
    use std::os::unix::fs::symlink;

    let owned = TempDir::new().expect("owned fixture");
    let root = owned.path().join("storage");
    let outside = owned.path().join("outside-storage");
    tokio::fs::create_dir_all(&root).await.expect("root");
    tokio::fs::create_dir_all(&outside).await.expect("outside");
    let outside_upload = outside.join(format!(".{}.uploading", Uuid::new_v4()));
    tokio::fs::write(&outside_upload, b"owned external fixture")
        .await
        .expect("outside upload");
    let real_upload = root.join(format!(".{}.uploading", Uuid::new_v4()));
    tokio::fs::write(&real_upload, b"abandoned")
        .await
        .expect("real upload");
    symlink(&outside, root.join("linked-directory")).expect("directory link");
    let linked_file = format!(".{}.uploading", Uuid::new_v4());
    symlink(&outside_upload, root.join(&linked_file)).expect("file link");
    symlink(&root, root.join("cycle")).expect("cycle");
    let broken_link = format!(".{}.uploading", Uuid::new_v4());
    symlink(outside.join("missing"), root.join(&broken_link)).expect("broken link");

    let backend = LocalStorageBackend::new(&root);
    let removed = tokio::time::timeout(
        Duration::from_secs(5),
        backend.sweep_temp_files(Duration::ZERO),
    )
    .await
    .expect("cycle must not cause unbounded traversal")
    .expect("sweep");
    assert_eq!(removed, 1);
    assert!(!real_upload.exists());
    assert_eq!(
        tokio::fs::read(&outside_upload)
            .await
            .expect("outside preserved"),
        b"owned external fixture"
    );
    for name in ["linked-directory", &linked_file, "cycle", &broken_link] {
        assert!(
            tokio::fs::symlink_metadata(root.join(name))
                .await
                .expect("link retained")
                .file_type()
                .is_symlink()
        );
    }

    let linked_root = owned.path().join("linked-root");
    symlink(&outside, &linked_root).expect("root link");
    assert_eq!(
        LocalStorageBackend::new(linked_root)
            .sweep_temp_files(Duration::ZERO)
            .await
            .expect("skip linked root"),
        0
    );
    assert!(outside_upload.exists());
}

#[tokio::test]
async fn local_blob_keys_ending_in_uploading_are_objects_not_temp_files() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let uuid = Uuid::new_v4();
    // Only `.<uuid>.uploading` is a temporary name. Everything else that ends
    // in `.uploading` is a caller's object, including the `<name>.<uuid>`
    // form earlier versions used for their own temporary files.
    let mut keys = vec![
        "reports/quarterly.uploading".to_string(),
        "reports/QUARTERLY.UPLOADING".to_string(),
        "reports/.uploading".to_string(),
        format!("reports/report.{uuid}.uploading"),
        format!("reports/.report.{uuid}.uploading"),
        format!("reports/{uuid}.uploading"),
        format!(
            "reports/.{}.uploading",
            uuid.hyphenated().to_string().to_uppercase()
        ),
        format!("reports/.{}.uploading", uuid.simple()),
        format!("reports/.{uuid}.uploading.bin"),
    ];
    for key in &keys {
        backend
            .put_blob(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"stored")),
                BlobPutOptions::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("put {key}: {error}"));
    }
    keys.sort();

    assert_eq!(backend.list_blobs("").await.expect("list"), keys);
    let page = backend
        .list_blobs_page("reports", None, 100)
        .await
        .expect("page");
    assert_eq!(page.keys, keys);
    assert_eq!(
        backend
            .sweep_temp_files(Duration::ZERO)
            .await
            .expect("sweep"),
        0
    );
    for key in &keys {
        assert!(
            backend.blob_exists(key).await.expect("exists"),
            "{key} kept"
        );
    }
}

#[tokio::test]
async fn local_blob_refuses_to_write_a_key_named_like_a_temp_file() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let reserved = format!("reserved/.{}.uploading", Uuid::new_v4());
    let body = || StorageByteStream::from_bytes(Bytes::from_static(b"x"));

    assert_invalid(
        backend
            .put_blob(&reserved, body(), BlobPutOptions::default())
            .await,
    );
    assert_invalid(
        backend
            .put_blob_if_not_exists(&reserved, body(), BlobPutOptions::default())
            .await,
    );
    backend
        .put_blob("source", body(), BlobPutOptions::default())
        .await
        .expect("source");
    assert_invalid(backend.copy_blob("source", &reserved).await);
    assert!(
        !temp.path().join("reserved").exists(),
        "a refused write creates no directory"
    );
}

#[tokio::test]
async fn local_blob_writes_leave_no_temp_files_and_interrupted_ones_are_swept() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    backend
        .put_blob(
            "nested/blob.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"blob")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put");
    let mut names = std::fs::read_dir(temp.path().join("nested"))
        .expect("read dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["blob.bin"]);

    let interrupted = temp
        .path()
        .join(format!("nested/.{}.uploading", Uuid::new_v4()));
    tokio::fs::write(&interrupted, b"partial")
        .await
        .expect("interrupted upload");
    assert_eq!(
        backend.list_blobs("").await.expect("list"),
        ["nested/blob.bin"]
    );
    assert_eq!(
        backend
            .sweep_temp_files(Duration::ZERO)
            .await
            .expect("sweep"),
        1
    );
    assert!(!interrupted.exists());
}

/// Name components chosen to sort around `/` (0x2F) and across UTF-8 widths,
/// so a sibling file or directory can fall between a directory and its
/// children in key order.
const TRICKY_COMPONENTS: &[&str] = &[
    "a",
    "a-b",
    "a.b",
    "a b",
    "a!",
    "a+",
    "a0",
    "a;",
    "a~",
    "ab",
    "a_",
    "-",
    ".a",
    "a.",
    "-a",
    "A",
    "z",
    "é",
    "e",
    "ä",
    "日本",
    "a\u{7f}",
    "a\u{e9}",
    "a\u{10000}",
];

/// Deterministic xorshift generator, so failures reproduce from the seed.
struct KeyRng(u64);

impl KeyRng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).expect("index fits")
    }
}

/// Generates a key set in which no key is also a directory of another key.
fn tricky_key_set(rng: &mut KeyRng) -> Vec<String> {
    let mut keys = std::collections::BTreeSet::new();
    for _ in 0..(4 + rng.below(20)) {
        let depth = 1 + rng.below(3);
        let key = (0..depth)
            .map(|_| TRICKY_COMPONENTS[rng.below(TRICKY_COMPONENTS.len())])
            .collect::<Vec<_>>()
            .join("/");
        let conflicts = keys.iter().any(|existing: &String| {
            existing.starts_with(&format!("{key}/")) || key.starts_with(&format!("{existing}/"))
        });
        if !conflicts {
            keys.insert(key);
        }
    }
    keys.into_iter().collect()
}

async fn drain_pages(backend: &LocalStorageBackend, prefix: &str, limit: usize) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut continuation = None;
    loop {
        let page = backend
            .list_blobs_page(prefix, continuation.clone(), limit)
            .await
            .expect("list page");
        assert!(page.keys.len() <= limit, "page exceeds its limit");
        if let Some(token) = &continuation {
            assert!(
                page.keys.iter().all(|key| key > token),
                "page {:?} repeats keys at or before {token:?}",
                page.keys
            );
        }
        keys.extend(page.keys);
        continuation = page.next_continuation;
        if continuation.is_none() {
            return keys;
        }
    }
}

#[tokio::test]
async fn local_blob_paged_listing_is_in_exact_key_order_for_tricky_sibling_names() {
    for seed in 1..=40_u64 {
        let mut rng = KeyRng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let keys = tricky_key_set(&mut rng);
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        for key in &keys {
            backend
                .put_blob(
                    key,
                    StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                    BlobPutOptions::default(),
                )
                .await
                .expect("put blob");
        }

        assert_eq!(
            backend.list_blobs("").await.expect("list all"),
            keys,
            "seed {seed}: full listing order"
        );
        for limit in 1..=keys.len() + 1 {
            assert_eq!(
                drain_pages(&backend, "", limit).await,
                keys,
                "seed {seed}: pages of {limit}"
            );
        }

        // Every prefix, whether a directory, a directory and its separator, or
        // a partial name, pages through exactly the keys it string-prefixes.
        let prefixes = keys
            .iter()
            .flat_map(|key| {
                let parts = key.split('/').collect::<Vec<_>>();
                (1..parts.len())
                    .flat_map(move |depth| {
                        let directory = parts[..depth].join("/");
                        // `.` alone is not a valid component, so not a prefix.
                        let first = parts[depth].chars().next().expect("component");
                        let partial = (first != '.').then(|| format!("{directory}/{first}"));
                        [Some(format!("{directory}/")), partial, Some(directory)]
                    })
                    .flatten()
            })
            .collect::<std::collections::BTreeSet<_>>();
        for prefix in prefixes {
            let expected = keys
                .iter()
                .filter(|key| key.starts_with(&prefix))
                .cloned()
                .collect::<Vec<_>>();
            for limit in [1, 2, 3] {
                assert_eq!(
                    drain_pages(&backend, &prefix, limit).await,
                    expected,
                    "seed {seed}: prefix {prefix:?}, pages of {limit}"
                );
            }
        }
    }
}

#[tokio::test]
async fn local_blob_pagination_does_not_skip_a_sibling_sorting_below_a_directory() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    for key in ["a/x", "a-b", "a.b/y", "b"] {
        backend
            .put_blob(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .expect("put blob");
    }

    let expected = ["a-b", "a.b/y", "a/x", "b"].map(String::from).to_vec();
    assert_eq!(backend.list_blobs("").await.expect("list all"), expected);
    assert_eq!(drain_pages(&backend, "", 1).await, expected);
}

/// Asserts that a local operation refused its key as invalid.
fn assert_refused_key<T: std::fmt::Debug>(result: Result<T, StorageError>, what: &str) {
    assert!(
        matches!(result, Err(StorageError::InvalidStorageKey { .. })),
        "{what} must be refused: {result:?}"
    );
}

/// A name that resolves to the internal directory is refused by identity, not
/// by spelling. A case-insensitive filesystem resolves `.GRAPHQL-ORM-ſTORAGE`
/// (or a Windows short name) to the same directory; a symbolic link stands in
/// for such an alias on a case-sensitive test filesystem.
#[cfg(unix)]
#[tokio::test]
async fn local_refuses_any_name_that_resolves_to_the_internal_directory() {
    use graphql_orm_storage::{
        MultipartBlobStore, MultipartStartOptions, ObjectMetadata, StreamingObjectStore,
    };

    let temp = TempDir::new().expect("temp dir");
    let internal = temp.path().join(".graphql-orm-storage");
    std::fs::create_dir_all(internal.join("multipart")).expect("internal dir");
    std::fs::write(internal.join("multipart/x"), b"internal").expect("internal file");
    std::os::unix::fs::symlink(&internal, temp.path().join("alias")).expect("alias");
    let backend = LocalStorageBackend::new(temp.path());
    backend
        .put_blob(
            "kept/a",
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put an ordinary key");

    for key in ["alias", "alias/multipart/x", "alias/new"] {
        assert_refused_key(
            backend
                .put_blob(
                    key,
                    StorageByteStream::from_bytes(Bytes::from_static(b"overwrite")),
                    BlobPutOptions::default(),
                )
                .await,
            key,
        );
        assert_refused_key(
            backend
                .put_blob_if_not_exists(
                    key,
                    StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                    BlobPutOptions::default(),
                )
                .await,
            key,
        );
        assert_refused_key(backend.get_blob(key).await, key);
        assert_refused_key(backend.blob_exists(key).await, key);
        assert_refused_key(backend.head_blob(key).await, key);
        assert_refused_key(backend.delete_blob(key).await, key);
        assert_refused_key(backend.copy_blob(key, "kept/copy").await, key);
        assert_refused_key(backend.copy_blob("kept/a", key).await, key);
        assert_refused_key(
            backend
                .start_multipart(key, MultipartStartOptions::default())
                .await,
            key,
        );
    }
    for prefix in ["alias", "alias/", "alias/multipart"] {
        assert_refused_key(backend.list_blobs(prefix).await, prefix);
        assert_refused_key(
            backend.list_incomplete_multipart(prefix, None, 10).await,
            prefix,
        );
    }
    // A partial name lists around the alias, never through it.
    assert_eq!(backend.list_blobs("").await.expect("list"), ["kept/a"]);
    assert!(backend.list_blobs("al").await.expect("list").is_empty());
    assert_eq!(
        backend
            .list_blobs_page("", None, 1)
            .await
            .expect("page")
            .keys,
        ["kept/a"]
    );

    // A bucket is refused the same way.
    assert_refused_key(
        backend
            .put_object_stream(
                "alias",
                "objects/x",
                None,
                ObjectMetadata::new(),
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            )
            .await,
        "bucket alias",
    );
    assert_refused_key(backend.list_objects("alias", "").await, "bucket alias");
    assert_refused_key(
        backend.get_object_metadata("alias", "multipart/x").await,
        "bucket alias",
    );

    assert_eq!(
        std::fs::read(internal.join("multipart/x")).expect("read"),
        b"internal"
    );
    assert!(!internal.join("new").exists());
}

/// A write creates the internal directory before it checks identities, so a
/// name that would alias it on a case-insensitive filesystem can never claim
/// that name first.
#[tokio::test]
async fn local_writes_create_the_internal_directory_first() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    backend
        .put_blob(
            "a",
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put");
    assert!(temp.path().join(".graphql-orm-storage").is_dir());
    assert_eq!(backend.list_blobs("").await.expect("list"), ["a"]);
}

/// `:` would name an alternate data stream on Windows, as in
/// `.graphql-orm-storage::$INDEX_ALLOCATION`; local keys refuse it everywhere.
#[tokio::test]
async fn local_refuses_colons_in_keys_buckets_and_prefixes() {
    use graphql_orm_storage::{ObjectMetadata, StreamingObjectStore};

    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    for key in [
        "a:b",
        "dir/x::$INDEX_ALLOCATION",
        ".graphql-orm-storage::$DATA",
    ] {
        assert_refused_key(
            backend
                .put_blob(
                    key,
                    StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                    BlobPutOptions::default(),
                )
                .await,
            key,
        );
        assert_refused_key(backend.get_blob(key).await, key);
        assert_refused_key(backend.list_blobs(key).await, key);
    }
    assert_refused_key(
        backend
            .put_object_stream(
                "bucket:x",
                "a",
                None,
                ObjectMetadata::new(),
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            )
            .await,
        "bucket",
    );
    assert_refused_key(
        backend
            .put_object_stream(
                "bucket",
                "a:b",
                None,
                ObjectMetadata::new(),
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            )
            .await,
        "object key",
    );
    // A stored name with a colon cannot be addressed, so it is not listed.
    std::fs::create_dir_all(temp.path().join("dir")).expect("mkdir");
    std::fs::write(temp.path().join("dir/c:d"), b"x").expect("write");
    std::fs::write(temp.path().join("dir/e"), b"x").expect("write");
    assert_eq!(backend.list_blobs("").await.expect("list"), ["dir/e"]);
}
