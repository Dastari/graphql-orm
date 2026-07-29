use std::fmt;

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{Builder as S3ConfigBuilder, Region},
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;

use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    ObjectStorage, StorageBackend, StorageByteStream, StorageError, StorageObjectBody,
    StoredObject, collect_storage_stream, validate_blob_key,
};

const MULTIPART_PART_SIZE: usize = 8 * 1024 * 1024;

/// Configuration for an S3-compatible storage backend.
#[derive(Clone, PartialEq, Eq)]
pub struct S3StorageConfig {
    /// S3-compatible endpoint URL.
    pub endpoint_url: String,
    /// Provider region.
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Optional key prefix prepended by the provider implementation.
    pub key_prefix: Option<String>,
    /// Access key identifier.
    pub access_key_id: String,
    /// Secret access key. Redacted from debug output.
    pub secret_access_key: String,
    /// Whether to use path-style addressing.
    pub path_style: bool,
}

impl fmt::Debug for S3StorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3StorageConfig")
            .field("endpoint_url", &self.endpoint_url)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .finish()
    }
}

/// S3-compatible storage backend.
#[derive(Clone, Debug)]
pub struct S3StorageBackend {
    config: S3StorageConfig,
    client: Client,
}

impl S3StorageBackend {
    /// Creates a new S3-compatible backend.
    #[must_use]
    pub fn new(config: S3StorageConfig) -> Self {
        let credentials = Credentials::new(
            config.access_key_id.clone(),
            config.secret_access_key.clone(),
            None,
            None,
            "graphql-orm-storage",
        );
        let s3_config = S3ConfigBuilder::new()
            .region(Region::new(config.region.clone()))
            .credentials_provider(credentials)
            .endpoint_url(config.endpoint_url.clone())
            .force_path_style(config.path_style)
            .build();

        Self {
            config,
            client: Client::from_conf(s3_config),
        }
    }

    /// Returns the backend configuration.
    #[must_use]
    pub const fn config(&self) -> &S3StorageConfig {
        &self.config
    }

    fn provider_key(&self, key: &str) -> Result<String, StorageError> {
        validate_blob_key(key)?;
        let Some(prefix) = self.config.key_prefix.as_deref() else {
            return Ok(key.to_string());
        };
        let prefix = prefix.trim_matches('/');
        if prefix.is_empty() {
            return Ok(key.to_string());
        }
        validate_blob_key(prefix)?;
        Ok(format!("{prefix}/{key}"))
    }

    fn strip_provider_prefix(&self, provider_key: &str) -> String {
        let Some(prefix) = self.config.key_prefix.as_deref() else {
            return provider_key.to_string();
        };
        let prefix = prefix.trim_matches('/');
        provider_key
            .strip_prefix(prefix)
            .and_then(|key| key.strip_prefix('/'))
            .unwrap_or(provider_key)
            .to_string()
    }

    async fn put_blob_inner(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
        if_not_exists: bool,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        let provider_key = self.provider_key(key)?;
        let mut stream = body.into_inner();
        let mut buffer = BytesMut::new();
        let mut hasher = Sha256::new();
        let mut size_bytes = 0_u64;
        let mut multipart = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            size_bytes = size_bytes.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            hasher.update(&chunk);
            buffer.extend_from_slice(&chunk);

            while buffer.len() >= MULTIPART_PART_SIZE {
                let state = match multipart.as_mut() {
                    Some(state) => state,
                    None => {
                        multipart = Some(
                            self.create_multipart_upload(&provider_key, &options)
                                .await?,
                        );
                        multipart.as_mut().expect("multipart state exists")
                    }
                };
                let part = buffer.split_to(MULTIPART_PART_SIZE).freeze();
                self.upload_part(&provider_key, state, part).await?;
            }
        }

        let outcome = BlobWriteOutcome {
            size_bytes,
            sha256_hex: format!("{:x}", hasher.finalize()),
        };

