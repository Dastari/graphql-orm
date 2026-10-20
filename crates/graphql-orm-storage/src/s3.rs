use std::fmt;

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{Builder as S3ConfigBuilder, Region, retry::RetryConfig},
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio_util::io::ReaderStream;

use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    CapacityBinding, CapacityObservation, CapacityPlacement, CapacityRequest, CapacityState,
    CapacityUnknownReason, ObjectStorage, ProviderManagedBasis, ProviderManagedCapacity,
    StorageBackend, StorageByteStream, StorageError, StorageObjectBody, StoredObject,
    UnknownCapacity, collect_storage_stream, validate_blob_key,
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

/// How an S3-compatible destination reports capacity.
///
/// The S3 API has no capacity or quota operation. Capacity is therefore never
/// inferred from a successful request; the destination configuration selects
/// the basis and every observation re-verifies it against the provider.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum S3CapacityMode {
    /// The provider may enforce a quota that the S3 API cannot observe.
    ///
    /// Observations report [`CapacityState::Unknown`] with
    /// [`CapacityUnknownReason::ProviderQuotaNotObservable`].
    #[default]
    Unknown,
    /// The operator declares that the provider performs no physical-byte
    /// capacity admission for this bucket and account.
    ///
    /// Observations report [`CapacityState::ProviderManaged`] with
    /// [`ProviderManagedBasis::OperatorDeclared`] after the authenticated
    /// account reaches the bucket. Reachability does not prove the absence of
    /// a finite provider quota. Consumers must still enforce a finite
    /// service-owned quota.
    ProviderManaged,
    /// An Amazon S3 general-purpose bucket, which AWS documents as having no
    /// maximum bucket size.
    ///
    /// The endpoint must be the AWS S3 HTTPS endpoint for the configured
    /// region (`s3.<region>.amazonaws.com` or its dual-stack form), the bucket
    /// name must not name a directory bucket, access point alias or Outposts
    /// alias, and `HeadBucket` must succeed with `ExpectedBucketOwner` and
    /// report the configured region and no directory-bucket location type.
    /// Observations then report [`CapacityState::ProviderManaged`] with
    /// [`ProviderManagedBasis::AwsGeneralPurposeBucket`]; any mismatch is an
    /// error. Consumers must still enforce a finite service-owned quota, and
    /// individual writes can still fail.
    AwsGeneralPurpose {
        /// Twelve-digit AWS account ID that must own the bucket.
        expected_bucket_owner: String,
    },
}

impl S3CapacityMode {
    const fn name(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::ProviderManaged => "provider_managed",
            Self::AwsGeneralPurpose { .. } => "aws_general_purpose",
        }
    }

    fn expected_bucket_owner(&self) -> Option<&str> {
        match self {
            Self::AwsGeneralPurpose {
                expected_bucket_owner,
            } => Some(expected_bucket_owner),
            Self::Unknown | Self::ProviderManaged => None,
        }
    }
}

