//! Provider-neutral listing contract, run against every `BlobStore` provider.
//!
//! A listing prefix is a plain string prefix of the key: `a` matches `a/x`,
//! `a-b/x` and `ab`, while `a/` matches only keys below `a/`. Each provider's
//! test calls [`prefixes_are_string_prefixes`] with a `base` component that no
//! other key in the store starts with.

use bytes::Bytes;
use graphql_orm_storage::{BlobPutOptions, BlobStore, StorageByteStream, StorageError};

/// Keys below `base/`, in byte order. `-` and `.` sort before `/`, so `a-b/x`
/// and `a.b` precede `a/x`. `a` is a directory, never a file, because a
/// filesystem cannot hold both.
const KEYS: [&str; 6] = ["a-b/x", "a.b", "a/x", "a/y/z", "ab", "b/a"];

/// Each prefix suffix below `base/` and the keys it lists.
const CASES: [(&str, &[&str]); 10] = [
    ("", &KEYS),
    ("a", &["a-b/x", "a.b", "a/x", "a/y/z", "ab"]),
    ("a/", &["a/x", "a/y/z"]),
    ("a-", &["a-b/x"]),
    ("a/y", &["a/y/z"]),
    ("a/y/", &["a/y/z"]),
    ("a/x", &["a/x"]),
    ("a/x/", &[]),
    ("b", &["b/a"]),
    ("c", &[]),
];

pub async fn prefixes_are_string_prefixes(store: &dyn BlobStore, base: &str) {
    for key in KEYS {
        store
            .put_blob(
                &format!("{base}/{key}"),
                StorageByteStream::from_bytes(Bytes::from_static(b"listing")),
                BlobPutOptions::default(),
            )
            .await
            .expect("put listing key");
    }

    for (suffix, expected) in CASES {
        let prefix = format!("{base}/{suffix}");
        let expected = expected
            .iter()
            .map(|key| format!("{base}/{key}"))
            .collect::<Vec<_>>();
        assert_eq!(
            store.list_blobs(&prefix).await.expect("list"),
            expected,
            "list_blobs({prefix:?})"
        );
        for limit in [1, 2, 4] {
            assert_eq!(
                drain_pages(store, &prefix, limit).await,
                expected,
                "list_blobs_page({prefix:?}) in pages of {limit}"
            );
        }
    }
    // The base itself, without a separator, is a string prefix too.
    assert_eq!(
        store.list_blobs(base).await.expect("list base").len(),
        KEYS.len()
    );

    // A prefix is a valid key, optionally followed by one `/`.
    for prefix in [
        format!("/{base}"),
        format!("{base}//"),
        format!("{base}/a//"),
        format!("{base}/../{base}"),
        format!("{base}/a\\b"),
        "/".to_string(),
    ] {
        assert!(
            matches!(
                store.list_blobs(&prefix).await,
                Err(StorageError::InvalidStorageKey { .. })
            ),
            "prefix {prefix:?} must be refused"
        );
    }

    for key in KEYS {
        store
            .delete_blob(&format!("{base}/{key}"))
            .await
            .expect("delete listing key");
    }
    assert!(
        store
            .list_blobs(&format!("{base}/"))
            .await
            .expect("list after cleanup")
            .is_empty()
    );
}

/// The internal `.graphql-orm-storage` namespace is reserved on every
/// provider: callers can neither address nor list it.
pub async fn the_internal_namespace_is_reserved(store: &dyn BlobStore) {
    for key in [
        ".graphql-orm-storage",
        ".graphql-orm-storage/multipart/x",
        ".GRAPHQL-ORM-Storage/x",
        ".graphql-orm-storage./x",
    ] {
        let refused = |result: Result<(), StorageError>| {
            assert!(
                matches!(result, Err(StorageError::InvalidStorageKey { .. })),
                "key {key:?} must be refused: {result:?}"
            );
        };
        refused(
            store
                .put_blob(
                    key,
                    StorageByteStream::from_bytes(Bytes::from_static(b"reserved")),
                    BlobPutOptions::default(),
                )
                .await
                .map(drop),
        );
        refused(store.get_blob(key).await.map(drop));
        refused(store.blob_exists(key).await.map(drop));
        refused(store.head_blob(key).await.map(drop));
        refused(store.delete_blob(key).await);
        refused(store.copy_blob(key, "copied").await);
        refused(store.list_blobs(key).await.map(drop));
    }
}

async fn drain_pages(store: &dyn BlobStore, prefix: &str, limit: usize) -> Vec<String> {
    let mut keys = Vec::new();
    let mut continuation = None;
    loop {
        let page = store
            .list_blobs_page(prefix, continuation, limit)
            .await
            .expect("list page");
        assert!(page.keys.len() <= limit, "page exceeds its limit");
        keys.extend(page.keys);
        continuation = page.next_continuation;
        if continuation.is_none() {
            return keys;
        }
    }
}
