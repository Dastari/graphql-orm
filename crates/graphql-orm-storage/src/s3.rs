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
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::namespace::{ScanDeadline, fence_invalidated, require_key_in_target};
use crate::{AncestorScan, NestedScan};
use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    CapacityDomainObservation, DEFAULT_MARKER_READ_TIMEOUT, DomainFact, DomainState, FactScope,
    FoundNamespace, KeyEquivalence, MAX_NAMESPACE_MARKER_BYTES, NAMESPACE_MARKER, NamespaceFence,
    NamespaceId, NamespaceInitialisation, NamespaceObservation, NamespaceState, NamespaceTarget,
    ObjectStorage, ObservationEpoch, ScanCompleteness, ScanResult, StorageBackend,
    StorageByteStream, StorageError, StorageObjectBody, StoredObject, TopologyScan,
    collect_storage_stream, decode_marker, encode_marker, is_reserved_key, validate_blob_key,
    validate_unreserved_key,
};

/// S3 keys are byte-exact.
const S3_KEY_EQUIVALENCE: KeyEquivalence = KeyEquivalence::Exact;

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
    instance: Uuid,
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
            instance: Uuid::new_v4(),
        }
    }

    /// Returns the backend configuration.
    #[must_use]
    pub const fn config(&self) -> &S3StorageConfig {
        &self.config
    }

    fn provider_key(&self, key: &str) -> Result<String, StorageError> {
        validate_unreserved_key(key, S3_KEY_EQUIVALENCE)?;
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

    /// Uploads a body. `attempt` selects the SDK retry policy for every
    /// mutation of the lifecycle (single object, multipart create, parts,
    /// completion and abort).
    async fn put_blob_inner(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
        if_not_exists: bool,
        attempt: Attempt,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        let provider_key = self.provider_key(key)?;
        let mut stream = body.into_inner();
        let mut buffer = BytesMut::new();
        let mut hasher = Sha256::new();
        let mut size_bytes = 0_u64;
        let mut multipart: Option<MultipartUploadState> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    // Before any mutation the caller's stream error is returned
                    // as is; after a multipart start the outcome is ambiguous.
                    let started = multipart.is_some();
                    self.abort_after_failure(&provider_key, multipart.as_ref(), attempt)
                        .await;
                    return Err(if started { attempt.wrap(error) } else { error });
                }
            };
            size_bytes = size_bytes.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            hasher.update(&chunk);
            buffer.extend_from_slice(&chunk);

            while buffer.len() >= MULTIPART_PART_SIZE {
                if multipart.is_none() {
                    multipart = Some(
                        self.create_multipart_upload(&provider_key, &options, attempt)
                            .await
                            .map_err(|error| attempt.wrap(error))?,
                    );
                }
                let state = multipart.as_mut().expect("multipart state exists");
                let part = buffer.split_to(MULTIPART_PART_SIZE).freeze();
                let uploaded = self.upload_part(&provider_key, state, part, attempt).await;
                if let Err(error) = uploaded {
                    self.abort_after_failure(&provider_key, multipart.as_ref(), attempt)
                        .await;
                    return Err(attempt.wrap(error));
                }
            }
        }

        let outcome = BlobWriteOutcome {
            size_bytes,
            sha256_hex: format!("{:x}", hasher.finalize()),
        };

        let result = match multipart {
            Some(mut state) => {
                let uploaded = if buffer.is_empty() {
                    Ok(())
                } else {
                    self.upload_part(&provider_key, &mut state, buffer.freeze(), attempt)
                        .await
                };
                if let Err(error) = uploaded {
                    self.abort_after_failure(&provider_key, Some(&state), attempt)
                        .await;
                    return Err(attempt.wrap(error));
                }
                self.complete_multipart_upload(&provider_key, state, if_not_exists, attempt)
                    .await
            }
            None => {
                let bytes = buffer.freeze();
                self.put_single_blob(&provider_key, bytes, options, if_not_exists, attempt)
                    .await
            }
        };
        match result {
            Ok(()) => Ok(Some(outcome)),
            Err(err) if if_not_exists && is_precondition_error(&err) => Ok(None),
            Err(err) => Err(attempt.wrap(err)),
        }
    }

    /// Single-attempt lifecycles abort a started multipart upload once, without
    /// retries. Ordinary uploads keep their existing behavior.
    async fn abort_after_failure(
        &self,
        provider_key: &str,
        state: Option<&MultipartUploadState>,
        attempt: Attempt,
    ) {
        if attempt == Attempt::Single
            && let Some(state) = state
        {
            self.abort_multipart_upload(provider_key, &state.upload_id, attempt)
                .await;
        }
    }

    async fn abort_multipart_upload(&self, provider_key: &str, upload_id: &str, attempt: Attempt) {
        let request = self
            .client
            .abort_multipart_upload()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .upload_id(upload_id);
        let _ = match attempt {
            Attempt::Ordinary => request.send().await,
            Attempt::Single => {
                request
                    .customize()
                    .config_override(single_attempt_config())
                    .send()
                    .await
            }
        };
    }

    async fn put_single_blob(
        &self,
        provider_key: &str,
        bytes: Bytes,
        options: BlobPutOptions,
        if_not_exists: bool,
        attempt: Attempt,
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

        match attempt {
            Attempt::Ordinary => request.send().await.map(|_| ()),
            Attempt::Single => request
                .customize()
                .config_override(single_attempt_config())
                .send()
                .await
                .map(|_| ()),
        }
        .map_err(map_s3_error)
    }

    async fn create_multipart_upload(
        &self,
        provider_key: &str,
        options: &BlobPutOptions,
        attempt: Attempt,
    ) -> Result<MultipartUploadState, StorageError> {
        let mut request = self
            .client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(provider_key);

        if let Some(content_type) = options.content_type.as_deref() {
            request = request.content_type(content_type);
        }

        let output = match attempt {
            Attempt::Ordinary => request.send().await,
            Attempt::Single => {
                request
                    .customize()
                    .config_override(single_attempt_config())
                    .send()
                    .await
            }
        }
        .map_err(map_s3_error)?;
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
        attempt: Attempt,
    ) -> Result<(), StorageError> {
        let part_number = state.next_part_number;
        let request = self
            .client
            .upload_part()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .upload_id(&state.upload_id)
            .part_number(part_number)
            .body(ByteStream::from(bytes));
        let output = match attempt {
            Attempt::Ordinary => request.send().await,
            Attempt::Single => {
                request
                    .customize()
                    .config_override(single_attempt_config())
                    .send()
                    .await
            }
        }
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
        attempt: Attempt,
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

        let result = match attempt {
            Attempt::Ordinary => request.send().await.map(|_| ()),
            Attempt::Single => request
                .customize()
                .config_override(single_attempt_config())
                .send()
                .await
                .map(|_| ()),
        };
        match result.map_err(map_s3_error) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.abort_multipart_upload(provider_key, &upload_id, attempt)
                    .await;
                Err(err)
            }
        }
    }
}

