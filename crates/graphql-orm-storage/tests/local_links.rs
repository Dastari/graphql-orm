#![cfg(all(feature = "local", unix))]
//! Local storage never follows a filesystem link below its root.
//!
//! Every key-addressed read, write, copy and delete refuses a key whose path
//! below the root names or passes through a symbolic link, wherever the link
//! points, with `StorageError::InvalidStorageKey`. Listings neither list nor
//! enter links. Nothing outside the root is read, created, changed or
//! removed.

use std::{
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    time::Duration,
};

use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, LocalStorageBackend, MultipartBlobStore, MultipartStartOptions,
    ObjectMetadata, StorageByteStream, StorageError, StreamingObjectStore,
};
use tempfile::TempDir;

const SECRET: &[u8] = b"outside the storage root";

struct Fixture {
    _owned: TempDir,
    root: PathBuf,
    outside: PathBuf,
    backend: LocalStorageBackend,
}

impl Fixture {
    /// A root holding `real/file`, and links that point out of the root
    /// (`linked-dir`, `linked-file`, `dangling`) and inside it (`inner-link`).
    async fn new() -> Self {
        let owned = TempDir::new().expect("owned fixture");
        let root = owned.path().join("storage");
        let outside = owned.path().join("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(outside.join("dir")).expect("outside");
        std::fs::write(outside.join("secret"), SECRET).expect("outside file");
        let backend = LocalStorageBackend::new(&root);
        backend
            .put_blob("real/file", body(b"inside"), BlobPutOptions::default())
            .await
            .expect("put an ordinary key");
        symlink(&outside, root.join("linked-dir")).expect("directory link");
        symlink(outside.join("secret"), root.join("linked-file")).expect("file link");
        symlink(outside.join("missing"), root.join("dangling")).expect("dangling link");
        symlink(root.join("real/file"), root.join("inner-link")).expect("inner link");
        symlink(root.join("real"), root.join("inner-dir")).expect("inner directory link");
        Self {
            _owned: owned,
            root,
            outside,
            backend,
        }
    }

    /// The outside tree still holds exactly the secret file and the empty
    /// directory it started with.
    fn assert_outside_untouched(&self) {
        let mut names: Vec<_> = std::fs::read_dir(&self.outside)
            .expect("outside")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["dir", "secret"], "nothing created outside the root");
        assert_eq!(
            std::fs::read_dir(self.outside.join("dir"))
                .expect("outside dir")
                .count(),
            0
        );
        assert_eq!(
            std::fs::read(self.outside.join("secret")).expect("secret"),
            SECRET
        );
    }

    fn assert_link(&self, name: &str) {
        assert!(
            std::fs::symlink_metadata(self.root.join(name))
                .expect("link kept")
                .file_type()
                .is_symlink(),
            "{name} must stay a link"
        );
    }
}

fn body(bytes: &'static [u8]) -> StorageByteStream {
    StorageByteStream::from_bytes(Bytes::from_static(bytes))
}

fn assert_refused<T: std::fmt::Debug>(result: Result<T, StorageError>, what: &str) {
    assert!(
        matches!(result, Err(StorageError::InvalidStorageKey { .. })),
        "{what} must be refused with InvalidStorageKey: {result:?}"
    );
}

/// Keys whose path names a link, or passes through one.
const LINKED_KEYS: [&str; 7] = [
    "linked-file",
    "linked-dir/secret",
    "linked-dir/dir/new",
    "dangling",
    "inner-link",
    "inner-dir/file",
    "inner-dir/new",
];

#[tokio::test]
async fn local_reads_refuse_keys_through_links() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    for key in LINKED_KEYS {
        assert_refused(backend.get_blob(key).await.map(|_| ()), key);
        assert_refused(backend.get_blob_range(key, 0..4).await.map(|_| ()), key);
        assert_refused(backend.head_blob(key).await, key);
        assert_refused(backend.blob_exists(key).await, key);
    }
    // The ordinary key the inner links point at still reads.
    let blob = backend.get_blob("real/file").await.expect("ordinary read");
    assert_eq!(
        graphql_orm_storage::collect_storage_stream(blob.body)
            .await
            .expect("bytes")
            .as_ref(),
        b"inside"
    );
    fixture.assert_outside_untouched();
}

#[tokio::test]
async fn local_writes_refuse_keys_through_links() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    for key in LINKED_KEYS
        .into_iter()
        .chain(["linked-dir/new", "linked-dir/deep/new"])
    {
        assert_refused(
            backend
                .put_blob(key, body(b"written"), BlobPutOptions::default())
                .await,
            key,
        );
        assert_refused(
            backend
                .put_blob_if_not_exists(key, body(b"written"), BlobPutOptions::default())
                .await,
            key,
        );
        assert_refused(backend.copy_blob("real/file", key).await, key);
        assert_refused(backend.copy_blob(key, "copies/out").await, key);
    }
    assert!(
        !fixture.root.join("copies/out").exists(),
        "a copy from a link wrote nothing"
    );
    fixture.assert_outside_untouched();
    for name in [
        "linked-dir",
        "linked-file",
        "dangling",
        "inner-link",
        "inner-dir",
    ] {
        fixture.assert_link(name);
    }
    assert_eq!(
        std::fs::read(fixture.root.join("real/file")).expect("inside"),
        b"inside",
        "a write through an inner link changed nothing"
    );
}

