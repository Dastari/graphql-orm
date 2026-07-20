//! Native SMB2/SMB3 blob storage.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use smb::{
    Client, ClientConfig, ConnectionConfig, CreateOptions, Dialect, DirAccessMask, Directory, File,
    FileAccessMask, FileAttributes, FileCreateArgs, FileDispositionInformation,
    FileFullDirectoryInformation, FileRenameInformation, FileStandardInformation, Resource, Status,
    UncPath, connection::EncryptionMode,
};
use smb_dtyp::binrw_util::sized_wide_string::SizedWideString;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use uuid::Uuid;

use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    StorageBackend, StorageByteStream, StorageError, StorageProviderErrorKind, validate_blob_key,
};

const DEFAULT_PORT: u16 = 445;
const DEFAULT_TRANSFER_CONCURRENCY: usize = 8;
const DEFAULT_DIRECTORY_CACHE_CAPACITY: usize = 4_096;
const READ_CHUNK_SIZE: usize = 256 * 1024;
// SMB2 MaxWriteSize is the maximum WRITE payload and excludes SMB framing,
// signing, and encryption overhead. Limit smb-rs requests to one MiB anyway,
// and align larger payloads to the 64-KiB SMB2 credit unit. This is deliberately
// conservative for servers and transports that advertise multi-credit writes
// but reject large signed or encrypted frames in practice.
const CONSERVATIVE_WRITE_REQUEST_CAP: usize = 1024 * 1024;
const SMB2_CREDIT_UNIT: usize = 64 * 1024;
const WRITE_PROGRESS_INTERVAL: u64 = 64 * 1024 * 1024;
const TEMP_SUFFIX: &str = ".uploading";

/// Lowest SMB2/SMB3 dialect accepted by a native SMB backend.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SmbDialect {
    /// SMB 2.0.2.
    Smb2_0_2,
    /// SMB 2.1.
    Smb2_1,
    /// SMB 3.0.
    #[default]
    Smb3_0,
    /// SMB 3.0.2.
    Smb3_0_2,
    /// SMB 3.1.1.
    Smb3_1_1,
}

impl SmbDialect {
    fn protocol(self) -> Dialect {
        match self {
            Self::Smb2_0_2 => Dialect::Smb0202,
            Self::Smb2_1 => Dialect::Smb021,
            Self::Smb3_0 => Dialect::Smb030,
            Self::Smb3_0_2 => Dialect::Smb0302,
            Self::Smb3_1_1 => Dialect::Smb0311,
        }
    }

    fn from_protocol(value: Dialect) -> Self {
        match value {
            Dialect::Smb0202 => Self::Smb2_0_2,
            Dialect::Smb021 => Self::Smb2_1,
            Dialect::Smb030 => Self::Smb3_0,
            Dialect::Smb0302 => Self::Smb3_0_2,
            Dialect::Smb0311 => Self::Smb3_1_1,
        }
    }
}

/// Runtime configuration for a native SMB2/SMB3 storage connection.
#[derive(Clone)]
pub struct SmbStorageConfig {
    /// DNS name or IP address. UNC URLs and local paths are rejected.
    pub server: String,
    /// TCP port, normally 445.
    pub port: u16,
    /// SMB share name without separators.
    pub share: String,
    /// Optional provider root expressed as a safe blob-key prefix.
    pub root_prefix: Option<String>,
    /// Username used for NTLM authentication.
    pub username: String,
    /// Password used for NTLM authentication. Debug output is redacted.
    pub password: SecretString,
    /// Optional NTLM domain or workgroup.
    pub domain: Option<String>,
    /// Lowest accepted dialect. SMB1 is never offered.
    pub min_dialect: SmbDialect,
    /// Require signed authenticated SMB messages.
    pub require_signing: bool,
    /// Require SMB3 encryption for the session.
    pub require_encryption: bool,
    /// DNS/TCP connection deadline.
    pub connect_timeout: Duration,
    /// Deadline for each storage operation.
    pub operation_timeout: Duration,
    /// Maximum number of simultaneous streaming transfers.
    pub max_transfer_concurrency: usize,
}

impl SmbStorageConfig {
    /// Creates a configuration with secure defaults for port, dialect, and signing.
    #[must_use]
    pub fn new(
        server: impl Into<String>,
        share: impl Into<String>,
        username: impl Into<String>,
        password: SecretString,
    ) -> Self {
        Self {
            server: server.into(),
            port: DEFAULT_PORT,
            share: share.into(),
            root_prefix: None,
            username: username.into(),
            password,
            domain: None,
            min_dialect: SmbDialect::default(),
            require_signing: true,
            require_encryption: false,
            connect_timeout: Duration::from_secs(10),
            operation_timeout: Duration::from_secs(60),
            max_transfer_concurrency: DEFAULT_TRANSFER_CONCURRENCY,
        }
    }

    /// Validates all non-secret fields and ensures explicit credentials exist.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when a field is empty, path-like, unsafe, or
    /// inconsistent with the requested security policy.
    pub fn validate(&self) -> Result<(), StorageError> {
        validate_server(&self.server)?;
        validate_component("share", &self.share)?;
        validate_component("username", &self.username)?;
        if self.password.expose_secret().is_empty() {
            return Err(invalid_config("password must not be empty"));
        }
        if let Some(domain) = &self.domain {
            validate_component("domain", domain)?;
        }
        if let Some(prefix) = &self.root_prefix {
            validate_blob_key(prefix)?;
        }
        if self.port == 0 {
            return Err(invalid_config("port must be greater than zero"));
        }
        if self.connect_timeout.is_zero() || self.operation_timeout.is_zero() {
            return Err(invalid_config("timeouts must be greater than zero"));
        }
        if self.max_transfer_concurrency == 0 {
            return Err(invalid_config(
                "max transfer concurrency must be greater than zero",
            ));
        }
        if self.require_encryption && self.min_dialect < SmbDialect::Smb3_0 {
            return Err(invalid_config(
                "SMB3 encryption requires a minimum dialect of SMB 3.0",
            ));
        }
        Ok(())
    }

    fn authenticated_username(&self) -> String {
        match self.domain.as_deref() {
            Some(domain) => format!(r"{domain}\{}", self.username),
            None => self.username.clone(),
        }
    }
}

impl fmt::Debug for SmbStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmbStorageConfig")
            .field("server", &self.server)
            .field("port", &self.port)
            .field("share", &self.share)
            .field("root_prefix", &self.root_prefix)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("domain", &self.domain)
            .field("min_dialect", &self.min_dialect)
            .field("require_signing", &self.require_signing)
            .field("require_encryption", &self.require_encryption)
            .field("connect_timeout", &self.connect_timeout)
            .field("operation_timeout", &self.operation_timeout)
            .field("max_transfer_concurrency", &self.max_transfer_concurrency)
            .finish()
    }
}

/// Options controlling whether a probe may create the configured root prefix.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SmbProbeOptions {
    /// Create missing root-prefix directories before testing read/write access.
    pub create_prefix: bool,
}

/// Redaction-safe diagnostic result returned by [`SmbStorageBackend::probe`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SmbProbeResult {
    /// Dialect selected by the server.
    pub negotiated_dialect: SmbDialect,
    /// Whether signing is active for the authenticated session.
    pub signing_active: bool,
    /// Whether encryption is required and active for this connection.
    pub encryption_active: bool,
    /// Whether server resolution and connection succeeded.
    pub server_reachable: bool,
    /// Whether authentication and tree connection succeeded.
    pub share_reachable: bool,
    /// Whether the repository prefix could be listed.
    pub prefix_readable: bool,
    /// Whether a random probe file round-tripped and was deleted.
    pub prefix_writable: bool,
    /// Maximum SMB2 READ payload accepted by the negotiated connection.
    pub max_read_size: u32,
    /// Maximum SMB2 WRITE payload accepted by the negotiated connection.
    pub max_write_size: u32,
    /// Maximum SMB2 transaction buffer accepted by the negotiated connection.
    pub max_transact_size: u32,
    /// Conservative payload limit used for each WRITE request by this backend.
    pub write_request_payload_limit: usize,
}