/// Retry policy for one upload lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Attempt {
    /// The client's configured retries (ordinary APIs).
    Ordinary,
    /// Exactly one request per mutation; failures are `SingleAttemptFailed`.
    Single,
}

impl Attempt {
    fn wrap(self, error: StorageError) -> StorageError {
        match self {
            Self::Ordinary => error,
            Self::Single => StorageError::single_attempt(error),
        }
    }
}

fn single_attempt_config() -> S3ConfigBuilder {
    S3ConfigBuilder::new().retry_config(RetryConfig::disabled())
}

fn s3_is_denied<E: fmt::Debug>(error: &aws_sdk_s3::error::SdkError<E>) -> bool {
    let message = format!("{error:?}");
    message.contains("AccessDenied")
        || message.contains("status: 403")
        || message.contains("code: 403")
}

fn s3_is_precondition<E: fmt::Debug>(error: &aws_sdk_s3::error::SdkError<E>) -> bool {
    let message = format!("{error:?}");
    message.contains("PreconditionFailed")
        || message.contains("status: 412")
        || message.contains("code: 412")
}

fn s3_is_unsatisfiable_range<E: fmt::Debug>(error: &aws_sdk_s3::error::SdkError<E>) -> bool {
    let message = format!("{error:?}");
    message.contains("InvalidRange")
        || message.contains("status: 416")
        || message.contains("code: 416")
}

fn marker_key_in(prefix: &str) -> String {
    if prefix.is_empty() {
        NAMESPACE_MARKER.to_string()
    } else {
        format!("{prefix}/{NAMESPACE_MARKER}")
    }
}

fn content_range_total(range: &str) -> Option<u64> {
    range
        .rsplit_once('/')
        .and_then(|(_, total)| total.parse().ok())
}

impl S3StorageBackend {
    fn root_prefix(&self) -> String {
        self.config
            .key_prefix
            .as_deref()
            .map(|prefix| prefix.trim_matches('/'))
            .unwrap_or_default()
            .to_string()
    }

