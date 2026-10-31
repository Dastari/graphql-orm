//! Resumable multipart uploads addressed by a persisted, credential-free handle.
//!
//! An upload is started for one validated blob key and receives numbered parts,
//! possibly from different requests or processes. The object becomes visible
//! only when [`MultipartBlobStore::complete_multipart`] succeeds with receipts
//! that exactly match the stored parts. Until then the parts can be listed,
//! replaced, completed or aborted through the same handle.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{BlobPutOptions, BlobStore, StorageBackend, StorageByteStream, StorageError};

/// Highest part number a multipart upload accepts. Part numbers start at 1.
pub const MULTIPART_MAX_PART_NUMBER: u32 = 10_000;

/// Options for starting a multipart upload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultipartStartOptions {
    /// Provider write options for the completed object.
    pub put: BlobPutOptions,
    /// Complete only if no object exists at the key at completion time.
    ///
    /// The check is atomic with publishing the object. A conflict fails
    /// completion with [`StorageError::PreconditionFailed`] and leaves the parts
    /// in place.
    pub if_not_exists: bool,
}

/// Persistable identity of one multipart upload.
///
/// It carries no credentials, endpoint or provider configuration: continuing an
/// upload requires a backend configured for the same storage. The serialized
/// form is stable across patch releases.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultipartUploadHandle {
    /// Backend that owns the upload.
    pub backend: StorageBackend,
    /// Validated blob key the completed object is written to.
    pub key: String,
    /// Provider upload identifier.
    pub upload_id: String,
    /// The upload was started with
    /// [`if_not_exists`](MultipartStartOptions::if_not_exists). Omitted from
    /// the wire when false. S3 keeps no per-upload condition, so completion
    /// on S3 uses this flag; local storage also records it with the upload.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub if_not_exists: bool,
}

/// One stored part, as returned by upload and listing and accepted by completion.
///
/// The serialized form is stable across patch releases.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartReceipt {
    /// Part number, from 1 to [`MULTIPART_MAX_PART_NUMBER`].
    pub part_number: u32,
    /// Exact part length in bytes.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the part bytes. Empty when a provider listing
    /// does not report a checksum for a part it stored.
    pub sha256: String,
    /// Provider part identity: the S3 ETag, or the SHA-256 for local storage.
    pub provider_tag: String,
}

/// The object a successful completion published.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MultipartCompleteOutcome {
    /// Object length in bytes: the total of the completed parts.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the whole object, when the backend computed it
    /// while assembling the object. Local storage always reports it. S3 reports
    /// none: its integrity rests on the per-part SHA-256 checksums the provider
    /// verified and on the exact receipt list. Use
    /// [`BlobStore::compute_blob_sha256`] to hash the object later.
    pub sha256_hex: Option<String>,
}

/// Part size and count limits of one backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MultipartLimits {
    /// Smallest accepted size of every part except the last.
    pub min_part_size: u64,
    /// Largest accepted part size.
    pub max_part_size: u64,
    /// Largest number of parts in one upload.
    pub max_parts: u32,
}

impl MultipartLimits {
    /// Limits with the given bounds.
    #[must_use]
    pub const fn new(min_part_size: u64, max_part_size: u64, max_parts: u32) -> Self {
        Self {
            min_part_size,
            max_part_size,
            max_parts,
        }
    }
}

/// One upload that has been started but neither completed nor aborted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IncompleteMultipartUpload {
    /// Handle that continues, completes or aborts the upload.
    pub handle: MultipartUploadHandle,
    /// When the upload was started, if the provider reports it.
    pub started_at: Option<OffsetDateTime>,
}

/// One page of incomplete uploads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IncompleteMultipartPage {
    /// Uploads in this page, ordered by key and then upload identifier.
    pub uploads: Vec<IncompleteMultipartUpload>,
    /// Opaque continuation for the next page, if any.
    pub next_continuation: Option<String>,
}