/// Redaction-safe counters for monitoring long-running SMB transfers.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SmbBackendDiagnostics {
    /// Current client generation. This increases after each successful reconnect.
    pub connection_generation: u64,
    /// Conservative payload limit currently used for each SMB WRITE request.
    pub write_request_payload_limit: usize,
    /// Upload operations currently running or waiting for a transfer permit.
    pub active_uploads: u64,
    /// Upload operations started since this backend was connected.
    pub uploads_started: u64,
    /// Upload operations completed successfully.
    pub uploads_completed: u64,
    /// Upload operations that failed or were cancelled.
    pub uploads_failed: u64,
    /// Bytes acknowledged by SMB WRITE responses.
    pub bytes_uploaded: u64,
    /// SMB WRITE requests issued.
    pub write_requests: u64,
    /// SMB WRITE responses that acknowledged only part of the request.
    pub partial_write_responses: u64,
    /// Successful reconnects.
    pub reconnects: u64,
    /// Current number of cached directories known to exist.
    pub directory_cache_entries: usize,
    /// Parent-directory lookups satisfied without a remote CREATE request.
    pub directory_cache_hits: u64,
    /// Parent-directory lookups that required coordination.
    pub directory_cache_misses: u64,
    /// Remote CREATE requests issued for directories.
    pub directory_create_requests: u64,
    /// Directory requests that joined another in-flight creation.
    pub directory_coalesced_waits: u64,
}

#[derive(Default)]
struct SmbDiagnosticCounters {
    active_uploads: AtomicU64,
    uploads_started: AtomicU64,
    uploads_completed: AtomicU64,
    uploads_failed: AtomicU64,
    bytes_uploaded: AtomicU64,
    write_requests: AtomicU64,
    partial_write_responses: AtomicU64,
    reconnects: AtomicU64,
    directory_cache_hits: AtomicU64,
    directory_cache_misses: AtomicU64,
    directory_create_requests: AtomicU64,
    directory_coalesced_waits: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
struct SmbConnectionProperties {
    negotiated_dialect: SmbDialect,
    signing_active: bool,
    encryption_active: bool,
    max_read_size: u32,
    max_write_size: u32,
    max_transact_size: u32,
    write_request_payload_limit: usize,
}

struct ConnectedClient {
    client: Arc<Client>,
    generation: u64,
    properties: SmbConnectionProperties,
}

struct DirectoryCacheState {
    known: HashSet<String>,
    insertion_order: VecDeque<String>,
    gates: HashMap<String, Weak<Semaphore>>,
}

struct DirectoryCache {
    capacity: usize,
    gate_capacity: usize,
    state: Mutex<DirectoryCacheState>,
}

impl DirectoryCache {
    fn new(capacity: usize, max_concurrency: usize) -> Self {
        Self {
            capacity,
            gate_capacity: capacity.max(max_concurrency),
            state: Mutex::new(DirectoryCacheState {
                known: HashSet::with_capacity(capacity),
                insertion_order: VecDeque::with_capacity(capacity),
                gates: HashMap::new(),
            }),
        }
    }

    async fn contains(&self, path: &str) -> bool {
        self.state.lock().await.known.contains(path)
    }

    async fn insert(&self, path: String) {
        let mut state = self.state.lock().await;
        if !state.known.insert(path.clone()) {
            return;
        }
        state.insertion_order.push_back(path);
        while state.known.len() > self.capacity {
            if let Some(expired) = state.insertion_order.pop_front() {
                state.known.remove(&expired);
            }
        }
    }

    async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.known.clear();
        state.insertion_order.clear();
    }

    async fn len(&self) -> usize {
        self.state.lock().await.known.len()
    }

    async fn gate(&self, path: &str) -> (Arc<Semaphore>, bool) {
        let mut state = self.state.lock().await;
        state.gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = state.gates.get(path).and_then(Weak::upgrade) {
            let busy = gate.available_permits() == 0;
            return (gate, busy);
        }

        let gate = Arc::new(Semaphore::new(1));
        if state.gates.len() < self.gate_capacity {
            state.gates.insert(path.to_string(), Arc::downgrade(&gate));
        }
        (gate, false)
    }
}

struct SmbStorageBackendInner {
    config: Arc<SmbStorageConfig>,
    client: RwLock<ConnectedClient>,
    reconnect_gate: Semaphore,
    transfer_limit: Arc<Semaphore>,
    directories: DirectoryCache,
    diagnostics: SmbDiagnosticCounters,
}

/// Native SMB2/SMB3 [`BlobStore`] implementation.
#[derive(Clone)]
pub struct SmbStorageBackend {
    inner: Arc<SmbStorageBackendInner>,
}

