//! S3 multipart uploads through the native multipart calls.
//!
//! Parts carry an `x-amz-checksum-sha256`, so the provider verifies each part's
//! bytes. Completion sends the caller's receipts after checking them against
//! `ListParts`, with one SDK attempt; a failed completion leaves the upload and
//! its parts in place.

use async_trait::async_trait;
use aws_sdk_s3::{
    config::{Builder as S3ConfigBuilder, retry::RetryConfig},
    error::{ProvideErrorMetadata, SdkError},
    primitives::ByteStream,
    types::{ChecksumAlgorithm, CompletedMultipartUpload, CompletedPart},
};
use base64::Engine;
use bytes::BytesMut;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::multipart::{check_handle, check_part_number, multipart_error, verify_receipts};
use crate::s3::{S3StorageBackend, map_s3_error};
use crate::{
    IncompleteMultipartPage, IncompleteMultipartUpload, MultipartBlobStore,
    MultipartCompleteOutcome, MultipartErrorKind, MultipartLimits, MultipartStartOptions,
    MultipartUploadHandle, PartReceipt, StorageBackend, StorageByteStream, StorageError,
    validate_blob_key,
};

/// S3's smallest part, except the last.
pub(crate) const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;
/// S3's largest part.
pub(crate) const MAX_PART_SIZE: u64 = 5 * 1024 * 1024 * 1024;
/// Largest part accepted by default; it is buffered once per in-flight part.
pub(crate) const DEFAULT_MAX_PART_SIZE: u64 = 64 * 1024 * 1024;