        match multipart {
            Some(mut state) => {
                if !buffer.is_empty() {
                    self.upload_part(&provider_key, &mut state, buffer.freeze())
                        .await?;
                }

                match self
                    .complete_multipart_upload(&provider_key, state, if_not_exists)
                    .await
                {
                    Ok(()) => Ok(Some(outcome)),
                    Err(err) if if_not_exists && is_precondition_error(&err) => Ok(None),
                    Err(err) => Err(err),
                }
            }
            None => {
                let bytes = buffer.freeze();
                match self
                    .put_single_blob(&provider_key, bytes, options, if_not_exists)
                    .await
                {
                    Ok(()) => Ok(Some(outcome)),
                    Err(err) if if_not_exists && is_precondition_error(&err) => Ok(None),
                    Err(err) => Err(err),
                }
            }
        }
    }

    async fn put_single_blob(
        &self,
        provider_key: &str,
        bytes: Bytes,
        options: BlobPutOptions,
        if_not_exists: bool,
    ) -> Result<(), StorageError> {
        let mut request = self
            .client
            .put_object()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .body(ByteStream::from(bytes));

        if let Some(content_type) = options.content_type {
            request = request.content_type(content_type);
        }
        if if_not_exists {
            request = request.if_none_match("*");
        }

        request.send().await.map_err(map_s3_error)?;
        Ok(())
    }

    async fn create_multipart_upload(
        &self,
        provider_key: &str,
        options: &BlobPutOptions,
    ) -> Result<MultipartUploadState, StorageError> {
        let mut request = self
            .client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(provider_key);

        if let Some(content_type) = options.content_type.as_deref() {
            request = request.content_type(content_type);
        }

        let output = request.send().await.map_err(map_s3_error)?;
        let upload_id = output.upload_id().ok_or_else(|| StorageError::Provider {
            backend: StorageBackend::S3.as_str().to_string(),
            message: "S3 multipart upload did not return an upload id".to_string(),
            retryable: true,
        })?;

        Ok(MultipartUploadState {
            upload_id: upload_id.to_string(),
            next_part_number: 1,
            completed_parts: Vec::new(),
        })
    }

    async fn upload_part(
        &self,
        provider_key: &str,
        state: &mut MultipartUploadState,
        bytes: Bytes,
    ) -> Result<(), StorageError> {
        let part_number = state.next_part_number;
        let output = self
            .client
            .upload_part()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .upload_id(&state.upload_id)
            .part_number(part_number)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(map_s3_error)?;

        let completed_part = CompletedPart::builder()
            .set_e_tag(output.e_tag().map(ToString::to_string))
            .part_number(part_number)
            .build();
        state.completed_parts.push(completed_part);
        state.next_part_number += 1;
        Ok(())
    }

    async fn complete_multipart_upload(
        &self,
        provider_key: &str,
        state: MultipartUploadState,
        if_not_exists: bool,
    ) -> Result<(), StorageError> {
        let upload_id = state.upload_id.clone();
        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(state.completed_parts))
            .build();
        let mut request = self
            .client
            .complete_multipart_upload()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .upload_id(&upload_id)
            .multipart_upload(completed);

        if if_not_exists {
            request = request.if_none_match("*");
        }

        match request.send().await.map_err(map_s3_error) {
            Ok(_) => Ok(()),
            Err(err) => {
                let _ = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&self.config.bucket)
                    .key(provider_key)
                    .upload_id(upload_id)
                    .send()
                    .await;
                Err(err)
            }
        }
    }
}