    fn namespace_prefix(&self, target: &NamespaceTarget) -> Result<String, StorageError> {
        let root = self.root_prefix();
        if !root.is_empty() {
            validate_blob_key(&root)?;
        }
        Ok(
            match (target.relative_prefix(S3_KEY_EQUIVALENCE)?, root.is_empty()) {
                (None, _) => root,
                (Some(relative), true) => relative.to_string(),
                (Some(relative), false) => format!("{root}/{relative}"),
            },
        )
    }

    /// Bounded marker read: a ranged GET of at most MAX+1 bytes, a bounded
    /// body read and one deadline. Size hints are never trusted.
    async fn read_marker(&self, provider_key: &str) -> NamespaceState {
        let read = async {
            let output = match self
                .client
                .get_object()
                .bucket(&self.config.bucket)
                .key(provider_key)
                .range(format!("bytes=0-{MAX_NAMESPACE_MARKER_BYTES}"))
                .send()
                .await
            {
                Ok(output) => output,
                Err(error) if is_missing_error(&error) => return NamespaceState::Uninitialised,
                Err(error) if s3_is_unsatisfiable_range(&error) => {
                    return NamespaceState::Conflict {
                        reason: "empty marker".to_string(),
                    };
                }
                Err(error) if s3_is_denied(&error) => {
                    return NamespaceState::Unknown {
                        reason: "marker read denied".to_string(),
                    };
                }
                Err(_) => {
                    return NamespaceState::Unknown {
                        reason: "marker read failed".to_string(),
                    };
                }
            };
            if output
                .content_range()
                .and_then(content_range_total)
                .is_some_and(|total| total > MAX_NAMESPACE_MARKER_BYTES as u64)
            {
                return NamespaceState::Conflict {
                    reason: "oversized marker".to_string(),
                };
            }
            let mut bytes = Vec::with_capacity(MAX_NAMESPACE_MARKER_BYTES + 1);
            let limit = (MAX_NAMESPACE_MARKER_BYTES + 1) as u64;
            match output
                .body
                .into_async_read()
                .take(limit)
                .read_to_end(&mut bytes)
                .await
            {
                Ok(_) => decode_marker(&bytes),
                Err(_) => NamespaceState::Unknown {
                    reason: "marker body read failed".to_string(),
                },
            }
        };
        match tokio::time::timeout(DEFAULT_MARKER_READ_TIMEOUT, read).await {
            Ok(state) => state,
            Err(_) => NamespaceState::Unknown {
                reason: "marker read timed out".to_string(),
            },
        }
    }

    fn location_fact(&self, prefix: &str) -> DomainFact {
        // Custom endpoints can alias one store, so the scope is never comparable.
        DomainFact::RootLocation {
            provider: StorageBackend::S3,
            tuple: vec![
                ("endpoint".to_string(), self.config.endpoint_url.clone()),
                ("bucket".to_string(), self.config.bucket.clone()),
                (
                    "prefix".to_string(),
                    if prefix.is_empty() {
                        "/".to_string()
                    } else {
                        prefix.to_string()
                    },
                ),
            ],
            scope: FactScope::Unspecified,
        }
    }

    async fn scan_ancestors(&self, prefix: &str, scan: AncestorScan) -> ScanResult {
        let deadline = ScanDeadline::after(scan.max_duration);
        let mut found = Vec::new();
        let mut completeness = ScanCompleteness::Complete;
        let mut components: Vec<&str> = if prefix.is_empty() {
            Vec::new()
        } else {
            prefix.split('/').collect()
        };
        let mut level = 0_usize;
        // The bucket root is the authority boundary.
        while !components.is_empty() {
            if level == scan.max_depth {
                completeness = ScanCompleteness::Truncated {
                    limit: "max_depth".to_string(),
                };
                break;
            }
            level += 1;
            components.pop();
            let Some(state) = deadline
                .bound(self.read_marker(&marker_key_in(&components.join("/"))))
                .await
            else {
                completeness = ScanCompleteness::Truncated {
                    limit: "max_duration".to_string(),
                };
                break;
            };
            if let NamespaceState::Unknown { reason } = &state {
                completeness = ScanCompleteness::Denied {
                    reason: reason.clone(),
                };
            }
            if state != NamespaceState::Uninitialised {
                found.push(FoundNamespace {
                    relative_path: vec![".."; level].join("/"),
                    state,
                });
            }
        }
        ScanResult {
            completeness,
            found,
        }
    }

