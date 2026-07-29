use std::str::FromStr;

use graphql_orm_storage::{
    StorageBackend, StorageError, StorageNamespace, build_storage_key, file_extension, sha256_hex,
    unsupported_backend,
};
use uuid::Uuid;

#[test]
fn checksum_matches_known_sha256() {
    assert_eq!(
        sha256_hex(b"hello storage"),
        "ada7ad17eeff1826bdf1e69d6a70d542548a6f0a3c3809748a36076d97671047"
    );
}

#[test]
fn generated_key_uses_namespace_uuid_shards_and_lowercase_extension() {
    let object_id = Uuid::parse_str("6c57a6cc-09e6-4a7f-a320-2f2bde4cfd86").expect("valid uuid");
    let key = build_storage_key(StorageNamespace::Originals, &object_id, Some("JPG"));
    assert_eq!(
        key,
        "originals/6c/57/6c57a6cc-09e6-4a7f-a320-2f2bde4cfd86.jpg"
    );
}

#[test]
fn generated_key_without_extension_has_no_extension_suffix() {
    let object_id = Uuid::parse_str("6c57a6cc-09e6-4a7f-a320-2f2bde4cfd86").expect("valid uuid");
    let key = build_storage_key(StorageNamespace::Originals, &object_id, None);
    assert_eq!(key, "originals/6c/57/6c57a6cc-09e6-4a7f-a320-2f2bde4cfd86");
}

#[test]
fn extension_parser_rejects_unsafe_extension_candidates() {
    assert_eq!(file_extension("artifact.jpg"), Some("jpg"));
    assert_eq!(file_extension("artifact."), None);
    assert_eq!(file_extension("artifact.jp/g"), None);
    assert_eq!(file_extension("artifact.jp\\g"), None);
    assert_eq!(file_extension("artifact.jp\0g"), None);
}

#[test]
fn backend_string_representations_are_stable() {
    assert_eq!(StorageBackend::Local.as_str(), "local");
    assert_eq!(StorageBackend::S3.as_str(), "s3");
    assert_eq!(StorageBackend::AzureBlob.as_str(), "azure_blob");
}

#[test]
fn namespace_string_representations_are_stable() {
    assert_eq!(StorageNamespace::Originals.as_str(), "originals");
    assert_eq!(StorageNamespace::RecycleBin.as_str(), "recycle_bin");
    assert_eq!(StorageNamespace::Thumbnails.as_str(), "thumbnails");
    assert_eq!(StorageNamespace::Derivatives.as_str(), "derivatives");
    assert_eq!(StorageNamespace::Exports.as_str(), "exports");
    assert_eq!(StorageNamespace::Temp.as_str(), "temp");
}

#[test]
fn parses_known_backend_names() {
    assert_eq!(
        StorageBackend::from_str("local").expect("local"),
        StorageBackend::Local
    );
    assert_eq!(
        StorageBackend::from_str("s3-compatible").expect("s3"),
        StorageBackend::S3
    );
    assert_eq!(
        StorageBackend::from_str("azure-blob").expect("azure"),
        StorageBackend::AzureBlob
    );
}

#[test]
fn invalid_backend_names_return_unsupported_backend_error() {
    let err = StorageBackend::from_str("dropbox").expect_err("invalid backend");

    assert!(matches!(
        err,
        StorageError::UnsupportedBackend { backend } if backend == "dropbox"
    ));
}

#[test]
fn unsupported_backend_uses_stable_backend_name() {
    let err = unsupported_backend(StorageBackend::S3);

    assert!(matches!(
        err,
        StorageError::UnsupportedBackend { backend } if backend == "s3"
    ));
}

#[test]
fn retryability_is_explicit_for_provider_and_permanent_errors() {
    let retryable = StorageError::Provider {
        backend: "s3".to_string(),
        message: "timeout".to_string(),
        retryable: true,
    };
    let permanent = StorageError::PreconditionFailed {
        key: "objects/test".to_string(),
        condition: "already exists".to_string(),
    };

    assert!(retryable.is_retryable());
    assert!(!permanent.is_retryable());
}