/// S3-compatible storage backend.
#[derive(Clone, Debug)]
pub struct S3StorageBackend {
    config: S3StorageConfig,
    capacity_mode: S3CapacityMode,
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
            capacity_mode: S3CapacityMode::default(),
            client: Client::from_conf(s3_config),
        }
    }

    /// Declares how this destination reports capacity.
    #[must_use]
    pub fn with_capacity_mode(mut self, mode: S3CapacityMode) -> Self {
        self.capacity_mode = mode;
        self
    }

    /// Returns the backend configuration.
    #[must_use]
    pub const fn config(&self) -> &S3StorageConfig {
        &self.config
    }

    /// Returns the declared capacity mode.
    #[must_use]
    pub const fn capacity_mode(&self) -> &S3CapacityMode {
        &self.capacity_mode
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
    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError> {
        let provider_key = self.provider_key(key)?;
        self.client
            .delete_object()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .customize()
            .config_override(S3ConfigBuilder::new().retry_config(RetryConfig::disabled()))
            .send()
            .await
            .map_err(|error| StorageError::single_attempt(map_s3_error(error)))?;
        Ok(())
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::S3
    }

    fn capacity_binding(&self) -> CapacityBinding {
        let path_style = if self.config.path_style {
            "true"
        } else {
            "false"
        };
        CapacityBinding::new(
            StorageBackend::S3,
            &[
                ("endpoint_url", self.config.endpoint_url.as_str()),
                ("region", self.config.region.as_str()),
                ("bucket", self.config.bucket.as_str()),
                (
                    "key_prefix",
                    self.config.key_prefix.as_deref().unwrap_or(""),
                ),
                ("access_key_id", self.config.access_key_id.as_str()),
                ("path_style", path_style),
                ("capacity_mode", self.capacity_mode.name()),
                (
                    "expected_bucket_owner",
                    self.capacity_mode.expected_bucket_owner().unwrap_or(""),
                ),
            ],
        )
    }

    async fn observe_capacity(
        &self,
        request: &CapacityRequest,
    ) -> Result<CapacityObservation, StorageError> {
        // Prove the configured account reaches the exact bucket before
        // reporting any state for it.
        let observed_at = OffsetDateTime::now_utc();
        let mut head = self.client.head_bucket().bucket(&self.config.bucket);
        if let Some(owner) = self.capacity_mode.expected_bucket_owner() {
            aws_general_purpose_configuration(&self.config, owner).map_err(s3_configuration_error)?;
            head = head.expected_bucket_owner(owner);
        }
        let head = head.send().await.map_err(map_s3_error)?;
        let placement = head
            .bucket_region()
            .map(|region| CapacityPlacement(format!("s3:region={region}")));
        let state = match self.capacity_mode {
            S3CapacityMode::Unknown => CapacityState::Unknown(UnknownCapacity {
                reason: CapacityUnknownReason::ProviderQuotaNotObservable,
            }),
            S3CapacityMode::ProviderManaged => {
                CapacityState::ProviderManaged(ProviderManagedCapacity {
                    basis: ProviderManagedBasis::OperatorDeclared,
                })
            }
            S3CapacityMode::AwsGeneralPurpose { .. } => {
                if head.bucket_region() != Some(self.config.region.as_str()) {
                    return Err(s3_configuration_error(
                        "bucket region does not match the configured AWS region",
                    ));
                }
                if head.bucket_location_type().is_some() {
                    return Err(s3_configuration_error(
                        "bucket is a directory bucket, not a general-purpose bucket",
                    ));
                }
                CapacityState::ProviderManaged(ProviderManagedCapacity {
                    basis: ProviderManagedBasis::AwsGeneralPurposeBucket,
                })
            }
        };
        let binding = self.capacity_binding();
        Ok(CapacityObservation::new(
            binding,
            placement,
            state,
            observed_at,
            request,
        ))
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

/// Checks the configuration-side preconditions of the AWS general-purpose
/// capacity basis without provider I/O.
fn aws_general_purpose_configuration(
    config: &S3StorageConfig,
    expected_bucket_owner: &str,
) -> Result<(), &'static str> {
    let digits = expected_bucket_owner.bytes().all(|b| b.is_ascii_digit());
    if expected_bucket_owner.len() != 12 || !digits {
        return Err("expected bucket owner must be a twelve-digit AWS account ID");
    }
    if !is_aws_regional_endpoint(&config.endpoint_url, &config.region) {
        return Err("AWS general-purpose capacity requires the regional AWS S3 HTTPS endpoint");
    }
    if !is_general_purpose_bucket_name(&config.bucket) {
        return Err("bucket name does not name an AWS general-purpose bucket");
    }
    Ok(())
}

fn is_aws_regional_endpoint(endpoint: &str, region: &str) -> bool {
    let valid_region = !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let Some(host) = endpoint.strip_prefix("https://") else {
        return false;
    };
    let host = host.strip_suffix('/').unwrap_or(host);
    valid_region
        && (host == format!("s3.{region}.amazonaws.com")
            || host == format!("s3.dualstack.{region}.amazonaws.com"))
}

fn is_general_purpose_bucket_name(bucket: &str) -> bool {
    let reserved_suffix = ["--x-s3", "--table-s3", "--op-s3", "-s3alias"]
        .iter()
        .any(|suffix| bucket.ends_with(suffix));
    (3..=63).contains(&bucket.len())
        && bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !reserved_suffix
}

fn s3_configuration_error(message: &str) -> StorageError {
    StorageError::Provider {
        backend: StorageBackend::S3.as_str().to_string(),
        message: message.to_string(),
        retryable: false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str, region: &str, bucket: &str) -> S3StorageConfig {
        S3StorageConfig {
            endpoint_url: endpoint.to_string(),
            region: region.to_string(),
            bucket: bucket.to_string(),
            key_prefix: None,
            access_key_id: "fixture".to_string(),
            secret_access_key: "fixture-secret".to_string(),
            path_style: false,
        }
    }

    #[test]
    fn aws_endpoint_must_be_the_https_endpoint_of_the_configured_region() {
        for endpoint in [
            "https://s3.ap-southeast-2.amazonaws.com",
            "https://s3.ap-southeast-2.amazonaws.com/",
            "https://s3.dualstack.ap-southeast-2.amazonaws.com",
        ] {
            assert!(is_aws_regional_endpoint(endpoint, "ap-southeast-2"), "{endpoint}");
        }
        for endpoint in [
            "http://s3.ap-southeast-2.amazonaws.com",
            "https://s3.us-east-1.amazonaws.com",
            "https://s3.ap-southeast-2.amazonaws.com:8443",
            "https://s3.ap-southeast-2.amazonaws.com/bucket",
            "https://s3.ap-southeast-2.amazonaws.com.example.test",
            "https://minio.example.test",
            "https://s3.cn-north-1.amazonaws.com.cn",
        ] {
            assert!(!is_aws_regional_endpoint(endpoint, "ap-southeast-2"), "{endpoint}");
        }
        assert!(!is_aws_regional_endpoint("https://s3..amazonaws.com", ""));
        assert!(!is_aws_regional_endpoint("https://s3.x/y.amazonaws.com", "x/y"));
    }

    #[test]
    fn only_general_purpose_bucket_names_qualify() {
        for bucket in ["backups", "org.backups-1", "abc"] {
            assert!(is_general_purpose_bucket_name(bucket), "{bucket}");
        }
        for bucket in [
            "backups--usw2-az1--x-s3",
            "tables--table-s3",
            "outpost-alias--op-s3",
            "ap-alias-abcdef-s3alias",
            "arn:aws:s3:::backups",
            "Backups",
            "ab",
        ] {
            assert!(!is_general_purpose_bucket_name(bucket), "{bucket}");
        }
    }

    #[test]
    fn aws_general_purpose_configuration_requires_account_endpoint_and_bucket() {
        let aws = config("https://s3.ap-southeast-2.amazonaws.com", "ap-southeast-2", "backups");
        assert_eq!(aws_general_purpose_configuration(&aws, "123456789012"), Ok(()));
        assert!(aws_general_purpose_configuration(&aws, "12345678901").is_err());
        assert!(aws_general_purpose_configuration(&aws, "12345678901x").is_err());
        let custom = config("https://minio.example.test", "ap-southeast-2", "backups");
        assert!(aws_general_purpose_configuration(&custom, "123456789012").is_err());
        let directory = config(
            "https://s3.ap-southeast-2.amazonaws.com",
            "ap-southeast-2",
            "backups--apse2-az1--x-s3",
        );
        assert!(aws_general_purpose_configuration(&directory, "123456789012").is_err());
    }
}