    /// Bounded nested scan: every await is bounded by the remaining original
    /// deadline, and pages, keys, depth, bytes and results are all budgeted.
    async fn scan_nested(&self, prefix: &str, scan: NestedScan) -> ScanResult {
        let deadline = ScanDeadline::after(scan.max_duration);
        let own_marker = marker_key_in(prefix);
        let marker_suffix = format!("/{NAMESPACE_MARKER}");
        let list_prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let truncated = |limit: &str| ScanCompleteness::Truncated {
            limit: limit.to_string(),
        };
        let mut found = Vec::new();
        let mut completeness = ScanCompleteness::Complete;
        let mut continuation = None;
        let (mut pages, mut keys, mut bytes) = (0_usize, 0_usize, 0_usize);
        'scan: loop {
            pages += 1;
            if pages > scan.max_pages {
                completeness = truncated("max_pages");
                break;
            }
            let listing = deadline
                .bound(
                    self.client
                        .list_objects_v2()
                        .bucket(&self.config.bucket)
                        .prefix(&list_prefix)
                        .set_continuation_token(continuation.take())
                        .send(),
                )
                .await;
            let output = match listing {
                None => {
                    completeness = truncated("max_duration");
                    break;
                }
                Some(Ok(output)) => output,
                Some(Err(error)) => {
                    let reason = if s3_is_denied(&error) {
                        "listing denied"
                    } else {
                        "listing failed"
                    };
                    completeness = ScanCompleteness::Denied {
                        reason: reason.to_string(),
                    };
                    break;
                }
            };
            for object in output.contents() {
                keys += 1;
                if keys > scan.max_keys {
                    completeness = truncated("max_keys");
                    break 'scan;
                }
                let Some(key) = object.key() else { continue };
                if key == own_marker || !key.ends_with(&marker_suffix) {
                    continue;
                }
                let relative = key
                    .strip_prefix(&list_prefix)
                    .unwrap_or(key)
                    .strip_suffix(&marker_suffix)
                    .unwrap_or_default()
                    .to_string();
                if relative.split('/').count() > scan.max_depth {
                    completeness = truncated("max_depth");
                    continue;
                }
                bytes = bytes
                    .saturating_add(relative.len())
                    .saturating_add(MAX_NAMESPACE_MARKER_BYTES);
                if bytes > scan.max_bytes {
                    completeness = truncated("max_bytes");
                    break 'scan;
                }
                if found.len() == scan.max_results {
                    completeness = truncated("max_results");
                    break 'scan;
                }
                let Some(state) = deadline.bound(self.read_marker(key)).await else {
                    completeness = truncated("max_duration");
                    break 'scan;
                };
                if let NamespaceState::Unknown { reason } = &state {
                    completeness = ScanCompleteness::Denied {
                        reason: reason.clone(),
                    };
                }
                found.push(FoundNamespace {
                    relative_path: relative,
                    state,
                });
            }
            match output.next_continuation_token() {
                Some(token) => continuation = Some(token.to_string()),
                None => break,
            }
        }
        found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        ScanResult {
            completeness,
            found,
        }
    }

    /// Configuration-only fence: endpoint aliasing or bucket recreation cannot
    /// be detected by a generic S3 endpoint.
    async fn check_fence(&self, key: &str, fence: &NamespaceFence) -> Result<(), StorageError> {
        self.provider_key(key)?;
        require_key_in_target(key, &fence.target)?;
        if fence.epoch.backend_instance != self.instance {
            return Err(fence_invalidated(key, "different backend instance"));
        }
        let marker = marker_key_in(&self.namespace_prefix(&fence.target)?);
        match self.read_marker(&marker).await {
            NamespaceState::Initialised { id } if id == fence.namespace => Ok(()),
            _ => Err(fence_invalidated(key, "namespace marker changed")),
        }
    }
}

#[async_trait]
impl BlobStore for S3StorageBackend {
    fn observation_epoch(&self) -> ObservationEpoch {
        ObservationEpoch {
            backend_instance: self.instance,
            connection_generation: 0,
            root_witness: None,
        }
    }