#[tokio::test]
async fn local_deletes_refuse_keys_through_links() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    for key in LINKED_KEYS {
        assert_refused(backend.delete_blob(key).await, key);
        assert_refused(backend.delete_blob_once(key).await, key);
    }
    fixture.assert_outside_untouched();
    for name in [
        "linked-dir",
        "linked-file",
        "dangling",
        "inner-link",
        "inner-dir",
    ] {
        fixture.assert_link(name);
    }
    assert!(fixture.root.join("real/file").exists());
}

#[tokio::test]
async fn local_listings_never_follow_links() {
    let fixture = Fixture::new().await;
    symlink(&fixture.root, fixture.root.join("cycle")).expect("cycle");
    let backend = &fixture.backend;
    let listed = tokio::time::timeout(Duration::from_secs(10), backend.list_blobs(""))
        .await
        .expect("a cycle must not cause unbounded traversal")
        .expect("list");
    assert_eq!(
        listed,
        ["real/file"],
        "links are neither listed nor entered"
    );
    assert_eq!(
        backend.list_blobs("linked").await.expect("string prefix"),
        Vec::<String>::new()
    );
    for prefix in ["linked-dir/", "linked-dir/dir/", "inner-dir/", "cycle/real"] {
        assert_refused(backend.list_blobs(prefix).await, prefix);
    }
}

#[tokio::test]
async fn local_streaming_objects_refuse_links() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    backend
        .put_object_stream(
            "bucket",
            "kept",
            None,
            ObjectMetadata::default(),
            body(b"object"),
        )
        .await
        .expect("ordinary object");
    symlink(&fixture.outside, fixture.root.join("linked-bucket")).expect("bucket link");
    symlink(&fixture.outside, fixture.root.join("bucket/sub")).expect("key link");

    for (bucket, key) in [("linked-bucket", "new"), ("bucket", "sub/new")] {
        assert_refused(
            backend
                .put_object_stream(bucket, key, None, ObjectMetadata::default(), body(b"x"))
                .await,
            key,
        );
        assert_refused(backend.delete_object(bucket, key).await, key);
    }
    assert_refused(
        backend.get_object_range("bucket", "sub/secret", 0..4).await,
        "object through a link",
    );

    // An object file replaced by a link is not served.
    std::fs::remove_file(fixture.root.join("bucket/kept")).expect("remove object");
    symlink(
        fixture.outside.join("secret"),
        fixture.root.join("bucket/kept"),
    )
    .expect("swap");
    assert_refused(
        backend.get_object_range("bucket", "kept", 0..4).await,
        "linked object file",
    );
    assert!(
        backend
            .list_objects("bucket", "")
            .await
            .expect("list objects")
            .is_empty(),
        "a linked object file is not listed"
    );

    // Object metadata reached through a linked internal directory is refused.
    let metadata_bucket = fixture
        .root
        .join(".graphql-orm-storage/object-metadata/other");
    std::fs::create_dir_all(metadata_bucket.parent().expect("parent")).expect("metadata dir");
    symlink(&fixture.outside, &metadata_bucket).expect("metadata link");
    assert_refused(
        backend
            .put_object_stream("other", "new", None, ObjectMetadata::default(), body(b"x"))
            .await,
        "metadata through a link",
    );
    assert_refused(
        backend.get_object_metadata("other", "secret").await,
        "metadata read through a link",
    );
    fixture.assert_outside_untouched();
}

#[tokio::test]
async fn local_multipart_uploads_refuse_links() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    assert_refused(
        backend
            .start_multipart("linked-dir/upload", MultipartStartOptions::default())
            .await,
        "start through a link",
    );

    // A target directory replaced by a link before completion is refused.
    let handle = backend
        .start_multipart("later/upload", MultipartStartOptions::default())
        .await
        .expect("start");
    let receipt = backend
        .upload_part(&handle, 1, body(b"part"))
        .await
        .expect("part");
    std::fs::create_dir_all(fixture.root.join("later")).ok();
    std::fs::remove_dir(fixture.root.join("later")).expect("remove target dir");
    symlink(&fixture.outside, fixture.root.join("later")).expect("target link");
    assert_refused(
        backend.complete_multipart(&handle, &[receipt]).await,
        "completion through a link",
    );
    fixture.assert_outside_untouched();

    // A staging directory replaced by a link is refused too.
    let internal = fixture.root.join(".graphql-orm-storage/multipart");
    std::fs::remove_dir_all(&internal).expect("remove staging");
    symlink(fixture.outside.join("dir"), &internal).expect("staging link");
    assert_refused(
        backend
            .start_multipart("real/upload", MultipartStartOptions::default())
            .await,
        "staging through a link",
    );
    assert_refused(
        backend.upload_part(&handle, 2, body(b"part")).await,
        "part through a linked staging directory",
    );
    fixture.assert_outside_untouched();
}