/// Decode lowercase or uppercase hex, or none for any other input.
fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// Base64 of a hex SHA-256. Receipts are validated before they get here, so a
/// malformed digest only ever yields none.
fn sha256_base64(hex: &str) -> Option<String> {
    hex_bytes(hex)
        .filter(|bytes| bytes.len() == 32)
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The ETag an MD5-ETag provider gives the object assembled from `parts`:
/// the MD5 of the concatenated part MD5s, then `-` and the part count. None when
/// a part tag is not an MD5.
fn multipart_etag(parts: &[PartReceipt]) -> Option<String> {
    let mut digests = Vec::with_capacity(parts.len() * 16);
    for part in parts {
        let md5 =
            hex_bytes(part.provider_tag.trim_matches('"')).filter(|bytes| bytes.len() == 16)?;
        digests.extend(md5);
    }
    let digest = md5::Md5::digest(&digests);
    Some(format!("\"{digest:x}-{}\"", parts.len()))
}

fn sha256_hex_from_base64(encoded: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    (bytes.len() == 32).then(|| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Map a provider refusal of a known upload condition to a typed error, and
/// anything else to the backend's ordinary provider error.
fn multipart_s3_error<E>(key: &str, error: SdkError<E>) -> StorageError
where
    E: ProvideErrorMetadata + std::fmt::Debug,
{
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code);
    let status = error
        .raw_response()
        .map(|response| response.status().as_u16());
    match (code, status) {
        // Only the upload itself being gone is an unknown upload. Any other 404,
        // such as NoSuchBucket or a wrong endpoint, is a provider error, so an
        // abort against misconfigured storage fails instead of "succeeding".
        (Some("NoSuchUpload"), _) => multipart_error(key, MultipartErrorKind::UnknownUpload),
        (Some("EntityTooSmall"), _) => multipart_error(key, MultipartErrorKind::PartTooSmall),
        (Some("EntityTooLarge"), _) => multipart_error(key, MultipartErrorKind::PartTooLarge),
        (Some("InvalidPart" | "InvalidPartOrder" | "BadDigest" | "InvalidDigest"), _) => {
            multipart_error(key, MultipartErrorKind::ReceiptMismatch)
        }
        (Some("PreconditionFailed"), _) | (_, Some(412)) => StorageError::PreconditionFailed {
            key: key.to_string(),
            condition: "object already exists".to_string(),
        },
        _ => map_s3_error(error),
    }
}

impl S3StorageBackend {
    /// A completion found the upload gone, as a retry after a lost completion
    /// response does. It succeeded only if `key` now holds exactly these parts;
    /// otherwise the upload is unknown.
    async fn recognise_completed(
        &self,
        key: &str,
        parts: &[PartReceipt],
    ) -> Result<MultipartCompleteOutcome, StorageError> {
        // The receipts must still be well formed on their own.
        verify_receipts(key, parts, parts, self.limits())?;
        let size_bytes = parts.iter().map(|part| part.size_bytes).sum();
        if self.holds_completed_parts(key, parts, size_bytes).await? {
            Ok(MultipartCompleteOutcome {
                size_bytes,
                sha256_hex: None,
            })
        } else {
            Err(multipart_error(key, MultipartErrorKind::UnknownUpload))
        }
    }

    /// Whether `key` holds the object assembled from exactly `parts`: its size
    /// is their total and its ETag is their multipart ETag. Providers whose
    /// part ETags are not MD5s cannot be checked, and report false.
    async fn holds_completed_parts(
        &self,
        key: &str,
        parts: &[PartReceipt],
        size_bytes: u64,
    ) -> Result<bool, StorageError> {
        let Some(expected) = multipart_etag(parts) else {
            return Ok(false);
        };
        let output = match self
            .client
            .head_object()
            .bucket(&self.config.bucket)
            .key(self.provider_key(key)?)
            .send()
            .await
        {
            Ok(output) => output,
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(map_s3_error(error)),
        };
        Ok(output
            .content_length()
            .and_then(|length| u64::try_from(length).ok())
            == Some(size_bytes)
            && output.e_tag() == Some(expected.as_str()))
    }

    fn limits(&self) -> MultipartLimits {
        MultipartLimits::new(
            MIN_PART_SIZE,
            self.multipart_max_part_size,
            crate::MULTIPART_MAX_PART_NUMBER,
        )
    }

    async fn stored_parts(
        &self,
        handle: &MultipartUploadHandle,
        provider_key: &str,
    ) -> Result<Vec<PartReceipt>, StorageError> {
        let mut parts = Vec::new();
        let mut marker = None;
        loop {
            let output = self
                .client
                .list_parts()
                .bucket(&self.config.bucket)
                .key(provider_key)
                .upload_id(&handle.upload_id)
                .set_part_number_marker(marker)
                .send()
                .await
                .map_err(|error| multipart_s3_error(&handle.key, error))?;
            for part in output.parts() {
                let (Some(number), Some(size), Some(tag)) =
                    (part.part_number(), part.size(), part.e_tag())
                else {
                    continue;
                };
                parts.push(PartReceipt {
                    part_number: u32::try_from(number).unwrap_or(0),
                    size_bytes: u64::try_from(size).unwrap_or(0),
                    sha256: part
                        .checksum_sha256()
                        .and_then(sha256_hex_from_base64)
                        .unwrap_or_default(),
                    provider_tag: tag.to_string(),
                });
            }
            if output.is_truncated() != Some(true) {
                break;
            }
            marker = output.next_part_number_marker().map(ToString::to_string);
            if marker.is_none() {
                break;
            }
        }
        parts.sort_by_key(|part| part.part_number);
        Ok(parts)
    }
}

#[async_trait]
impl MultipartBlobStore for S3StorageBackend {
    fn multipart_limits(&self) -> MultipartLimits {
        self.limits()
    }

    async fn start_multipart(
        &self,
        key: &str,
        options: MultipartStartOptions,
    ) -> Result<MultipartUploadHandle, StorageError> {
        let provider_key = self.provider_key(key)?;
        let mut request = self
            .client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .checksum_algorithm(ChecksumAlgorithm::Sha256);
        if let Some(content_type) = options.put.content_type {
            request = request.content_type(content_type);
        }
        let output = request.send().await.map_err(map_s3_error)?;
        let upload_id = output.upload_id().ok_or_else(|| StorageError::Provider {
            backend: StorageBackend::S3.as_str().to_string(),
            message: "S3 multipart upload did not return an upload id".to_string(),
            retryable: false,
        })?;
        // S3 keeps no per-upload condition: the handle carries it to the
        // conditional completion.
        Ok(MultipartUploadHandle {
            backend: StorageBackend::S3,
            key: key.to_string(),
            upload_id: upload_id.to_string(),
            if_not_exists: options.if_not_exists,
        })
    }

    async fn upload_part(
        &self,
        handle: &MultipartUploadHandle,
        part_number: u32,
        body: StorageByteStream,
    ) -> Result<PartReceipt, StorageError> {
        check_handle(handle, StorageBackend::S3)?;
        let limits = self.limits();
        check_part_number(&handle.key, part_number, limits)?;
        let provider_key = self.provider_key(&handle.key)?;

        // S3 needs the part length before its body, so the part is held in a
        // buffer bounded by the configured maximum part size.
        let mut stream = body.into_inner();
        let mut buffer = BytesMut::new();
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if (buffer.len() + chunk.len()) as u64 > limits.max_part_size {
                return Err(multipart_error(
                    &handle.key,
                    MultipartErrorKind::PartTooLarge,
                ));
            }
            hasher.update(&chunk);
            buffer.extend_from_slice(&chunk);
        }
        let sha256 = format!("{:x}", hasher.finalize());
        let size_bytes = buffer.len() as u64;
        let output = self
            .client
            .upload_part()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .upload_id(&handle.upload_id)
            .part_number(i32::try_from(part_number).unwrap_or(i32::MAX))
            .set_checksum_sha256(sha256_base64(&sha256))
            .body(ByteStream::from(buffer.freeze()))
            .send()
            .await
            .map_err(|error| multipart_s3_error(&handle.key, error))?;
        let provider_tag =
            output
                .e_tag()
                .map(ToString::to_string)
                .ok_or_else(|| StorageError::Provider {
                    backend: StorageBackend::S3.as_str().to_string(),
                    message: "S3 part upload did not return an ETag".to_string(),
                    retryable: false,
                })?;
        Ok(PartReceipt {
            part_number,
            size_bytes,
            sha256,
            provider_tag,
        })
    }

    async fn list_parts(
        &self,
        handle: &MultipartUploadHandle,
    ) -> Result<Vec<PartReceipt>, StorageError> {
        check_handle(handle, StorageBackend::S3)?;
        let provider_key = self.provider_key(&handle.key)?;
        self.stored_parts(handle, &provider_key).await
    }

    async fn complete_multipart(
        &self,
        handle: &MultipartUploadHandle,
        parts: &[PartReceipt],
    ) -> Result<MultipartCompleteOutcome, StorageError> {
        check_handle(handle, StorageBackend::S3)?;
        let provider_key = self.provider_key(&handle.key)?;
        let stored = match self.stored_parts(handle, &provider_key).await {
            Ok(stored) => stored,
            Err(StorageError::Multipart {
                kind: MultipartErrorKind::UnknownUpload,
                ..
            }) => return self.recognise_completed(&handle.key, parts).await,
            Err(error) => return Err(error),
        };
        verify_receipts(&handle.key, &stored, parts, self.limits())?;
        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(
                parts
                    .iter()
                    .map(|part| {
                        // A receipt rebuilt from a listing that reports no
                        // checksum carries none; the ETag still binds the part.
                        CompletedPart::builder()
                            .part_number(i32::try_from(part.part_number).unwrap_or(i32::MAX))
                            .e_tag(&part.provider_tag)
                            .set_checksum_sha256(sha256_base64(&part.sha256))
                            .build()
                    })
                    .collect(),
            ))
            .build();
        let request = self
            .client
            .complete_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .upload_id(&handle.upload_id)
            .multipart_upload(completed);
        let request = if handle.if_not_exists {
            request.if_none_match("*")
        } else {
            request
        };
        // One attempt: a lost response must not be replayed against an upload
        // that may already have become the object.
        let sent = request
            .customize()
            .config_override(S3ConfigBuilder::new().retry_config(RetryConfig::disabled()))
            .send()
            .await
            .map_err(|error| multipart_s3_error(&handle.key, error));
        let size_bytes = parts.iter().map(|part| part.size_bytes).sum();
        match sent {
            Ok(_) => Ok(MultipartCompleteOutcome {
                size_bytes,
                sha256_hex: None,
            }),
            Err(StorageError::Multipart {
                kind: MultipartErrorKind::UnknownUpload,
                ..
            }) => self.recognise_completed(&handle.key, parts).await,
            Err(error) => Err(error),
        }
    }

    async fn abort_multipart(&self, handle: &MultipartUploadHandle) -> Result<(), StorageError> {
        check_handle(handle, StorageBackend::S3)?;
        let provider_key = self.provider_key(&handle.key)?;
        match self
            .client
            .abort_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .upload_id(&handle.upload_id)
            .send()
            .await
            .map_err(|error| multipart_s3_error(&handle.key, error))
        {
            Ok(_)
            | Err(StorageError::Multipart {
                kind: MultipartErrorKind::UnknownUpload,
                ..
            }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn list_incomplete_multipart(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<IncompleteMultipartPage, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;
        let (key_marker, upload_id_marker) = match continuation {
            Some(token) => {
                let (key, upload) =
                    split_pair_token(&token).ok_or_else(|| StorageError::PreconditionFailed {
                        key: prefix.to_string(),
                        condition: "invalid multipart listing continuation".to_string(),
                    })?;
                (Some(key), Some(upload))
            }
            None => (None, None),
        };
        let provider_prefix = self.listing_prefix(prefix)?;
        let output = self
            .client
            .list_multipart_uploads()
            .bucket(&self.config.bucket)
            .prefix(provider_prefix)
            .set_key_marker(key_marker)
            .set_upload_id_marker(upload_id_marker)
            .max_uploads(i32::try_from(limit.clamp(1, 1000)).unwrap_or(1000))
            .send()
            .await
            .map_err(map_s3_error)?;
        let mut page = IncompleteMultipartPage::default();
        for upload in output.uploads() {
            let (Some(provider_key), Some(upload_id)) = (upload.key(), upload.upload_id()) else {
                continue;
            };
            // An upload outside this backend's key prefix, such as another
            // prefix sharing the bucket, is never reported.
            let Some(key) = self.strip_provider_prefix(provider_key) else {
                continue;
            };
            if !key.starts_with(prefix) || validate_blob_key(&key).is_err() {
                continue;
            }
            page.uploads.push(IncompleteMultipartUpload {
                handle: MultipartUploadHandle {
                    backend: StorageBackend::S3,
                    key,
                    upload_id: upload_id.to_string(),
                    // S3 does not report the condition an upload was started with.
                    if_not_exists: false,
                },
                started_at: upload.initiated().and_then(|initiated| {
                    OffsetDateTime::from_unix_timestamp(initiated.secs()).ok()
                }),
            });
        }
        if output.is_truncated() == Some(true)
            && let (Some(key), Some(upload)) =
                (output.next_key_marker(), output.next_upload_id_marker())
        {
            page.next_continuation = Some(pair_token(key, upload));
        }
        Ok(page)
    }
}

fn pair_token(key: &str, upload: &str) -> String {
    format!("{}\n{}", key.len(), [key, upload].concat())
}

fn split_pair_token(token: &str) -> Option<(String, String)> {
    let (length, rest) = token.split_once('\n')?;
    let length = length.parse::<usize>().ok()?;
    (rest.len() >= length && rest.is_char_boundary(length))
        .then(|| (rest[..length].to_string(), rest[length..].to_string()))
}