    async fn observe_namespace(
        &self,
        target: &NamespaceTarget,
        scan: &TopologyScan,
    ) -> Result<NamespaceObservation, StorageError> {
        scan.validate()?;
        let observed_at = OffsetDateTime::now_utc();
        let prefix = self.namespace_prefix(target)?;
        let state = self.read_marker(&marker_key_in(&prefix)).await;
        let ancestors = match scan.ancestors {
            Some(ancestors) => self.scan_ancestors(&prefix, ancestors).await,
            None => ScanResult::not_requested(),
        };
        let nested = match scan.nested {
            Some(nested) => self.scan_nested(&prefix, nested).await,
            None => ScanResult::not_requested(),
        };
        Ok(NamespaceObservation {
            target: target.clone(),
            state,
            root_location: Some(self.location_fact(&prefix)),
            ancestors,
            nested,
            epoch: self.observation_epoch(),
            observed_at,
        })
    }

    async fn initialise_namespace(
        &self,
        target: &NamespaceTarget,
        proposed: NamespaceId,
    ) -> Result<NamespaceInitialisation, StorageError> {
        let key = marker_key_in(&self.namespace_prefix(target)?);
        let settled = |state: NamespaceState| match state {
            NamespaceState::Initialised { id } if id == proposed => {
                Some(NamespaceInitialisation::AlreadyInitialisedSame(id))
            }
            NamespaceState::Initialised { id } => {
                Some(NamespaceInitialisation::AlreadyInitialisedOther(id))
            }
            NamespaceState::Conflict { reason } => {
                Some(NamespaceInitialisation::Conflict { reason })
            }
            _ => None,
        };
        let existing = self.read_marker(&key).await;
        if let NamespaceState::Unknown { reason } = &existing {
            return Err(StorageError::PreconditionFailed {
                key: NAMESPACE_MARKER.to_string(),
                condition: format!("marker state unknown: {reason}"),
            });
        }
        if let Some(result) = settled(existing) {
            return Ok(result);
        }
        let put = self
            .client
            .put_object()
            .bucket(&self.config.bucket)
            .key(&key)
            .content_type("application/json")
            .if_none_match("*")
            .body(ByteStream::from(Bytes::from(encode_marker(proposed))))
            .send()
            .await;
        match put {
            Ok(_) => {}
            // Another writer won; read back and report what is there.
            Err(error) if s3_is_precondition(&error) => {}
            // Outcome unknown: the caller retries with the SAME identity.
            Err(error) => return Err(map_s3_error(error)),
        }
        match self.read_marker(&key).await {
            NamespaceState::Initialised { id } if id == proposed => {
                Ok(NamespaceInitialisation::Created(id))
            }
            other => Ok(settled(other).unwrap_or(NamespaceInitialisation::Conflict {
                reason: "published marker did not read back".to_string(),
            })),
        }
    }

    async fn observe_capacity_domain(&self) -> Result<CapacityDomainObservation, StorageError> {
        Ok(CapacityDomainObservation {
            facts: Vec::new(),
            state: DomainState::Unknown {
                reason: "a generic S3 endpoint reports no physical or quota domain facts"
                    .to_string(),
            },
            epoch: self.observation_epoch(),
            observed_at: OffsetDateTime::now_utc(),
        })
    }

    async fn put_blob_fenced(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
        fence: &NamespaceFence,
    ) -> Result<BlobWriteOutcome, StorageError> {
        self.check_fence(key, fence).await?;
        // One request per mutation after the single fence check; never resent.
        self.put_blob_inner(key, body, options, false, Attempt::Single)
            .await?
            .ok_or_else(|| StorageError::PreconditionFailed {
                key: key.to_string(),
                condition: "blob unexpectedly already exists".to_string(),
            })
    }

    async fn put_blob_if_not_exists_fenced(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
        fence: &NamespaceFence,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        // If-None-Match create after a configuration-only fence check. Every
        // mutation of the lifecycle is one request with SDK retries disabled.
        self.check_fence(key, fence).await?;
        self.put_blob_inner(key, body, options, true, Attempt::Single)
            .await
    }

    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError> {
        let provider_key = self.provider_key(key)?;
        self.client
            .delete_object()
            .bucket(&self.config.bucket)
            .key(provider_key)
            .customize()
            .config_override(single_attempt_config())
            .send()
            .await
            .map_err(|error| StorageError::single_attempt(map_s3_error(error)))?;
        Ok(())
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::S3
    }

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        self.put_blob_inner(key, body, options, false, Attempt::Ordinary)
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
        self.put_blob_inner(key, body, options, true, Attempt::Ordinary)
            .await
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
            .filter(|key| !key.is_empty() && !is_reserved_key(key, S3_KEY_EQUIVALENCE))
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