/// The configured root itself may be a link: the host chose it. Only links
/// below it are refused.
#[tokio::test]
async fn a_linked_root_is_the_hosts_choice() {
    let owned = TempDir::new().expect("owned fixture");
    let real = owned.path().join("real-root");
    std::fs::create_dir_all(&real).expect("real root");
    let linked: &Path = &owned.path().join("linked-root");
    symlink(&real, linked).expect("root link");
    let backend = LocalStorageBackend::new(linked);
    backend
        .put_blob("a/b", body(b"x"), BlobPutOptions::default())
        .await
        .expect("write below a linked root");
    assert_eq!(backend.list_blobs("").await.expect("list"), ["a/b"]);
    backend.delete_blob("a/b").await.expect("delete");
    assert!(!real.join("a/b").exists());
}

/// Completion reads staged parts without following links: a part replaced
/// by a link to an outside file with the same bytes, so its digest would
/// still match, is refused and nothing is published.
#[tokio::test]
async fn local_multipart_completion_refuses_a_linked_part() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    let handle = backend
        .start_multipart("uploads/object", MultipartStartOptions::default())
        .await
        .expect("start");
    let receipt = backend
        .upload_part(&handle, 1, body(SECRET))
        .await
        .expect("part");
    let part = staged_part(&fixture.root, &handle.upload_id);
    std::fs::remove_file(&part).expect("remove part");
    symlink(fixture.outside.join("secret"), &part).expect("part link");

    assert_refused(
        backend.complete_multipart(&handle, &[receipt]).await,
        "completion from a linked part",
    );
    assert!(!fixture.root.join("uploads/object").exists());
    fixture.assert_outside_untouched();
}

/// The data file of the upload's only staged part.
fn staged_part(root: &Path, upload_id: &str) -> PathBuf {
    let dir = root.join(".graphql-orm-storage/multipart").join(upload_id);
    std::fs::read_dir(&dir)
        .expect("upload dir")
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "data")
        })
        .expect("a staged part")
}

/// A regular file that shares its data with another hard link, such as an
/// outside file linked into the root, is never read.
#[tokio::test]
async fn local_reads_refuse_hard_linked_files() {
    let fixture = Fixture::new().await;
    let backend = &fixture.backend;
    std::fs::hard_link(
        fixture.outside.join("secret"),
        fixture.root.join("real/hard"),
    )
    .expect("hard link");

    let key = "real/hard";
    assert_refused(backend.get_blob(key).await.map(|_| ()), key);
    assert_refused(backend.get_blob_range(key, 0..4).await.map(|_| ()), key);
    assert_refused(backend.head_blob(key).await, key);
    assert_refused(backend.blob_exists(key).await, key);
    assert_refused(backend.copy_blob(key, "copies/out").await, key);
    assert!(!fixture.root.join("copies/out").exists());

    // A staged part hard-linked to an outside file is refused too.
    let handle = backend
        .start_multipart("uploads/hard", MultipartStartOptions::default())
        .await
        .expect("start");
    let receipt = backend
        .upload_part(&handle, 1, body(SECRET))
        .await
        .expect("part");
    let part = staged_part(&fixture.root, &handle.upload_id);
    std::fs::remove_file(&part).expect("remove part");
    std::fs::hard_link(fixture.outside.join("secret"), &part).expect("part hard link");
    assert_refused(
        backend.complete_multipart(&handle, &[receipt]).await,
        "completion from a hard-linked part",
    );
    assert!(!fixture.root.join("uploads/hard").exists());

    // Deleting the key removes only the link inside the root.
    backend.delete_blob(key).await.expect("delete the link");
    assert!(!fixture.root.join(key).exists());
    fixture.assert_outside_untouched();

    // A file this backend wrote has one link and reads normally, including
    // one published by a conditional create.
    backend
        .put_blob_if_not_exists("real/conditional", body(b"x"), BlobPutOptions::default())
        .await
        .expect("conditional create")
        .expect("created");
    backend
        .get_blob("real/conditional")
        .await
        .expect("a conditional create leaves one link");
}

/// Canonical keys apply when an upload completes, not only when it starts:
/// an upload started before the mode was enabled cannot publish a key the
/// mode refuses.
#[tokio::test]
async fn local_canonical_keys_refuse_a_resumed_completion() {
    let owned = TempDir::new().expect("temp dir");
    let ordinary = LocalStorageBackend::new(owned.path());
    let handle = ordinary
        .start_multipart("Uploads/Object", MultipartStartOptions::default())
        .await
        .expect("start without the mode");
    let receipt = ordinary
        .upload_part(&handle, 1, body(b"part"))
        .await
        .expect("part");

    let canonical = LocalStorageBackend::new(owned.path()).with_canonical_keys(true);
    assert_refused(
        canonical
            .complete_multipart(&handle, std::slice::from_ref(&receipt))
            .await,
        "a non-canonical completion",
    );
    assert!(!owned.path().join("Uploads/Object").exists());

    // The staged parts are intact: the upload can still complete without the
    // mode, or be aborted.
    ordinary
        .complete_multipart(&handle, &[receipt])
        .await
        .expect("the ordinary backend completes it");
}