impl fmt::Debug for SmbStorageBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmbStorageBackend")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl SmbStorageBackend {
    /// Validates the configuration, connects, authenticates, and opens the share.
    ///
    /// # Errors
    ///
    /// Returns a structured [`StorageError`] for validation, connectivity,
    /// authentication, security negotiation, or share failures.
    pub async fn connect(config: SmbStorageConfig) -> Result<Self, StorageError> {
        config.validate()?;
        let transfer_concurrency = config.max_transfer_concurrency;
        let config = Arc::new(config);
        let client = connect_client(&config, 0).await?;
        tracing::info!(
            target: "graphql_orm_storage::smb",
            event = "connected",
            generation = client.generation,
            dialect = ?client.properties.negotiated_dialect,
            signing_active = client.properties.signing_active,
            encryption_active = client.properties.encryption_active,
            max_read_size = client.properties.max_read_size,
            max_write_size = client.properties.max_write_size,
            max_transact_size = client.properties.max_transact_size,
            write_request_payload_limit = client.properties.write_request_payload_limit,
        );
        Ok(Self {
            inner: Arc::new(SmbStorageBackendInner {
                config,
                client: RwLock::new(client),
                reconnect_gate: Semaphore::new(1),
                transfer_limit: Arc::new(Semaphore::new(transfer_concurrency)),
                directories: DirectoryCache::new(
                    DEFAULT_DIRECTORY_CACHE_CAPACITY,
                    transfer_concurrency,
                ),
                diagnostics: SmbDiagnosticCounters::default(),
            }),
        })
    }

    /// Returns redaction-safe counters for transfer progress and provider health.
    pub async fn diagnostics(&self) -> SmbBackendDiagnostics {
        let client = self.inner.client.read().await;
        let connection_generation = client.generation;
        let write_request_payload_limit = client.properties.write_request_payload_limit;
        drop(client);
        SmbBackendDiagnostics {
            connection_generation,
            write_request_payload_limit,
            active_uploads: self
                .inner
                .diagnostics
                .active_uploads
                .load(Ordering::Relaxed),
            uploads_started: self
                .inner
                .diagnostics
                .uploads_started
                .load(Ordering::Relaxed),
            uploads_completed: self
                .inner
                .diagnostics
                .uploads_completed
                .load(Ordering::Relaxed),
            uploads_failed: self
                .inner
                .diagnostics
                .uploads_failed
                .load(Ordering::Relaxed),
            bytes_uploaded: self
                .inner
                .diagnostics
                .bytes_uploaded
                .load(Ordering::Relaxed),
            write_requests: self
                .inner
                .diagnostics
                .write_requests
                .load(Ordering::Relaxed),
            partial_write_responses: self
                .inner
                .diagnostics
                .partial_write_responses
                .load(Ordering::Relaxed),
            reconnects: self.inner.diagnostics.reconnects.load(Ordering::Relaxed),
            directory_cache_entries: self.inner.directories.len().await,
            directory_cache_hits: self
                .inner
                .diagnostics
                .directory_cache_hits
                .load(Ordering::Relaxed),
            directory_cache_misses: self
                .inner
                .diagnostics
                .directory_cache_misses
                .load(Ordering::Relaxed),
            directory_create_requests: self
                .inner
                .diagnostics
                .directory_create_requests
                .load(Ordering::Relaxed),
            directory_coalesced_waits: self
                .inner
                .diagnostics
                .directory_coalesced_waits
                .load(Ordering::Relaxed),
        }
    }

    /// Performs a destructive random-file round trip and returns safe diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a structured, redaction-safe error if any probe stage fails.
    pub async fn probe(
        config: SmbStorageConfig,
        options: SmbProbeOptions,
    ) -> Result<SmbProbeResult, StorageError> {
        let backend = Self::connect(config).await?;
        if options.create_prefix {
            backend.ensure_root_prefix().await?;
        }
        let prefix_readable = backend.list_blobs("").await.is_ok();
        let key = format!(".graphql-orm-probe-{}", Uuid::new_v4());
        let expected = Bytes::copy_from_slice(Uuid::new_v4().as_bytes());
        backend
            .put_blob(
                &key,
                StorageByteStream::from_bytes(expected.clone()),
                BlobPutOptions::default(),
            )
            .await?;
        let loaded = backend.get_blob(&key).await?;
        let actual = crate::collect_storage_stream(loaded.body).await?;
        if actual != expected {
            let _ = backend.delete_blob(&key).await;
            return Err(remote_error(
                StorageProviderErrorKind::Protocol,
                "probe",
                "probe file contents did not match",
                false,
            ));
        }
        backend.delete_blob(&key).await?;

        let snapshot = backend.client_snapshot().await;
        let info = snapshot.properties;

        Ok(SmbProbeResult {
            negotiated_dialect: info.negotiated_dialect,
            signing_active: info.signing_active,
            encryption_active: info.encryption_active,
            server_reachable: true,
            share_reachable: true,
            prefix_readable,
            prefix_writable: true,
            max_read_size: info.max_read_size,
            max_write_size: info.max_write_size,
            max_transact_size: info.max_transact_size,
            write_request_payload_limit: info.write_request_payload_limit,
        })
    }

    async fn client_snapshot(&self) -> ConnectedClient {
        let client = self.inner.client.read().await;
        ConnectedClient {
            client: Arc::clone(&client.client),
            generation: client.generation,
            properties: client.properties,
        }
    }

    async fn reconnect_after(&self, failed_generation: u64) -> Result<(), StorageError> {
        let _permit = self.inner.reconnect_gate.acquire().await.map_err(|_| {
            remote_error(
                StorageProviderErrorKind::Protocol,
                "reconnect",
                "reconnect coordinator is closed",
                false,
            )
        })?;
        if self.inner.client.read().await.generation != failed_generation {
            return Ok(());
        }

        let delays = [100_u64, 250, 500];
        let mut last_error = None;
        for delay in delays {
            let jitter = u64::from(Uuid::new_v4().as_bytes()[0]) % 75;
            tokio::time::sleep(Duration::from_millis(delay + jitter)).await;
            let generation = failed_generation.saturating_add(1);
            match connect_client(&self.inner.config, generation).await {
                Ok(client) => {
                    let properties = client.properties;
                    *self.inner.client.write().await = client;
                    self.inner.directories.clear().await;
                    self.inner
                        .diagnostics
                        .reconnects
                        .fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        target: "graphql_orm_storage::smb",
                        event = "reconnected",
                        generation,
                        dialect = ?properties.negotiated_dialect,
                        max_write_size = properties.max_write_size,
                        write_request_payload_limit = properties.write_request_payload_limit,
                    );
                    return Ok(());
                }
                Err(error) => {
                    if !error.is_retryable() {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            remote_error(
                StorageProviderErrorKind::ConnectionLost,
                "reconnect",
                "reconnect attempts were exhausted",
                true,
            )
        }))
    }

    fn share_path(&self) -> Result<UncPath, StorageError> {
        UncPath::new(&self.inner.config.server)
            .and_then(|path| path.with_share(&self.inner.config.share))
            .map_err(|error| map_smb_error("path", error))
    }

    fn mapped_key(&self, key: &str) -> Result<String, StorageError> {
        validate_blob_key(key)?;
        Ok(match &self.inner.config.root_prefix {
            Some(prefix) => format!("{prefix}/{key}"),
            None => key.to_string(),
        })
    }

    fn remote_path(&self, key: &str) -> Result<UncPath, StorageError> {
        Ok(self.share_path()?.with_path(&self.mapped_key(key)?))
    }

    async fn ensure_root_prefix(&self) -> Result<(), StorageError> {
        if let Some(prefix) = &self.inner.config.root_prefix {
            self.ensure_directories(prefix).await?;
        }
        Ok(())
    }

    async fn ensure_parent_directories(&self, key: &str) -> Result<(), StorageError> {
        let mapped = self.mapped_key(key)?;
        let Some((parent, _)) = mapped.rsplit_once('/') else {
            return Ok(());
        };
        self.ensure_directories(parent).await
    }

    async fn ensure_directories(&self, path: &str) -> Result<(), StorageError> {
        if self.inner.directories.contains(path).await {
            self.inner
                .diagnostics
                .directory_cache_hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        for pass in 0..2 {
            let mut current = String::new();
            let mut retry_from_root = false;
            for segment in path.split('/') {
                if !current.is_empty() {
                    current.push('/');
                }
                current.push_str(segment);
                match self.ensure_one_directory(&current).await {
                    Ok(()) => {}
                    Err(error) if pass == 0 && is_not_found(&error) => {
                        self.inner.directories.clear().await;
                        retry_from_root = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if !retry_from_root {
                return Ok(());
            }
        }
        Err(remote_error(
            StorageProviderErrorKind::RemotePathNotFound,
            "create_directory",
            "parent directory disappeared while it was being created",
            false,
        ))
    }

    async fn ensure_one_directory(&self, path: &str) -> Result<(), StorageError> {
        if self.inner.directories.contains(path).await {
            self.inner
                .diagnostics
                .directory_cache_hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        self.inner
            .diagnostics
            .directory_cache_misses
            .fetch_add(1, Ordering::Relaxed);

        let (gate, busy) = self.inner.directories.gate(path).await;
        if busy {
            self.inner
                .diagnostics
                .directory_coalesced_waits
                .fetch_add(1, Ordering::Relaxed);
        }
        let _permit = gate.acquire_owned().await.map_err(|_| {
            remote_error(
                StorageProviderErrorKind::Protocol,
                "create_directory",
                "directory coordinator is closed",
                false,
            )
        })?;
        if self.inner.directories.contains(path).await {
            self.inner
                .diagnostics
                .directory_cache_hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        let target = self.share_path()?.with_path(path);
        let create = FileCreateArgs {
            disposition: smb::CreateDisposition::OpenIf,
            attributes: FileAttributes::new().with_directory(true),
            options: CreateOptions::new().with_directory_file(true),
            desired_access: DirAccessMask::new()
                .with_list_directory(true)
                .with_synchronize(true)
                .into(),
        };
        for attempt in 0..2 {
            let snapshot = self.client_snapshot().await;
            self.inner
                .diagnostics
                .directory_create_requests
                .fetch_add(1, Ordering::Relaxed);
            let result = self
                .with_timeout(
                    "create_directory",
                    snapshot.client.create_file(&target, &create),
                )
                .await;
            match result {
                Ok(resource) => {
                    let close_result = self.with_timeout("close", close_resource(resource)).await;
                    match close_result {
                        Ok(()) => {
                            self.inner.directories.insert(path.to_string()).await;
                            return Ok(());
                        }
                        Err(error) if error.is_retryable() => {
                            self.reconnect_after(snapshot.generation).await?;
                            if attempt == 1 {
                                return Err(error);
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) if error.is_retryable() => {
                    self.reconnect_after(snapshot.generation).await?;
                    if attempt == 1 {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("directory create loop always returns")
    }

    async fn with_timeout<T>(
        &self,
        operation: &'static str,
        future: impl std::future::Future<Output = smb::Result<T>>,
    ) -> Result<T, StorageError> {
        tokio::time::timeout(self.inner.config.operation_timeout, future)
            .await
            .map_err(|_| {
                remote_error(
                    StorageProviderErrorKind::Timeout,
                    operation,
                    "operation timed out",
                    true,
                )
            })?
            .map_err(|error| map_smb_error(operation, error))
    }

    async fn with_io_timeout<T>(
        &self,
        operation: &'static str,
        future: impl std::future::Future<Output = std::io::Result<T>>,
    ) -> Result<T, StorageError> {
        tokio::time::timeout(self.inner.config.operation_timeout, future)
            .await
            .map_err(|_| {
                remote_error(
                    StorageProviderErrorKind::Timeout,
                    operation,
                    "operation timed out",
                    true,
                )
            })?
            .map_err(|error| map_io_error(operation, error))
    }

    async fn open_file(&self, key: &str, access: FileAccessMask) -> Result<File, StorageError> {
        let path = self.remote_path(key)?;
        let snapshot = self.client_snapshot().await;
        let first = self
            .with_timeout(
                "open",
                snapshot
                    .client
                    .create_file(&path, &FileCreateArgs::make_open_existing(access)),
            )
            .await;
        let resource = match first {
            Ok(resource) => resource,
            Err(error) if error.is_retryable() => {
                self.reconnect_after(snapshot.generation).await?;
                let retry = self.client_snapshot().await;
                let result = self
                    .with_timeout(
                        "open",
                        retry
                            .client
                            .create_file(&path, &FileCreateArgs::make_open_existing(access)),
                    )
                    .await;
                if let Err(retry_error) = &result
                    && retry_error.is_retryable()
                {
                    let _ = self.reconnect_after(retry.generation).await;
                }
                result?
            }
            Err(error) => return Err(error),
        };
        match resource {
            Resource::File(file) => Ok(file),
            other => {
                let _ = close_resource(other).await;
                Err(remote_error(
                    StorageProviderErrorKind::Protocol,
                    "open",
                    "remote key is not a file",
                    false,
                ))
            }
        }
    }

    async fn write_file(
        &self,
        key: &str,
        body: StorageByteStream,
        exclusive: bool,
    ) -> Result<BlobWriteOutcome, StorageError> {
        let mut upload = UploadDiagnosticGuard::new(Arc::clone(&self.inner));
        let _permit = self.transfer_permit().await?;
        let args = write_create_args(exclusive);
        let (file, generation, request_limit) =
            self.create_write_file(key, &args, !exclusive).await?;
        let mut cleanup = WriteCleanupGuard::new(self.clone(), key.to_string(), file, generation);

        // Delete-on-close makes cancellation safe after CREATE succeeds. The flag
        // is cleared only after the complete stream has been flushed.
        if let Err(error) = self
            .with_timeout(
                "arm_write_cleanup",
                cleanup
                    .file()
                    .set_info(FileDispositionInformation::default()),
            )
            .await
        {
            if error.is_retryable() {
                let _ = self.reconnect_after(generation).await;
            }
            cleanup.cleanup().await;
            return Err(error);
        }

        tracing::info!(
            target: "graphql_orm_storage::smb",
            event = "upload_started",
            key,
            conditional = exclusive,
            size_hint = body.size_hint(),
            write_request_payload_limit = request_limit,
        );
        let writer = TimedSmbBlockWriter {
            file: cleanup.file(),
            timeout: self.inner.config.operation_timeout,
        };
        let outcome = match write_stream_to_writer(
            &writer,
            body,
            request_limit,
            &self.inner.diagnostics,
            Some(key),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                if error.is_retryable() {
                    let _ = self.reconnect_after(generation).await;
                }
                cleanup.cleanup().await;
                tracing::warn!(
                    target: "graphql_orm_storage::smb",
                    event = "upload_failed",
                    key,
                    conditional = exclusive,
                    error_kind = ?provider_error_kind(&error),
                );
                return Err(error);
            }
        };

        if let Err(error) = self.with_io_timeout("flush", cleanup.file().flush()).await {
            if error.is_retryable() {
                let _ = self.reconnect_after(generation).await;
            }
            cleanup.cleanup().await;
            return Err(error);
        }
        if let Err(error) = self
            .with_timeout(
                "disarm_write_cleanup",
                cleanup.file().set_info(FileDispositionInformation {
                    delete_pending: false.into(),
                }),
            )
            .await
        {
            if error.is_retryable() {
                let _ = self.reconnect_after(generation).await;
            }
            cleanup.cleanup().await;
            return Err(error);
        }
        if let Err(error) = self.with_timeout("close", cleanup.file().close()).await {
            if error.is_retryable() {
                let _ = self.reconnect_after(generation).await;
            }
            cleanup.cleanup().await;
            return Err(error);
        }
        cleanup.disarm();
        upload.complete();
        tracing::info!(
            target: "graphql_orm_storage::smb",
            event = "upload_completed",
            key,
            conditional = exclusive,
            size_bytes = outcome.size_bytes,
        );
        Ok(outcome)
    }

    async fn create_write_file(
        &self,
        key: &str,
        args: &FileCreateArgs,
        replay_safe: bool,
    ) -> Result<(File, u64, usize), StorageError> {
        for path_attempt in 0..2 {
            self.ensure_parent_directories(key).await?;
            let path = self.remote_path(key)?;
            let snapshot = self.client_snapshot().await;
            let result = self
                .with_timeout("create", snapshot.client.create_file(&path, args))
                .await;
            match result {
                Ok(Resource::File(file)) => {
                    return Ok((
                        file,
                        snapshot.generation,
                        snapshot.properties.write_request_payload_limit,
                    ));
                }
                Ok(other) => {
                    let _ = self.with_timeout("close", close_resource(other)).await;
                    return Err(remote_error(
                        StorageProviderErrorKind::Protocol,
                        "create",
                        "remote key did not create a file",
                        false,
                    ));
                }
                Err(error) if path_attempt == 0 && is_not_found(&error) => {
                    self.inner.directories.clear().await;
                }
                Err(error) if error.is_retryable() => {
                    self.reconnect_after(snapshot.generation).await?;
                    if replay_safe && path_attempt == 0 {
                        continue;
                    }
                    // An exclusive CREATE may have reached the server even when
                    // its response was lost. Replaying it could turn our own
                    // partial object into a false collision.
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(remote_error(
            StorageProviderErrorKind::RemotePathNotFound,
            "create",
            "parent directory remained unavailable after cache invalidation",
            false,
        ))
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), StorageError> {
        self.ensure_parent_directories(to).await?;
        let file = self
            .open_file(
                from,
                FileAccessMask::new()
                    .with_generic_read(true)
                    .with_delete(true),
            )
            .await?;
        let generation = self.client_snapshot().await.generation;
        let destination = self.mapped_key(to)?.replace('/', "\\");
        let result = self
            .with_timeout(
                "rename",
                file.set_info(FileRenameInformation {
                    replace_if_exists: replace.into(),
                    root_directory: 0,
                    file_name: SizedWideString::from(destination),
                }),
            )
            .await;
        let close_result = self.with_timeout("close", file.close()).await;
        let combined = result.and(close_result);
        if let Err(error) = &combined
            && error.is_retryable()
        {
            let _ = self.reconnect_after(generation).await;
        }
        combined
    }

    async fn remove_file(&self, key: &str) -> Result<(), StorageError> {
        for attempt in 0..2 {
            let file = match self
                .open_file(key, FileAccessMask::new().with_delete(true))
                .await
            {
                Ok(file) => file,
                Err(error) if is_not_found(&error) => return Ok(()),
                Err(error) => return Err(error),
            };
            let generation = self.client_snapshot().await.generation;
            let result = self
                .with_timeout(
                    "delete",
                    file.set_info(FileDispositionInformation::default()),
                )
                .await;
            let close_result = self.with_timeout("close", file.close()).await;
            let combined = result.and(close_result);
            match combined {
                Ok(()) => return Ok(()),
                Err(error) if error.is_retryable() => {
                    self.reconnect_after(generation).await?;
                    if attempt == 1 {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn transfer_permit(&self) -> Result<OwnedSemaphorePermit, StorageError> {
        Arc::clone(&self.inner.transfer_limit)
            .acquire_owned()
            .await
            .map_err(|_| {
                remote_error(
                    StorageProviderErrorKind::Protocol,
                    "transfer",
                    "transfer limiter is closed",
                    false,
                )
            })
    }

    async fn collect_keys(&self) -> Result<Vec<String>, StorageError> {
        for attempt in 0..2 {
            let snapshot = self.client_snapshot().await;
            match self.collect_keys_once(&snapshot.client).await {
                Ok(keys) => return Ok(keys),
                Err(error) if error.is_retryable() => {
                    self.reconnect_after(snapshot.generation).await?;
                    if attempt == 1 {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(Vec::new())
    }

    async fn collect_keys_once(&self, client: &Client) -> Result<Vec<String>, StorageError> {
        let base = self.share_path()?;
        let start = self.inner.config.root_prefix.clone().unwrap_or_default();
        let mut stack = vec![start];
        let mut keys = Vec::new();

        while let Some(directory_path) = stack.pop() {
            let target = base.clone().with_path(&directory_path);
            let resource = match self
                .with_timeout(
                    "list",
                    client.create_file(
                        &target,
                        &FileCreateArgs::make_open_existing(
                            DirAccessMask::new()
                                .with_list_directory(true)
                                .with_synchronize(true)
                                .into(),
                        ),
                    ),
                )
                .await
            {
                Ok(resource) => resource,
                Err(error) if is_not_found(&error) => continue,
                Err(error) => return Err(error),
            };
            let directory = match resource {
                Resource::Directory(directory) => Arc::new(directory),
                other => {
                    let _ = close_resource(other).await;
                    continue;
                }
            };
            let mut entries = self
                .with_timeout(
                    "list",
                    Directory::query::<FileFullDirectoryInformation>(&directory, "*"),
                )
                .await?;
            loop {
                let entry =
                    tokio::time::timeout(self.inner.config.operation_timeout, entries.next())
                        .await
                        .map_err(|_| {
                            remote_error(
                                StorageProviderErrorKind::Timeout,
                                "list",
                                "operation timed out",
                                true,
                            )
                        })?;
                let Some(entry) = entry else {
                    break;
                };
                let entry = entry.map_err(|error| map_smb_error("list", error))?;
                let name = entry.file_name.to_string();
                if name == "." || name == ".." {
                    continue;
                }
                let child = if directory_path.is_empty() {
                    name
                } else {
                    format!("{directory_path}/{name}")
                };
                if entry.file_attributes.directory() {
                    stack.push(child);
                } else if !is_temp_key(&child) {
                    let key = match &self.inner.config.root_prefix {
                        Some(root) => child
                            .strip_prefix(root)
                            .and_then(|rest| rest.strip_prefix('/'))
                            .unwrap_or(&child)
                            .to_string(),
                        None => child,
                    };
                    keys.push(key);
                }
            }
            drop(entries);
            self.with_timeout("close", directory.close()).await?;
        }
        keys.sort();
        Ok(keys)
    }
}

struct UploadDiagnosticGuard {
    inner: Arc<SmbStorageBackendInner>,
    completed: bool,
}

impl UploadDiagnosticGuard {
    fn new(inner: Arc<SmbStorageBackendInner>) -> Self {
        inner
            .diagnostics
            .active_uploads
            .fetch_add(1, Ordering::Relaxed);
        inner
            .diagnostics
            .uploads_started
            .fetch_add(1, Ordering::Relaxed);
        Self {
            inner,
            completed: false,
        }
    }

    fn complete(&mut self) {
        self.completed = true;
        self.inner
            .diagnostics
            .uploads_completed
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .diagnostics
            .active_uploads
            .fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for UploadDiagnosticGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.inner
            .diagnostics
            .uploads_failed
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .diagnostics
            .active_uploads
            .fetch_sub(1, Ordering::Relaxed);
    }
}

struct WriteCleanupGuard {
    backend: SmbStorageBackend,
    key: String,
    file: Option<File>,
    generation: u64,
    armed: bool,
}

struct TempObjectCleanupGuard {
    backend: SmbStorageBackend,
    key: String,
    armed: bool,
}

impl TempObjectCleanupGuard {
    fn new(backend: SmbStorageBackend, key: String) -> Self {
        Self {
            backend,
            key,
            armed: true,
        }
    }

    async fn cleanup(&mut self) {
        if self.armed {
            let _ = self.backend.remove_file(&self.key).await;
            self.armed = false;
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempObjectCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let backend = self.backend.clone();
        let key = self.key.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = backend.remove_file(&key).await;
            });
        }
    }
}

impl WriteCleanupGuard {
    fn new(backend: SmbStorageBackend, key: String, file: File, generation: u64) -> Self {
        Self {
            backend,
            key,
            file: Some(file),
            generation,
            armed: true,
        }
    }

    fn file(&self) -> &File {
        self.file
            .as_ref()
            .expect("write cleanup guard must own a file while armed")
    }

    async fn cleanup(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(file) = &self.file {
            let close = self.backend.with_timeout("close", file.close()).await;
            if let Err(error) = close
                && error.is_retryable()
            {
                let _ = self.backend.reconnect_after(self.generation).await;
            }
        }
        let _ = self.backend.remove_file(&self.key).await;
        self.armed = false;
        self.file.take();
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.file.take();
    }
}

impl Drop for WriteCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(file) = self.file.take() else {
            return;
        };
        let backend = self.backend.clone();
        let key = self.key.clone();
        let generation = self.generation;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let close = backend.with_timeout("close", file.close()).await;
                if let Err(error) = close
                    && error.is_retryable()
                {
                    let _ = backend.reconnect_after(generation).await;
                }
                let _ = backend.remove_file(&key).await;
            });
        }
    }
}

#[async_trait]
trait SmbBlockWriter: Send + Sync {
    async fn write_block(&self, bytes: &[u8], offset: u64) -> Result<usize, StorageError>;
}

struct TimedSmbBlockWriter<'a> {
    file: &'a File,
    timeout: Duration,
}

#[async_trait]
impl SmbBlockWriter for TimedSmbBlockWriter<'_> {
    async fn write_block(&self, bytes: &[u8], offset: u64) -> Result<usize, StorageError> {
        tokio::time::timeout(self.timeout, self.file.write_block(bytes, offset, None))
            .await
            .map_err(|_| {
                remote_error(
                    StorageProviderErrorKind::Timeout,
                    "write",
                    "operation timed out",
                    true,
                )
            })?
            .map_err(|error| map_io_error("write", error))
    }
}

async fn write_stream_to_writer<W: SmbBlockWriter + ?Sized>(
    writer: &W,
    body: StorageByteStream,
    request_limit: usize,
    diagnostics: &SmbDiagnosticCounters,
    progress_key: Option<&str>,
) -> Result<BlobWriteOutcome, StorageError> {
    if request_limit == 0 {
        return Err(remote_error(
            StorageProviderErrorKind::Protocol,
            "write",
            "negotiated maximum write size is zero",
            false,
        ));
    }

    let started = Instant::now();
    let mut next_progress = WRITE_PROGRESS_INTERVAL;
    let mut source = body.into_inner();
    let mut offset = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) = source.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        let mut written = 0_usize;
        while written < chunk.len() {
            let request_end = written.saturating_add(request_limit).min(chunk.len());
            let request = &chunk[written..request_end];
            diagnostics.write_requests.fetch_add(1, Ordering::Relaxed);
            let count = writer.write_block(request, offset).await?;
            if count == 0 {
                return Err(remote_error(
                    StorageProviderErrorKind::Protocol,
                    "write",
                    "server accepted a zero-length write",
                    false,
                ));
            }
            if count > request.len() {
                return Err(remote_error(
                    StorageProviderErrorKind::Protocol,
                    "write",
                    "server acknowledged more bytes than requested",
                    false,
                ));
            }
            if count < request.len() {
                diagnostics
                    .partial_write_responses
                    .fetch_add(1, Ordering::Relaxed);
            }
            diagnostics
                .bytes_uploaded
                .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
            written = written.checked_add(count).ok_or_else(|| {
                remote_error(
                    StorageProviderErrorKind::Protocol,
                    "write",
                    "write position overflowed",
                    false,
                )
            })?;
            offset = offset
                .checked_add(u64::try_from(count).map_err(|_| {
                    remote_error(
                        StorageProviderErrorKind::Protocol,
                        "write",
                        "write count exceeded the supported range",
                        false,
                    )
                })?)
                .ok_or_else(|| {
                    remote_error(
                        StorageProviderErrorKind::Protocol,
                        "write",
                        "remote file offset overflowed",
                        false,
                    )
                })?;
            if offset >= next_progress {
                tracing::info!(
                    target: "graphql_orm_storage::smb",
                    event = "upload_progress",
                    key = progress_key.unwrap_or("[redacted]"),
                    size_bytes = offset,
                    elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
                next_progress = offset.saturating_add(WRITE_PROGRESS_INTERVAL);
            }
        }
    }

    Ok(BlobWriteOutcome {
        size_bytes: offset,
        sha256_hex: format!("{:x}", hasher.finalize()),
    })
}

fn write_request_payload_limit(max_write_size: u32) -> Result<usize, StorageError> {
    let negotiated = usize::try_from(max_write_size).map_err(|_| {
        remote_error(
            StorageProviderErrorKind::Protocol,
            "connect",
            "negotiated maximum write size is unsupported on this platform",
            false,
        )
    })?;
    if negotiated == 0 {
        return Err(remote_error(
            StorageProviderErrorKind::Protocol,
            "connect",
            "server negotiated a zero maximum write size",
            false,
        ));
    }
    let capped = negotiated.min(CONSERVATIVE_WRITE_REQUEST_CAP);
    Ok(if capped >= SMB2_CREDIT_UNIT {
        capped - (capped % SMB2_CREDIT_UNIT)
    } else {
        capped
    })
}

fn write_create_args(exclusive: bool) -> FileCreateArgs {
    if exclusive {
        FileCreateArgs::make_create_new(FileAttributes::new(), CreateOptions::new())
    } else {
        FileCreateArgs::make_overwrite(FileAttributes::new(), CreateOptions::new())
    }
}

#[async_trait]
impl BlobStore for SmbStorageBackend {
    fn backend(&self) -> StorageBackend {
        StorageBackend::Smb
    }

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        validate_blob_key(key)?;
        let temp = temp_key(key)?;
        let result = self.write_file(&temp, body, false).await;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self.remove_file(&temp).await;
                return Err(error);
            }
        };
        let mut cleanup = TempObjectCleanupGuard::new(self.clone(), temp.clone());
        if let Err(error) = self.rename(&temp, key, true).await {
            cleanup.cleanup().await;
            return Err(error);
        }
        cleanup.disarm();
        Ok(outcome)
    }

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        validate_blob_key(key)?;
        match self.write_file(key, body, true).await {
            Ok(outcome) => Ok(Some(outcome)),
            Err(error) if is_collision(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError> {
        validate_blob_key(key)?;
        let permit = self.transfer_permit().await?;
        let file = self
            .open_file(key, FileAccessMask::new().with_generic_read(true))
            .await?;
        let generation = self.client_snapshot().await.generation;
        let standard: FileStandardInformation =
            match self.with_timeout("head", file.query_info()).await {
                Ok(standard) => standard,
                Err(error) => {
                    if error.is_retryable() {
                        let _ = self.reconnect_after(generation).await;
                    }
                    return Err(error);
                }
            };
        let size = standard.end_of_file;
        let operation_timeout = self.inner.config.operation_timeout;
        let stream_key = key.to_string();
        let backend = self.clone();
        let body = stream::try_unfold(
            (Some(file), 0_u64, size, permit, stream_key.clone()),
            move |(file, offset, size, permit, key)| {
                let backend = backend.clone();
                async move {
                    let Some(file) = file else {
                        return Ok(None);
                    };
                    if offset >= size {
                        let close =
                            match tokio::time::timeout(operation_timeout, file.close()).await {
                                Ok(result) => result.map_err(|error| map_smb_error("close", error)),
                                Err(_) => Err(remote_error(
                                    StorageProviderErrorKind::Timeout,
                                    "close",
                                    "operation timed out",
                                    true,
                                )),
                            };
                        if let Err(error) = &close
                            && error.is_retryable()
                        {
                            let _ = backend.reconnect_after(generation).await;
                        }
                        close?;
                        return Ok(None);
                    }
                    let chunk_len = usize::try_from((size - offset).min(READ_CHUNK_SIZE as u64))
                        .unwrap_or(READ_CHUNK_SIZE);
                    let mut buffer = vec![0_u8; chunk_len];
                    let read = match tokio::time::timeout(
                        operation_timeout,
                        file.read_block(&mut buffer, offset, None, false),
                    )
                    .await
                    {
                        Ok(result) => result.map_err(|error| map_io_error("read", error)),
                        Err(_) => Err(remote_error(
                            StorageProviderErrorKind::Timeout,
                            "read",
                            "operation timed out",
                            true,
                        )),
                    };
                    let read = match read {
                        Ok(read) => read,
                        Err(error) => {
                            if error.is_retryable() {
                                let _ = backend.reconnect_after(generation).await;
                            }
                            return Err(error);
                        }
                    };
                    if read == 0 {
                        return Err(remote_error(
                            StorageProviderErrorKind::Protocol,
                            "read",
                            "unexpected end of remote file",
                            false,
                        ));
                    }
                    buffer.truncate(read);
                    let next = offset
                        .checked_add(u64::try_from(read).map_err(|_| {
                            remote_error(
                                StorageProviderErrorKind::Protocol,
                                "read",
                                "read count exceeded the supported range",
                                false,
                            )
                        })?)
                        .ok_or_else(|| {
                            remote_error(
                                StorageProviderErrorKind::Protocol,
                                "read",
                                "remote file offset overflowed",
                                false,
                            )
                        })?;
                    Ok(Some((
                        Bytes::from(buffer),
                        (Some(file), next, size, permit, key),
                    )))
                }
            },
        );
        Ok(BlobBody {
            key: stream_key.clone(),
            metadata: Some(BlobMetadata {
                key: stream_key,
                size_bytes: Some(size),
                sha256_hex: None,
                etag: None,
                last_modified: None,
            }),
            body: StorageByteStream::with_size_hint(Box::pin(body), size),
        })
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError> {
        validate_blob_key(key)?;
        match self
            .open_file(key, FileAccessMask::new().with_generic_read(true))
            .await
        {
            Ok(file) => {
                let generation = self.client_snapshot().await.generation;
                match self.with_timeout("close", file.close()).await {
                    Ok(()) => Ok(true),
                    Err(error) if error.is_retryable() => {
                        self.reconnect_after(generation).await?;
                        Ok(true)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError> {
        validate_blob_key(key)?;
        let file = match self
            .open_file(key, FileAccessMask::new().with_generic_read(true))
            .await
        {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        let generation = self.client_snapshot().await.generation;
        let standard: FileStandardInformation =
            match self.with_timeout("head", file.query_info()).await {
                Ok(standard) => standard,
                Err(error) => {
                    if error.is_retryable() {
                        let _ = self.reconnect_after(generation).await;
                    }
                    return Err(error);
                }
            };
        match self.with_timeout("close", file.close()).await {
            Ok(()) => {}
            Err(error) if error.is_retryable() => {
                self.reconnect_after(generation).await?;
            }
            Err(error) => return Err(error),
        }
        Ok(Some(BlobMetadata {
            key: key.to_string(),
            size_bytes: Some(standard.end_of_file),
            sha256_hex: None,
            etag: None,
            last_modified: None,
        }))
    }

    async fn list_blobs_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;
        if limit == 0 {
            return Err(StorageError::PreconditionFailed {
                key: prefix.to_string(),
                condition: "list limit must be greater than zero".to_string(),
            });
        }
        let mut keys = self.collect_keys().await?;
        keys.retain(|key| {
            key.starts_with(prefix)
                && continuation
                    .as_deref()
                    .is_none_or(|token| key.as_str() > token)
        });
        let next_continuation = if keys.len() > limit {
            keys.truncate(limit);
            keys.last().cloned()
        } else {
            None
        };
        Ok(BlobListPage {
            keys,
            next_continuation,
        })
    }

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError> {
        validate_blob_key(key)?;
        self.remove_file(key).await
    }
}

async fn connect_client(
    config: &SmbStorageConfig,
    generation: u64,
) -> Result<ConnectedClient, StorageError> {
    let connection = ConnectionConfig {
        port: Some(config.port),
        timeout: Some(config.connect_timeout),
        min_dialect: Some(config.min_dialect.protocol()),
        encryption_mode: if config.require_encryption {
            EncryptionMode::Required
        } else {
            EncryptionMode::Allowed
        },
        allow_unsigned_guest_access: false,
        smb2_only_negotiate: true,
        ..ConnectionConfig::default()
    };
    let client = Client::new(ClientConfig {
        connection,
        ..ClientConfig::default()
    });
    let share = UncPath::new(&config.server)
        .and_then(|path| path.with_share(&config.share))
        .map_err(|error| map_smb_error("connect", error))?;
    let username = config.authenticated_username();
    tokio::time::timeout(
        config.connect_timeout,
        client.share_connect(
            &share,
            &username,
            config.password.expose_secret().to_owned(),
        ),
    )
    .await
    .map_err(|_| {
        remote_error(
            StorageProviderErrorKind::Timeout,
            "connect",
            "SMB negotiation, authentication, or share connection timed out",
            true,
        )
    })?
    .map_err(|error| map_smb_error("connect", error))?;
    let connection = client
        .get_connection(&config.server)
        .await
        .map_err(|error| map_smb_error("connect", error))?;
    let info = connection.conn_info().ok_or_else(|| {
        remote_error(
            StorageProviderErrorKind::Protocol,
            "connect",
            "negotiated connection information is unavailable",
            false,
        )
    })?;
    let negotiated_dialect = SmbDialect::from_protocol(info.negotiation.dialect_rev);
    let max_read_size = info.negotiation.max_read_size;
    let max_write_size = info.negotiation.max_write_size;
    let max_transact_size = info.negotiation.max_transact_size;
    let write_request_payload_limit = write_request_payload_limit(max_write_size)?;

    // smb-rs does not expose the authenticated SessionInfo through its public
    // high-level client. This backend refuses guest/anonymous fallback, so an
    // authenticated non-encrypted session is signed; required encryption is
    // forced by ConnectionConfig and therefore active after share_connect.
    let signing_active = true;
    let encryption_active = config.require_encryption;
    drop(connection);

    Ok(ConnectedClient {
        client: Arc::new(client),
        generation,
        properties: SmbConnectionProperties {
            negotiated_dialect,
            signing_active,
            encryption_active,
            max_read_size,
            max_write_size,
            max_transact_size,
            write_request_payload_limit,
        },
    })
}

async fn close_resource(resource: Resource) -> smb::Result<()> {
    match resource {
        Resource::File(file) => file.close().await,
        Resource::Directory(directory) => directory.close().await,
        Resource::Pipe(pipe) => pipe.close().await,
    }
}

fn validate_server(server: &str) -> Result<(), StorageError> {
    if server.is_empty()
        || server.trim() != server
        || server.contains(['/', '\\', '\0'])
        || server.contains("://")
        || server == "."
        || server == ".."
    {
        return Err(invalid_config(
            "server must be a DNS name or IP address without a scheme or path",
        ));
    }
    Ok(())
}

fn validate_component(name: &str, value: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.trim() != value
        || value.contains(['/', '\\', '\0'])
        || value == "."
        || value == ".."
    {
        return Err(invalid_config(&format!("{name} is invalid")));
    }
    Ok(())
}

fn invalid_config(message: &str) -> StorageError {
    remote_error(
        StorageProviderErrorKind::Protocol,
        "configuration",
        message,
        false,
    )
}

fn temp_key(key: &str) -> Result<String, StorageError> {
    let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
    let temp_name = format!(".{name}.{}{TEMP_SUFFIX}", Uuid::new_v4());
    let temp = if parent.is_empty() {
        temp_name
    } else {
        format!("{parent}/{temp_name}")
    };
    validate_blob_key(&temp)?;
    Ok(temp)
}

fn is_temp_key(key: &str) -> bool {
    let Some(name) = key.rsplit('/').next() else {
        return false;
    };
    let Some(stem) = name.strip_suffix(TEMP_SUFFIX) else {
        return false;
    };
    stem.rsplit_once('.')
        .is_some_and(|(_, uuid)| Uuid::parse_str(uuid).is_ok())
}

fn map_io_error(operation: &'static str, error: std::io::Error) -> StorageError {
    let kind = match error.kind() {
        std::io::ErrorKind::TimedOut => StorageProviderErrorKind::Timeout,
        std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::UnexpectedEof => StorageProviderErrorKind::ConnectionLost,
        std::io::ErrorKind::PermissionDenied => StorageProviderErrorKind::PermissionDenied,
        std::io::ErrorKind::NotFound => StorageProviderErrorKind::RemotePathNotFound,
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => {
            StorageProviderErrorKind::Capacity
        }
        _ => StorageProviderErrorKind::Protocol,
    };
    let retryable = matches!(
        kind,
        StorageProviderErrorKind::Timeout
            | StorageProviderErrorKind::ConnectionLost
            | StorageProviderErrorKind::Connectivity
    );
    remote_error(kind, operation, error.to_string(), retryable)
}

fn map_smb_error(operation: &'static str, error: smb::Error) -> StorageError {
    let status = match &error {
        smb::Error::UnexpectedMessageStatus(status)
        | smb::Error::ReceivedErrorMessage(status, _) => Some(*status),
        _ => None,
    };
    let kind = match status {
        Some(value) if value == Status::LogonFailure as u32 => {
            StorageProviderErrorKind::Authentication
        }
        Some(value) if value == Status::AccessDenied as u32 => {
            StorageProviderErrorKind::PermissionDenied
        }
        Some(value) if value == Status::BadNetworkName as u32 => {
            StorageProviderErrorKind::ContainerNotFound
        }
        Some(value)
            if value == Status::ObjectNameNotFound as u32
                || value == Status::ObjectPathNotFound as u32 =>
        {
            StorageProviderErrorKind::RemotePathNotFound
        }
        Some(value) if value == Status::ObjectNameCollision as u32 => {
            StorageProviderErrorKind::ConditionalCreateConflict
        }
        Some(value) if value == Status::IoTimeout as u32 => StorageProviderErrorKind::Timeout,
        Some(value)
            if value == Status::NetworkNameDeleted as u32
                || value == Status::UserSessionDeleted as u32
                || value == Status::NetworkSessionExpired as u32 =>
        {
            StorageProviderErrorKind::ConnectionLost
        }
        _ => match &error {
            smb::Error::TransportError(smb::transport::TransportError::Timeout(_))
            | smb::Error::OperationTimeout(_, _) => StorageProviderErrorKind::Timeout,
            smb::Error::TransportError(smb::transport::TransportError::NotConnected)
            | smb::Error::ConnectionStopped => StorageProviderErrorKind::ConnectionLost,
            smb::Error::MessageProcessingError(message)
                if message.contains("Failed to send message to worker") =>
            {
                StorageProviderErrorKind::ConnectionLost
            }
            smb::Error::TransportError(smb::transport::TransportError::IoError(_))
            | smb::Error::IoError(_) => StorageProviderErrorKind::Connectivity,
            smb::Error::SspiError(_) => StorageProviderErrorKind::Authentication,
            smb::Error::NegotiationError(_)
            | smb::Error::SignatureVerificationFailed
            | smb::Error::CryptoError(_) => StorageProviderErrorKind::SecurityNegotiation,
            _ => StorageProviderErrorKind::Protocol,
        },
    };
    let retryable = matches!(
        kind,
        StorageProviderErrorKind::Connectivity
            | StorageProviderErrorKind::Timeout
            | StorageProviderErrorKind::ConnectionLost
    );
    remote_error(kind, operation, error.to_string(), retryable)
}

fn remote_error(
    kind: StorageProviderErrorKind,
    operation: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
) -> StorageError {
    StorageError::RemoteProvider {
        backend: StorageBackend::Smb.as_str().to_string(),
        kind,
        operation: operation.into(),
        message: message.into(),
        retryable,
    }
}

fn is_not_found(error: &StorageError) -> bool {
    matches!(
        error,
        StorageError::RemoteProvider {
            kind: StorageProviderErrorKind::RemotePathNotFound,
            ..
        }
    )
}

fn is_collision(error: &StorageError) -> bool {
    matches!(
        error,
        StorageError::RemoteProvider {
            kind: StorageProviderErrorKind::ConditionalCreateConflict,
            ..
        }
    )
}

fn provider_error_kind(error: &StorageError) -> Option<StorageProviderErrorKind> {
    match error {
        StorageError::RemoteProvider { kind, .. } => Some(*kind),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingWriter {
        bytes: std::sync::Mutex<Vec<u8>>,
        offsets: std::sync::Mutex<Vec<u64>>,
        request_lengths: std::sync::Mutex<Vec<usize>>,
        partial_limit: Option<usize>,
        zero_on_request: Option<u64>,
        requests: AtomicU64,
    }

    #[async_trait]
    impl SmbBlockWriter for RecordingWriter {
        async fn write_block(&self, bytes: &[u8], offset: u64) -> Result<usize, StorageError> {
            let request = self.requests.fetch_add(1, Ordering::Relaxed);
            self.offsets.lock().expect("offset lock").push(offset);
            self.request_lengths
                .lock()
                .expect("request length lock")
                .push(bytes.len());
            if self.zero_on_request == Some(request) {
                return Ok(0);
            }
            let count = self.partial_limit.unwrap_or(bytes.len()).min(bytes.len());
            self.bytes
                .lock()
                .expect("bytes lock")
                .extend_from_slice(&bytes[..count]);
            Ok(count)
        }
    }

    async fn write_test_stream(
        writer: &RecordingWriter,
        stream: StorageByteStream,
        request_limit: usize,
    ) -> Result<BlobWriteOutcome, StorageError> {
        write_stream_to_writer(
            writer,
            stream,
            request_limit,
            &SmbDiagnosticCounters::default(),
            None,
        )
        .await
    }

    async fn simulate_cached_directory(
        cache: &DirectoryCache,
        path: &str,
        remote_creates: &AtomicU64,
    ) {
        if cache.contains(path).await {
            return;
        }
        let (gate, _) = cache.gate(path).await;
        let _permit = gate.acquire_owned().await.expect("directory gate");
        if cache.contains(path).await {
            return;
        }
        remote_creates.fetch_add(1, Ordering::Relaxed);
        tokio::task::yield_now().await;
        cache.insert(path.to_string()).await;
    }

    fn config() -> SmbStorageConfig {
        SmbStorageConfig::new(
            "files.example.test",
            "backups",
            "backup-user",
            SecretString::from("correct horse battery staple"),
        )
    }

    #[test]
    fn configuration_defaults_are_secure() {
        let config = config();
        assert_eq!(config.port, 445);
        assert_eq!(config.min_dialect, SmbDialect::Smb3_0);
        assert!(config.require_signing);
        assert!(!config.require_encryption);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn configuration_rejects_paths_and_unsafe_prefixes() {
        for server in [r"\\server\share", "smb://server/share", "/mnt/share"] {
            let mut config = config();
            config.server = server.to_string();
            assert!(config.validate().is_err(), "accepted {server}");
        }
        let mut config = config();
        config.root_prefix = Some("backups/../escape".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn debug_never_exposes_password() {
        let rendered = format!("{:?}", config());
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("correct horse battery staple"));
    }

    #[test]
    fn root_prefix_mapping_is_provider_relative() {
        let mut config = config();
        config.root_prefix = Some("repositories/nightly".to_string());
        let mapped = match config.root_prefix.as_deref() {
            Some(prefix) => format!("{prefix}/objects/data"),
            None => "objects/data".to_string(),
        };
        assert_eq!(mapped, "repositories/nightly/objects/data");
    }

    #[test]
    fn temporary_names_are_filtered_strictly() {
        let temp = temp_key("nested/blob.bin").expect("safe temp key");
        assert!(is_temp_key(&temp));
        assert!(!is_temp_key("nested/report.uploading"));
        assert!(!is_temp_key("nested/blob.not-a-uuid.uploading"));
    }

    #[test]
    fn retry_classification_is_bounded_to_transient_errors() {
        let timeout = remote_error(StorageProviderErrorKind::Timeout, "read", "timed out", true);
        let denied = remote_error(
            StorageProviderErrorKind::PermissionDenied,
            "read",
            "denied",
            false,
        );
        assert!(timeout.is_retryable());
        assert!(!denied.is_retryable());
    }

    #[test]
    fn write_limit_respects_negotiation_cap_and_credit_alignment() {
        assert_eq!(
            write_request_payload_limit(8 * 1024 * 1024).expect("large negotiated limit"),
            CONSERVATIVE_WRITE_REQUEST_CAP
        );
        assert_eq!(
            write_request_payload_limit(100_000).expect("unaligned negotiated limit"),
            SMB2_CREDIT_UNIT
        );
        assert_eq!(
            write_request_payload_limit(4_096).expect("small negotiated limit"),
            4_096
        );
        assert!(write_request_payload_limit(0).is_err());
    }

    #[test]
    fn conditional_and_overwrite_paths_use_distinct_create_dispositions() {
        assert_eq!(
            write_create_args(true).disposition,
            smb::CreateDisposition::Create
        );
        assert_eq!(
            write_create_args(false).disposition,
            smb::CreateDisposition::OverwriteIf
        );
    }

    #[tokio::test]
    async fn oversized_single_chunk_is_split_and_verified_exactly() {
        const LENGTH: usize = 12 * 1024 * 1024 + 137;
        let input = Bytes::from(
            (0..LENGTH)
                .map(|index| u8::try_from(index % 251).expect("bounded byte"))
                .collect::<Vec<_>>(),
        );
        let expected_hash = format!("{:x}", Sha256::digest(&input));
        let writer = RecordingWriter::default();

        let outcome = write_test_stream(
            &writer,
            StorageByteStream::from_bytes(input.clone()),
            CONSERVATIVE_WRITE_REQUEST_CAP,
        )
        .await
        .expect("oversized stream");

        assert_eq!(outcome.size_bytes, u64::try_from(LENGTH).expect("length"));
        assert_eq!(outcome.sha256_hex, expected_hash);
        assert_eq!(&*writer.bytes.lock().expect("bytes lock"), input.as_ref());
        assert!(
            writer
                .request_lengths
                .lock()
                .expect("request lengths")
                .iter()
                .all(|length| *length <= CONSERVATIVE_WRITE_REQUEST_CAP)
        );
    }

    #[tokio::test]
    async fn multiple_chunks_and_small_negotiated_limit_preserve_output() {
        let chunks = vec![
            Ok(Bytes::from_static(b"abcde")),
            Ok(Bytes::from_static(b"fghijklmnop")),
            Ok(Bytes::new()),
            Ok(Bytes::from_static(b"qrstuvwxyz")),
        ];
        let expected = b"abcdefghijklmnopqrstuvwxyz";
        let writer = RecordingWriter::default();

        let outcome = write_test_stream(
            &writer,
            StorageByteStream::new(Box::pin(stream::iter(chunks))),
            7,
        )
        .await
        .expect("multi-chunk stream");

        assert_eq!(outcome.size_bytes, 26);
        assert_eq!(
            outcome.sha256_hex,
            format!("{:x}", Sha256::digest(expected))
        );
        assert_eq!(&*writer.bytes.lock().expect("bytes lock"), expected);
        assert!(
            writer
                .request_lengths
                .lock()
                .expect("request lengths")
                .iter()
                .all(|length| *length <= 7)
        );
    }

    #[tokio::test]
    async fn partial_writes_advance_offsets_without_losing_bytes() {
        let writer = RecordingWriter {
            partial_limit: Some(3),
            ..RecordingWriter::default()
        };
        let input = Bytes::from_static(b"partial write response");

        let outcome = write_test_stream(&writer, StorageByteStream::from_bytes(input.clone()), 8)
            .await
            .expect("partial writes");

        assert_eq!(outcome.size_bytes, input.len() as u64);
        assert_eq!(&*writer.bytes.lock().expect("bytes lock"), input.as_ref());
        let offsets = writer.offsets.lock().expect("offset lock");
        assert_eq!(offsets.first(), Some(&0));
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(offsets.last(), Some(&21));
    }

    #[tokio::test]
    async fn zero_byte_response_fails_but_empty_stream_succeeds() {
        let zero_writer = RecordingWriter {
            zero_on_request: Some(0),
            ..RecordingWriter::default()
        };
        let error = write_test_stream(
            &zero_writer,
            StorageByteStream::from_bytes(Bytes::from_static(b"data")),
            4,
        )
        .await
        .expect_err("zero response must fail");
        assert_eq!(
            provider_error_kind(&error),
            Some(StorageProviderErrorKind::Protocol)
        );

        let empty_writer = RecordingWriter::default();
        let empty = write_test_stream(
            &empty_writer,
            StorageByteStream::from_bytes(Bytes::new()),
            4,
        )
        .await
        .expect("empty stream");
        assert_eq!(empty.size_bytes, 0);
        assert_eq!(empty.sha256_hex, format!("{:x}", Sha256::digest([])));
        assert_eq!(empty_writer.requests.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn same_parent_creation_is_coalesced_for_supported_concurrency() {
        for concurrency in [1_usize, 2, 4, 8] {
            let cache = Arc::new(DirectoryCache::new(32, concurrency));
            let remote_creates = Arc::new(AtomicU64::new(0));
            let mut tasks = Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                let cache = Arc::clone(&cache);
                let remote_creates = Arc::clone(&remote_creates);
                tasks.push(tokio::spawn(async move {
                    simulate_cached_directory(&cache, "objects/ab", &remote_creates).await;
                }));
            }
            for task in tasks {
                task.await.expect("directory task");
            }
            assert_eq!(
                remote_creates.load(Ordering::Relaxed),
                1,
                "concurrency {concurrency}"
            );
        }
    }

    #[tokio::test]
    async fn reconnect_invalidation_rechecks_cached_directories() {
        let cache = DirectoryCache::new(4, 1);
        let remote_creates = AtomicU64::new(0);
        simulate_cached_directory(&cache, "objects/ab", &remote_creates).await;
        simulate_cached_directory(&cache, "objects/ab", &remote_creates).await;
        assert_eq!(remote_creates.load(Ordering::Relaxed), 1);

        cache.clear().await;
        simulate_cached_directory(&cache, "objects/ab", &remote_creates).await;
        assert_eq!(remote_creates.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn shared_parent_is_not_reopened_per_object() {
        let cache = DirectoryCache::new(8, 1);
        let remote_creates = AtomicU64::new(0);
        for _ in 0..43_000 {
            simulate_cached_directory(&cache, "objects/ab", &remote_creates).await;
        }
        assert_eq!(remote_creates.load(Ordering::Relaxed), 1);
        assert_eq!(cache.len().await, 1);
    }
}
