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
    BlobStore, BlobWriteOutcome, IncompleteMultipartPage, IncompleteMultipartUpload,
    MultipartBlobStore, MultipartErrorKind, MultipartLimits, MultipartStartOptions,
    MultipartUploadHandle, PartReceipt, StorageBackend, StorageByteStream, StorageError,
    validate_blob_key,
};

/// S3's smallest part, except the last.
pub(crate) const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;
/// S3's largest part.
pub(crate) const MAX_PART_SIZE: u64 = 5 * 1024 * 1024 * 1024;
/// Largest part accepted by default; it is buffered once per in-flight part.
pub(crate) const DEFAULT_MAX_PART_SIZE: u64 = 64 * 1024 * 1024;

fn sha256_base64(hex: &str) -> String {
    let bytes = (0..hex.len())
        .step_by(2)
        .filter_map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
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
        (Some("NoSuchUpload"), _) | (_, Some(404)) => {
            multipart_error(key, MultipartErrorKind::UnknownUpload)
        }
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
            .checksum_sha256(sha256_base64(&sha256))
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
    ) -> Result<BlobWriteOutcome, StorageError> {
        check_handle(handle, StorageBackend::S3)?;
        let provider_key = self.provider_key(&handle.key)?;
        let stored = self.stored_parts(handle, &provider_key).await?;
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
                            .set_checksum_sha256(
                                (!part.sha256.is_empty()).then(|| sha256_base64(&part.sha256)),
                            )
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
        request
            .customize()
            .config_override(S3ConfigBuilder::new().retry_config(RetryConfig::disabled()))
            .send()
            .await
            .map_err(|error| multipart_s3_error(&handle.key, error))?;

        // S3 reports no whole-object SHA-256 for multipart objects, so read the
        // published object back once to return a verified digest.
        let blob = self.get_blob(&handle.key).await?;
        let mut stream = blob.body.into_inner();
        let mut hasher = Sha256::new();
        let mut size_bytes = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            size_bytes += chunk.len() as u64;
            hasher.update(&chunk);
        }
        Ok(BlobWriteOutcome {
            size_bytes,
            sha256_hex: format!("{:x}", hasher.finalize()),
        })
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
        let provider_prefix = if prefix.is_empty() {
            self.config
                .key_prefix
                .as_deref()
                .map(|prefix| prefix.trim_matches('/'))
                .unwrap_or_default()
                .to_string()
        } else {
            self.provider_key(prefix)?
        };
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
            let key = self.strip_provider_prefix(provider_key);
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