#[async_trait]
impl BlobStore for S3StorageBackend {
    fn backend(&self) -> StorageBackend {
        StorageBackend::S3
    }

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        self.put_blob_inner(key, body, options, false)
            .await?
            .ok_or_else(|| StorageError::PreconditionFailed {
                key: key.to_string(),
                condition: "blob unexpectedly already exists".to_string(),
            })
    }

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        self.put_blob_inner(key, body, options, true).await
    }

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError> {
        let provider_key = self.provider_key(key)?;
        let output = match self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) if is_missing_error(&err) => {
                return Err(StorageError::MissingBlob {
                    key: key.to_string(),
                });
            }
            Err(err) => return Err(map_s3_error(err)),
        };

        let metadata = BlobMetadata {
            key: key.to_string(),
            size_bytes: output
                .content_length()
                .and_then(|size| u64::try_from(size).ok()),
            sha256_hex: None,
            etag: output.e_tag().map(ToString::to_string),
            last_modified: None,
        };
        let stream_key = key.to_string();
        let stream = ReaderStream::new(output.body.into_async_read()).map(move |chunk| {
            chunk.map_err(|source| StorageError::Provider {
                backend: StorageBackend::S3.as_str().to_string(),
                message: format!("S3 object stream failed for {stream_key}: {source}"),
                retryable: true,
            })
        });

        Ok(BlobBody {
            key: key.to_string(),
            metadata: Some(metadata),
            body: StorageByteStream::new(Box::pin(stream)),
        })
    }

    async fn get_blob_range(
        &self,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<BlobBody, StorageError> {
        if range.end < range.start {
            return Err(StorageError::PreconditionFailed {
                key: key.to_string(),
                condition: "range end is before range start".to_string(),
            });
        }
        if range.start == range.end {
            return Ok(BlobBody {
                key: key.to_string(),
                metadata: self.head_blob(key).await?,
                body: StorageByteStream::from_bytes(Bytes::new()),
            });
        }

        let provider_key = self.provider_key(key)?;
        let range_header = format!("bytes={}-{}", range.start, range.end - 1);
        let output = match self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .range(range_header)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) if is_missing_error(&err) => {
                return Err(StorageError::MissingBlob {
                    key: key.to_string(),
                });
            }
            Err(err) => return Err(map_s3_error(err)),
        };

        let metadata = BlobMetadata {
            key: key.to_string(),
            size_bytes: output
                .content_length()
                .and_then(|size| u64::try_from(size).ok()),
            sha256_hex: None,
            etag: output.e_tag().map(ToString::to_string),
            last_modified: None,
        };
        let stream_key = key.to_string();
        let stream = ReaderStream::new(output.body.into_async_read()).map(move |chunk| {
            chunk.map_err(|source| StorageError::Provider {
                backend: StorageBackend::S3.as_str().to_string(),
                message: format!("S3 object range stream failed for {stream_key}: {source}"),
                retryable: true,
            })
        });

        Ok(BlobBody {
            key: key.to_string(),
            metadata: Some(metadata),
            body: StorageByteStream::with_size_hint(Box::pin(stream), range.end - range.start),
        })
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.head_blob(key).await?.is_some())
    }

    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError> {
        let provider_key = self.provider_key(key)?;
        let output = match self
            .client
            .head_object()
            .bucket(&self.config.bucket)
            .key(&provider_key)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) if is_missing_error(&err) => return Ok(None),
            Err(err) => return Err(map_s3_error(err)),
        };

        Ok(Some(BlobMetadata {
            key: key.to_string(),
            size_bytes: output
                .content_length()
                .and_then(|size| u64::try_from(size).ok()),
            sha256_hex: None,
            etag: output.e_tag().map(ToString::to_string),
            last_modified: None,
        }))
    }

    async fn list_blobs_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError> {
        if limit == 0 {
            return Err(StorageError::PreconditionFailed {
                key: prefix.to_string(),
                condition: "list limit must be greater than zero".to_string(),
            });
        }

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
            .list_objects_v2()
            .bucket(&self.config.bucket)
            .prefix(provider_prefix)
            .set_continuation_token(continuation)
            .max_keys(i32::try_from(limit).unwrap_or(i32::MAX))
            .send()
            .await
            .map_err(map_s3_error)?;

        let keys = output
            .contents()
            .iter()
            .filter_map(|object| object.key())
            .map(|key| self.strip_provider_prefix(key))
            .filter(|key| !key.is_empty())
            .collect();

        Ok(BlobListPage {
            keys,
            next_continuation: output.next_continuation_token().map(ToString::to_string),
        })
    }

    async fn copy_blob(&self, from: &str, to: &str) -> Result<(), StorageError> {
        let from_key = self.provider_key(from)?;
        let to_key = self.provider_key(to)?;
        let copy_source = format!("{}/{}", self.config.bucket, from_key);

        match self
            .client
            .copy_object()
            .bucket(&self.config.bucket)
            .key(to_key)
            .copy_source(copy_source)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if is_missing_error(&err) => Err(StorageError::MissingBlob {
                key: from.to_string(),
            }),
            Err(err) => Err(map_s3_error(err)),
        }
    }

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError> {
        let provider_key = self.provider_key(key)?;
        self.client
            .delete_object()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .send()
            .await
            .map_err(map_s3_error)?;
        Ok(())
    }
}

#[async_trait]
impl ObjectStorage for S3StorageBackend {
    async fn put_object(
        &self,
        object: StoredObject,
        bytes: Vec<u8>,
    ) -> Result<StoredObject, StorageError> {
        self.put_blob(
            &object.storage_key,
            StorageByteStream::from_bytes(bytes),
            BlobPutOptions {
                content_type: object.mime_type.clone(),
            },
        )
        .await?;
        Ok(object)
    }

    async fn get_object(&self, object: &StoredObject) -> Result<StorageObjectBody, StorageError> {
        let body = self.get_blob(&object.storage_key).await?;
        let bytes = collect_storage_stream(body.body).await?;
        Ok(StorageObjectBody {
            object: object.clone(),
            bytes: bytes.to_vec(),
        })
    }

    async fn delete_object(&self, object: &StoredObject) -> Result<(), StorageError> {
        self.delete_blob(&object.storage_key).await
    }
}

#[derive(Debug)]
struct MultipartUploadState {
    upload_id: String,
    next_part_number: i32,
    completed_parts: Vec<CompletedPart>,
}

fn is_missing_error<E>(err: &aws_sdk_s3::error::SdkError<E>) -> bool
where
    E: fmt::Debug,
{
    let error = format!("{err:?}");
    error.contains("NoSuchKey")
        || error.contains("NotFound")
        || error.contains("Not Found")
        || error.contains("status: 404")
        || error.contains("code: 404")
}

fn is_precondition_error(err: &StorageError) -> bool {
    let message = err.to_string();
    message.contains("PreconditionFailed")
        || message.contains("Precondition Failed")
        || message.contains("status: 412")
        || message.contains("code: 412")
}

fn map_s3_error<E>(err: aws_sdk_s3::error::SdkError<E>) -> StorageError
where
    E: fmt::Debug,
{
    let message = format!("{err:?}");
    let retryable = is_retryable_s3_message(&message);
    StorageError::Provider {
        backend: StorageBackend::S3.as_str().to_string(),
        message,
        retryable,
    }
}

fn is_retryable_s3_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("dispatch")
        || lower.contains("connection")
        || lower.contains("throttl")
        || lower.contains("slowdown")
        || lower.contains("temporarily")
        || lower.contains("status: 5")
        || lower.contains("code: 5")
}