/// Why a multipart operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MultipartErrorKind {
    /// The upload is unknown, already completed or aborted, or expired.
    UnknownUpload,
    /// The handle belongs to another backend.
    BackendMismatch,
    /// The part number is outside 1 to [`MULTIPART_MAX_PART_NUMBER`] or the
    /// backend's part limit.
    PartNumberOutOfRange,
    /// A part other than the last is smaller than the backend's minimum.
    PartTooSmall,
    /// A part is larger than the backend's maximum.
    PartTooLarge,
    /// The receipts are empty, out of order, or differ from the stored parts.
    ReceiptMismatch,
}

impl MultipartErrorKind {
    /// Stable snake_case identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownUpload => "unknown_upload",
            Self::BackendMismatch => "backend_mismatch",
            Self::PartNumberOutOfRange => "part_number_out_of_range",
            Self::PartTooSmall => "part_too_small",
            Self::PartTooLarge => "part_too_large",
            Self::ReceiptMismatch => "receipt_mismatch",
        }
    }
}

impl std::fmt::Display for MultipartErrorKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Resumable multipart uploads.
///
/// Implemented by the local and S3 backends. For other providers
/// [`BlobStore::as_multipart`] returns none and
/// [`StorageService::multipart`](crate::StorageService::multipart) returns
/// [`StorageError::UnsupportedOperation`].
///
/// One upload, resumed by a second backend instance from the persisted handle,
/// as after a restart:
///
/// ```
/// # #[cfg(feature = "local")]
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use graphql_orm_storage::{
///     BlobStore, LocalStorageBackend, MultipartBlobStore, MultipartStartOptions,
///     MultipartUploadHandle, StorageByteStream,
/// };
///
/// let root = tempfile::tempdir()?;
/// let saved = {
///     let first = LocalStorageBackend::new(root.path());
///     let handle = first
///         .start_multipart("scans/archive.tar", MultipartStartOptions::default())
///         .await?;
///     first
///         .upload_part(&handle, 1, StorageByteStream::from_bytes(&b"first part, "[..]))
///         .await?;
///     serde_json::to_string(&handle)?
/// };
///
/// let second = LocalStorageBackend::new(root.path());
/// let handle: MultipartUploadHandle = serde_json::from_str(&saved)?;
/// second
///     .upload_part(&handle, 2, StorageByteStream::from_bytes(&b"last part"[..]))
///     .await?;
/// let parts = second.list_parts(&handle).await?;
/// let outcome = second.complete_multipart(&handle, &parts).await?;
/// assert_eq!(outcome.size_bytes, 21);
/// assert!(second.blob_exists("scans/archive.tar").await?);
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// # }).unwrap();
/// ```
#[async_trait]
pub trait MultipartBlobStore: BlobStore {
    /// Part size and count limits of this backend.
    fn multipart_limits(&self) -> MultipartLimits;

    /// Starts an upload for a validated key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// start the upload.
    async fn start_multipart(
        &self,
        key: &str,
        options: MultipartStartOptions,
    ) -> Result<MultipartUploadHandle, StorageError>;

    /// Stores one part, replacing any earlier part with the same number.
    ///
    /// The body is streamed and refused as soon as it exceeds
    /// [`MultipartLimits::max_part_size`]. Concurrent uploads of one part number
    /// leave exactly one of them stored, never an interleaving.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Multipart`] for an unknown upload, an invalid
    /// part number or an oversized part, the body stream's error, or a
    /// provider error.
    async fn upload_part(
        &self,
        handle: &MultipartUploadHandle,
        part_number: u32,
        body: StorageByteStream,
    ) -> Result<PartReceipt, StorageError>;

    /// Lists the stored parts in ascending part-number order.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Multipart`] for an unknown upload, or a provider
    /// error.
    async fn list_parts(
        &self,
        handle: &MultipartUploadHandle,
    ) -> Result<Vec<PartReceipt>, StorageError>;

    /// Assembles the object from exactly the given parts and publishes it
    /// atomically.
    ///
    /// The receipts must equal the stored parts, in ascending part-number
    /// order. The object is never visible before this succeeds. On any error
    /// the parts stay in place, so the caller can retry or abort; a completion
    /// that races [`abort_multipart`](Self::abort_multipart) either publishes
    /// the complete object or fails with [`MultipartErrorKind::UnknownUpload`].
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Multipart`] for an unknown upload, mismatched
    /// receipts or a too-small part, [`StorageError::PreconditionFailed`] when
    /// `if_not_exists` was requested and an object exists, or a provider error.
    async fn complete_multipart(
        &self,
        handle: &MultipartUploadHandle,
        parts: &[PartReceipt],
    ) -> Result<MultipartCompleteOutcome, StorageError>;

    /// Removes every stored part. Succeeds for an upload that is unknown,
    /// already aborted, completed or expired.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for a handle of another backend or a provider
    /// error.
    async fn abort_multipart(&self, handle: &MultipartUploadHandle) -> Result<(), StorageError>;

    /// Lists incomplete uploads whose key starts with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the prefix or continuation is invalid or
    /// the provider cannot list uploads.
    async fn list_incomplete_multipart(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<IncompleteMultipartPage, StorageError>;
}

#[cfg(any(feature = "local", feature = "s3"))]
pub(crate) fn multipart_error(key: &str, kind: MultipartErrorKind) -> StorageError {
    StorageError::Multipart {
        key: key.to_string(),
        kind,
    }
}

#[cfg(any(feature = "local", feature = "s3"))]
/// Validate a handle against the serving backend and its key rules.
pub(crate) fn check_handle(
    handle: &MultipartUploadHandle,
    backend: StorageBackend,
) -> Result<(), StorageError> {
    crate::validate_blob_key(&handle.key)?;
    if handle.backend != backend {
        return Err(multipart_error(
            &handle.key,
            MultipartErrorKind::BackendMismatch,
        ));
    }
    if handle.upload_id.is_empty() || handle.upload_id.len() > 1024 {
        return Err(multipart_error(
            &handle.key,
            MultipartErrorKind::UnknownUpload,
        ));
    }
    Ok(())
}

#[cfg(any(feature = "local", feature = "s3"))]
pub(crate) fn check_part_number(
    key: &str,
    part_number: u32,
    limits: MultipartLimits,
) -> Result<(), StorageError> {
    if part_number == 0 || part_number > MULTIPART_MAX_PART_NUMBER || part_number > limits.max_parts
    {
        return Err(multipart_error(
            key,
            MultipartErrorKind::PartNumberOutOfRange,
        ));
    }
    Ok(())
}

#[cfg(any(feature = "local", feature = "s3"))]
/// Require the caller's receipts to name exactly the stored parts.
///
/// A stored checksum that a provider listing did not report (empty) is not
/// compared; sizes and provider tags always are.
pub(crate) fn verify_receipts(
    key: &str,
    stored: &[PartReceipt],
    given: &[PartReceipt],
    limits: MultipartLimits,
) -> Result<(), StorageError> {
    let mismatch = || multipart_error(key, MultipartErrorKind::ReceiptMismatch);
    if given.is_empty() || given.len() != stored.len() {
        return Err(mismatch());
    }
    // A caller's digest is either absent or exactly one SHA-256 in lowercase
    // hex. It may be sent to the provider unchecked when a listing reports no
    // checksum, so anything else is refused here.
    if given
        .iter()
        .any(|part| !(part.sha256.is_empty() || is_sha256_hex(&part.sha256)))
    {
        return Err(mismatch());
    }
    if given
        .windows(2)
        .any(|pair| pair[0].part_number >= pair[1].part_number)
    {
        return Err(mismatch());
    }
    for (given, stored) in given.iter().zip(stored) {
        let checksum_matches = stored.sha256.is_empty() || given.sha256 == stored.sha256;
        if given.part_number != stored.part_number
            || given.size_bytes != stored.size_bytes
            || given.provider_tag != stored.provider_tag
            || !checksum_matches
        {
            return Err(mismatch());
        }
    }
    let last = given.len() - 1;
    if given[..last]
        .iter()
        .any(|part| part.size_bytes < limits.min_part_size)
    {
        return Err(multipart_error(key, MultipartErrorKind::PartTooSmall));
    }
    Ok(())
}

#[cfg(any(feature = "local", feature = "s3"))]
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(all(test, any(feature = "local", feature = "s3")))]
mod tests {
    use super::*;

    fn receipt(part_number: u32, size_bytes: u64) -> PartReceipt {
        PartReceipt {
            part_number,
            size_bytes,
            sha256: format!("{part_number:064x}"),
            provider_tag: format!("tag-{part_number}"),
        }
    }

    #[test]
    fn receipts_must_exactly_match_the_stored_parts() {
        let limits = MultipartLimits::new(5, 100, 10);
        let stored = [receipt(1, 5), receipt(2, 5), receipt(3, 1)];
        verify_receipts("k", &stored, &stored, limits).unwrap();

        let kind = |given: &[PartReceipt]| match verify_receipts("k", &stored, given, limits) {
            Err(StorageError::Multipart { kind, .. }) => kind,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(kind(&[]), MultipartErrorKind::ReceiptMismatch);
        assert_eq!(kind(&stored[..2]), MultipartErrorKind::ReceiptMismatch);
        let reordered = [stored[1].clone(), stored[0].clone(), stored[2].clone()];
        assert_eq!(kind(&reordered), MultipartErrorKind::ReceiptMismatch);
        let mut changed = stored.clone();
        changed[1].provider_tag = "other".into();
        assert_eq!(kind(&changed), MultipartErrorKind::ReceiptMismatch);
        let mut resized = stored.clone();
        resized[2].size_bytes = 2;
        assert_eq!(kind(&resized), MultipartErrorKind::ReceiptMismatch);

        let small = [receipt(1, 4), receipt(2, 1)];
        assert_eq!(
            match verify_receipts("k", &small, &small, limits) {
                Err(StorageError::Multipart { kind, .. }) => kind,
                other => panic!("unexpected {other:?}"),
            },
            MultipartErrorKind::PartTooSmall
        );

        // A provider listing without checksums still compares sizes and tags.
        let mut unlisted = stored.clone();
        unlisted.iter_mut().for_each(|part| part.sha256.clear());
        verify_receipts("k", &unlisted, &stored, limits).unwrap();
    }

    #[test]
    fn malformed_receipt_digests_are_mismatches_not_panics() {
        let limits = MultipartLimits::new(1, 100, 10);
        let stored = [PartReceipt {
            part_number: 1,
            size_bytes: 5,
            sha256: String::new(),
            provider_tag: "tag".into(),
        }];
        let malformed = [
            "abc".to_string(),
            "é".repeat(32),
            "A".repeat(64),
            "g".repeat(64),
            "a".repeat(63),
        ];
        for sha256 in malformed {
            let given = [PartReceipt {
                sha256,
                ..stored[0].clone()
            }];
            assert!(matches!(
                verify_receipts("k", &stored, &given, limits),
                Err(StorageError::Multipart {
                    kind: MultipartErrorKind::ReceiptMismatch,
                    ..
                })
            ));
        }
        let given = [PartReceipt {
            sha256: "a".repeat(64),
            ..stored[0].clone()
        }];
        verify_receipts("k", &stored, &given, limits).unwrap();
    }

    #[test]
    fn handle_and_receipt_wire_forms_are_stable() {
        let handle = MultipartUploadHandle {
            backend: StorageBackend::S3,
            key: "videos/a.mp4".into(),
            upload_id: "abc".into(),
            if_not_exists: false,
        };
        assert_eq!(
            serde_json::to_value(&handle).unwrap(),
            serde_json::json!({"backend": "S3", "key": "videos/a.mp4", "upload_id": "abc"})
        );
        let conditional = MultipartUploadHandle {
            if_not_exists: true,
            ..handle.clone()
        };
        assert_eq!(
            serde_json::to_value(&conditional).unwrap()["if_not_exists"],
            serde_json::json!(true)
        );
        let receipt = receipt(1, 5);
        assert_eq!(
            serde_json::to_value(&receipt).unwrap(),
            serde_json::json!({"part_number": 1, "size_bytes": 5, "sha256": format!("{:064x}", 1), "provider_tag": "tag-1"})
        );
        assert!(
            serde_json::from_value::<MultipartUploadHandle>(serde_json::json!({
                "backend": "S3", "key": "k", "upload_id": "u", "secret": "x"
            }))
            .is_err()
        );
    }
}
