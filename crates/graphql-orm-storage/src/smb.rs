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
/// Default deepest directory nesting a listing descends below the root.
const DEFAULT_MAX_LISTING_DEPTH: usize = 64;
/// Default most directory entries (files and directories) one listing enumerates.
const DEFAULT_MAX_LISTING_ENTRIES: usize = 1_000_000;
/// Default overall deadline for one complete listing.
const DEFAULT_LISTING_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Default most bytes of names and paths one listing may accumulate.
const DEFAULT_MAX_LISTING_BYTES: usize = 256 * 1024 * 1024;
/// Longest name component, in UTF-16 code units (MS-FSCC 2.1.5.2).
const MAX_NAME_UTF16_UNITS: usize = 255;
/// Longest blob key, in UTF-8 bytes, this backend writes or lists.
const MAX_KEY_BYTES: usize = 4_096;
/// Bytes a temporary upload key may add to a target key: `/` plus
/// `.<uuid>.uploading` replaces at least one byte of the target name.
const TEMP_KEY_EXTRA_BYTES: usize = 48;
/// Bytes charged per retained name or path, beyond its text, for its
/// allocation and container slot.
const LISTED_PATH_OVERHEAD: usize = 64;
/// Most pages one continuation chain of `list_blobs_page` may return. Each
/// page re-lists the whole tree; `list_blobs` lists it once.
const MAX_LISTING_PAGES: u64 = 1_024;
/// Prefix of this backend's continuation tokens.
const CONTINUATION_PREFIX: &str = "smb1:";
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
    /// Username used for NTLM authentication. Use a dedicated, least-privilege
    /// service account for this backend.
    pub username: String,
    /// Password used for NTLM authentication. Debug output is redacted. NTLM
    /// exposes a challenge response that can be attacked offline, so use a
    /// long, random password that is not used anywhere else.
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
    /// Server GUID the server must report during negotiation. When `None`, the
    /// GUID reported by the first successful connection is pinned instead.
    /// Either way, a reconnect to a server reporting another GUID is refused
    /// before authentication. See [`SmbProbeResult::server_guid`].
    pub expected_server_guid: Option<Uuid>,
    /// Deepest directory nesting a listing descends below the root prefix;
    /// 64 by default. A deeper listing fails instead of returning a partial
    /// result.
    pub max_listing_depth: usize,
    /// Most directory entries, files and directories, one listing may
    /// enumerate; 1,000,000 by default. A larger listing fails instead of
    /// returning a partial result.
    pub max_listing_entries: usize,
    /// Overall deadline for one complete listing, including its retry after
    /// a reconnect, in addition to `operation_timeout` for each request;
    /// 15 minutes by default.
    pub listing_timeout: Duration,
    /// Most bytes of names and paths one listing may accumulate; 256 MiB by
    /// default. A larger listing fails instead of returning a partial result.
    pub max_listing_bytes: usize,
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
            expected_server_guid: None,
            max_listing_depth: DEFAULT_MAX_LISTING_DEPTH,
            max_listing_entries: DEFAULT_MAX_LISTING_ENTRIES,
            listing_timeout: DEFAULT_LISTING_TIMEOUT,
            max_listing_bytes: DEFAULT_MAX_LISTING_BYTES,
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
        if self.connect_timeout.is_zero()
            || self.operation_timeout.is_zero()
            || self.listing_timeout.is_zero()
        {
            return Err(invalid_config("timeouts must be greater than zero"));
        }
        if self.max_listing_depth == 0
            || self.max_listing_entries == 0
            || self.max_listing_bytes == 0
        {
            return Err(invalid_config(
                "listing depth, entry and byte limits must be greater than zero",
            ));
        }
        if self.expected_server_guid.is_some_and(|guid| guid.is_nil()) {
            return Err(invalid_config("expected server GUID must not be nil"));
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
            .field("expected_server_guid", &self.expected_server_guid)
            .field("max_listing_depth", &self.max_listing_depth)
            .field("max_listing_entries", &self.max_listing_entries)
            .field("listing_timeout", &self.listing_timeout)
            .field("max_listing_bytes", &self.max_listing_bytes)
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
    /// Server GUID reported during negotiation. Record it in
    /// [`SmbStorageConfig::expected_server_guid`] to pin the server from the
    /// first connection. It is not secret and does not authenticate the server.
    pub server_guid: Uuid,
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
    server_guid: Uuid,
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
    /// Server GUID pinned by the first connection; reconnects must match it.
    server_guid: Uuid,
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
        let client = connect_client(&config, 0, config.expected_server_guid).await?;
        tracing::info!(
            target: "graphql_orm_storage::smb",
            event = "connected",
            generation = client.generation,
            server_guid = %client.properties.server_guid,
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
                server_guid: client.properties.server_guid,
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
        // Read no more than was written, whatever size the server declares.
        let actual = match loaded.metadata.and_then(|metadata| metadata.size_bytes) {
            Some(size) if size == expected.len() as u64 => {
                collect_bounded(loaded.body, expected.len()).await
            }
            _ => Err(probe_mismatch()),
        };
        let actual = match actual {
            Ok(actual) => actual,
            Err(error) => {
                let _ = backend.delete_blob(&key).await;
                return Err(error);
            }
        };
        if actual != expected {
            let _ = backend.delete_blob(&key).await;
            return Err(probe_mismatch());
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
            server_guid: info.server_guid,
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
            match connect_client(&self.inner.config, generation, Some(self.inner.server_guid)).await
            {
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
        map_key(self.inner.config.root_prefix.as_deref(), key)
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
        let target = if exclusive {
            WriteCleanupTarget::ExclusiveHandle
        } else {
            WriteCleanupTarget::TemporaryPath(key.to_string())
        };
        let mut cleanup = WriteCleanupGuard::new(self.clone(), target, file, generation);

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

    async fn remove_file_once(&self, key: &str) -> Result<(), StorageError> {
        remove_file_without_replay(
            self.open_file(key, FileAccessMask::new().with_delete(true)),
            |file| async move {
                let result = self
                    .with_timeout(
                        "delete",
                        file.set_info(FileDispositionInformation::default()),
                    )
                    .await;
                let close_result = self.with_timeout("close", file.close()).await;
                // Never reconnect and reopen this path after disposition or
                // close may have deleted it. The key may already have a new owner.
                result.and(close_result)
            },
        )
        .await
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

    /// Lists every key once. One `listing_timeout` deadline covers both
    /// attempts and the reconnect between them.
    async fn collect_keys(&self) -> Result<Vec<String>, StorageError> {
        let limits = ListingLimits::from_config(&self.inner.config);
        within_listing_deadline(
            limits.timeout,
            retry_listing_once(|| self.collect_keys_attempt(limits)),
        )
        .await
    }

    async fn collect_keys_attempt(
        &self,
        limits: ListingLimits,
    ) -> Result<Vec<String>, StorageError> {
        let snapshot = self.client_snapshot().await;
        match self.collect_keys_once(&snapshot.client, limits).await {
            Err(error) if error.is_retryable() => {
                self.reconnect_after(snapshot.generation).await?;
                Err(error)
            }
            other => other,
        }
    }

    async fn collect_keys_once(
        &self,
        client: &Client,
        limits: ListingLimits,
    ) -> Result<Vec<String>, StorageError> {
        let source = SmbListingSource {
            backend: self,
            client,
            base: self.share_path()?,
        };
        let root = self.inner.config.root_prefix.clone().unwrap_or_default();
        let mut keys = walk_listing(&source, root, limits).await?;
        keys.retain(|key| !is_temp_key(key));
        keys.sort();
        Ok(keys)
    }
}

/// Resource bounds for one recursive listing. A listing that reaches a bound
/// fails; it never returns a partial result.
#[derive(Clone, Copy, Debug)]
struct ListingLimits {
    max_depth: usize,
    max_entries: usize,
    max_bytes: usize,
    timeout: Duration,
}

impl ListingLimits {
    fn from_config(config: &SmbStorageConfig) -> Self {
        Self {
            max_depth: config.max_listing_depth,
            max_entries: config.max_listing_entries,
            max_bytes: config.max_listing_bytes,
            timeout: config.listing_timeout,
        }
    }
}

/// Counts what one listing has enumerated and accumulated against its limits.
#[derive(Debug)]
struct ListingBudget {
    max_entries: usize,
    entries: usize,
    max_bytes: usize,
    bytes: usize,
}

impl ListingBudget {
    fn new(limits: ListingLimits) -> Self {
        Self {
            max_entries: limits.max_entries,
            entries: 0,
            max_bytes: limits.max_bytes,
            bytes: 0,
        }
    }

    /// Records one enumerated entry and the bytes of its name, failing once
    /// a limit is exceeded or the name is longer than a valid component.
    fn record_entry(&mut self, name: &str) -> Result<(), StorageError> {
        self.entries += 1;
        if self.entries > self.max_entries {
            return Err(listing_limit_exceeded(format!(
                "listing enumerated more than {} entries",
                self.max_entries
            )));
        }
        if name.encode_utf16().count() > MAX_NAME_UTF16_UNITS {
            return Err(listing_limit_exceeded(format!(
                "listing returned a name longer than {MAX_NAME_UTF16_UNITS} UTF-16 code units"
            )));
        }
        self.charge(name.len().saturating_add(LISTED_PATH_OVERHEAD))
    }

    /// Charges bytes the listing holds, failing once the budget is exceeded.
    /// The charge is cumulative and never released, so it is conservative.
    fn charge(&mut self, bytes: usize) -> Result<(), StorageError> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > self.max_bytes {
            return Err(listing_limit_exceeded(format!(
                "listing accumulated more than {} bytes of names",
                self.max_bytes
            )));
        }
        Ok(())
    }
}

/// One enumerated directory entry, excluding `.` and `..`.
#[derive(Debug)]
struct ListedEntry {
    name: String,
    directory: bool,
}

/// Enumerates single directories for [`walk_listing`].
#[async_trait]
trait ListingSource: Sync {
    /// Lists the share-relative `path`, recording each entry in `budget` as
    /// it is enumerated. Returns `None` for a path the listing may skip (see
    /// [`listing_open_failure`]).
    async fn list_directory(
        &self,
        path: &str,
        discovered: bool,
        budget: &mut ListingBudget,
    ) -> Result<Option<Vec<ListedEntry>>, StorageError>;
}

/// Bounds a listing, including any retry, by one overall deadline. Running
/// out of time is not retryable: a listing that cannot finish in time would
/// not finish on a retry either.
async fn within_listing_deadline<T>(
    timeout: Duration,
    listing: impl std::future::Future<Output = Result<T, StorageError>>,
) -> Result<T, StorageError> {
    tokio::time::timeout(timeout, listing).await.map_err(|_| {
        remote_error(
            StorageProviderErrorKind::Timeout,
            "list",
            format!(
                "listing did not complete within {timeout:?}; refusing to return a partial listing"
            ),
            false,
        )
    })?
}

/// Runs `attempt` again once if it fails with a retryable error.
async fn retry_listing_once<T, F, Fut>(mut attempt: F) -> Result<T, StorageError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, StorageError>>,
{
    match attempt().await {
        Err(error) if error.is_retryable() => attempt().await,
        other => other,
    }
}

/// Walks the tree under the share-relative `root` within `limits`, returning
/// file keys relative to `root`.
async fn walk_listing<S: ListingSource>(
    source: &S,
    root: String,
    limits: ListingLimits,
) -> Result<Vec<String>, StorageError> {
    let mut budget = ListingBudget::new(limits);
    // Each entry is a root-relative directory path, whether enumeration
    // discovered it, and its depth below the root.
    let mut stack = vec![(String::new(), false, 0_usize)];
    let mut files = Vec::new();
    while let Some((directory, discovered, depth)) = stack.pop() {
        let path = match (root.is_empty(), directory.is_empty()) {
            (true, _) => directory.clone(),
            (false, true) => root.clone(),
            (false, false) => format!("{root}/{directory}"),
        };
        let Some(entries) = source
            .list_directory(&path, discovered, &mut budget)
            .await?
        else {
            continue;
        };
        for entry in entries {
            let name_len = entry.name.len();
            let child = if directory.is_empty() {
                entry.name
            } else {
                format!("{directory}/{}", entry.name)
            };
            // The name was charged when enumerated; charge the parent path.
            budget.charge(child.len() - name_len)?;
            // An interrupted upload's temporary file may be longer than its
            // target key; it is filtered from the result.
            if child.len() > key_byte_limit(&child) {
                return Err(listing_limit_exceeded(format!(
                    "listing reached a path longer than {MAX_KEY_BYTES} bytes"
                )));
            }
            if entry.directory {
                // A self-nesting directory (for example, a server-side link
                // to an ancestor) would otherwise be listed forever.
                if depth >= limits.max_depth {
                    return Err(listing_limit_exceeded(format!(
                        "listing exceeded the maximum directory depth of {} at {child:?}",
                        limits.max_depth
                    )));
                }
                stack.push((child, true, depth + 1));
            } else {
                files.push(child);
            }
        }
    }
    Ok(files)
}

/// Validates a caller's key: a safe relative key no longer than
/// [`MAX_KEY_BYTES`], whose every component fits [`MAX_NAME_UTF16_UNITS`]. A
/// key this backend could not list back is never written.
fn check_key(key: &str) -> Result<(), StorageError> {
    check_key_within(key, MAX_KEY_BYTES)
}

fn check_key_within(key: &str, max_bytes: usize) -> Result<(), StorageError> {
    validate_blob_key(key)?;
    if key.len() > max_bytes
        || key
            .split('/')
            .any(|component| component.encode_utf16().count() > MAX_NAME_UTF16_UNITS)
    {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }
    Ok(())
}

/// The longest key a path may have: a temporary upload name may exceed
/// [`MAX_KEY_BYTES`] by its fixed name, for a target key at the limit.
fn key_byte_limit(key: &str) -> usize {
    if is_temp_key(key) {
        MAX_KEY_BYTES + TEMP_KEY_EXTRA_BYTES
    } else {
        MAX_KEY_BYTES
    }
}

/// Maps a blob key below the root prefix, after the checks of [`check_key`].
fn map_key(root_prefix: Option<&str>, key: &str) -> Result<String, StorageError> {
    check_key_within(key, key_byte_limit(key))?;
    Ok(match root_prefix {
        Some(prefix) => format!("{prefix}/{key}"),
        None => key.to_string(),
    })
}

fn listing_limit_exceeded(reason: String) -> StorageError {
    remote_error(
        StorageProviderErrorKind::Protocol,
        "list",
        format!("{reason}; refusing to return a partial listing"),
        false,
    )
}

/// Position in a chain of `list_blobs_page` calls. Every page re-lists the
/// tree, so the chain carries its own count to bound the total work.
#[derive(Debug, Default, PartialEq, Eq)]
struct ListingContinuation {
    pages: u64,
    returned: u64,
    after: String,
}

impl ListingContinuation {
    fn parse(token: &str) -> Result<Self, StorageError> {
        let invalid = || StorageError::PreconditionFailed {
            key: String::new(),
            condition: "continuation token was not issued by this SMB backend".to_string(),
        };
        let rest = token
            .strip_prefix(CONTINUATION_PREFIX)
            .ok_or_else(invalid)?;
        let mut parts = rest.splitn(3, ':');
        let pages = parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(invalid)?;
        let returned = parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(invalid)?;
        let after = parts.next().ok_or_else(invalid)?.to_string();
        Ok(Self {
            pages,
            returned,
            after,
        })
    }

    /// The number of the page this position continues to, failing once the
    /// chain has used [`MAX_LISTING_PAGES`].
    fn next_page(&self) -> Result<u64, StorageError> {
        let pages = self.pages.saturating_add(1);
        if pages > MAX_LISTING_PAGES {
            return Err(listing_limit_exceeded(format!(
                "paged listing exceeded {MAX_LISTING_PAGES} pages; use list_blobs to list once"
            )));
        }
        Ok(pages)
    }

    fn encode(&self) -> String {
        format!(
            "{CONTINUATION_PREFIX}{}:{}:{}",
            self.pages, self.returned, self.after
        )
    }
}

/// Selects one page from a complete, sorted listing. A chain fails once it
/// has returned more keys than one listing may enumerate, or more than
/// [`MAX_LISTING_PAGES`] pages, so a server that adds names between pages
/// cannot keep it going forever.
fn listing_page(
    keys: Vec<String>,
    prefix: &str,
    continuation: Option<&str>,
    limit: usize,
    max_entries: usize,
) -> Result<BlobListPage, StorageError> {
    let position = continuation
        .map(ListingContinuation::parse)
        .transpose()?
        .unwrap_or_default();
    let pages = position.next_page()?;
    let mut matching = keys.into_iter().filter(|key| {
        key.starts_with(prefix)
            && (continuation.is_none() || key.as_str() > position.after.as_str())
    });
    let page: Vec<String> = matching.by_ref().take(limit).collect();
    let more = matching.next().is_some();
    let returned = position.returned.saturating_add(page.len() as u64);
    if returned > max_entries as u64 {
        return Err(listing_limit_exceeded(format!(
            "paged listing returned more than {max_entries} keys"
        )));
    }
    let next_continuation = match page.last() {
        Some(last) if more => Some(
            ListingContinuation {
                pages,
                returned,
                after: last.clone(),
            }
            .encode(),
        ),
        _ => None,
    };
    Ok(BlobListPage {
        keys: page,
        next_continuation,
    })
}

struct SmbListingSource<'a> {
    backend: &'a SmbStorageBackend,
    client: &'a Client,
    base: UncPath,
}

#[async_trait]
impl ListingSource for SmbListingSource<'_> {
    async fn list_directory(
        &self,
        directory_path: &str,
        discovered: bool,
        budget: &mut ListingBudget,
    ) -> Result<Option<Vec<ListedEntry>>, StorageError> {
        let target = self.base.clone().with_path(directory_path);
        let resource = match self
            .backend
            .with_timeout(
                "list",
                self.client.create_file(
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
            Err(error) => {
                listing_open_failure(directory_path, discovered, error)?;
                return Ok(None);
            }
        };
        let directory = match resource {
            Resource::Directory(directory) => Arc::new(directory),
            other => {
                let _ = close_resource(other).await;
                listing_not_directory(directory_path, discovered)?;
                return Ok(None);
            }
        };
        let listed = self.enumerate(&directory, budget).await;
        let closed = self.backend.with_timeout("close", directory.close()).await;
        // Close the handle whether or not enumeration reached a limit; the
        // enumeration error takes precedence over a close error.
        let listed = listed?;
        closed?;
        Ok(Some(listed))
    }
}

impl SmbListingSource<'_> {
    async fn enumerate(
        &self,
        directory: &Arc<Directory>,
        budget: &mut ListingBudget,
    ) -> Result<Vec<ListedEntry>, StorageError> {
        let operation_timeout = self.backend.inner.config.operation_timeout;
        let mut entries = self
            .backend
            .with_timeout(
                "list",
                Directory::query::<FileFullDirectoryInformation>(directory, "*"),
            )
            .await?;
        let mut listed = Vec::new();
        loop {
            let entry = tokio::time::timeout(operation_timeout, entries.next())
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
            budget.record_entry(&name)?;
            listed.push(ListedEntry {
                name,
                directory: entry.file_attributes.directory(),
            });
        }
        Ok(listed)
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

#[derive(Clone)]
enum WriteCleanupTarget {
    // Conditional creation uses a shared key. Never retain it for fallback
    // cleanup: closing the original handle can allow a new owner to create it.
    ExclusiveHandle,
    TemporaryPath(String),
}

async fn cleanup_write_target<'a, C, R, F>(target: &'a WriteCleanupTarget, close: C, remove: R)
where
    C: std::future::Future<Output = ()>,
    R: FnOnce(&'a str) -> F,
    F: std::future::Future<Output = Result<(), StorageError>>,
{
    close.await;
    if let WriteCleanupTarget::TemporaryPath(key) = target {
        let _ = remove(key).await;
    }
}

struct WriteCleanupGuard {
    backend: SmbStorageBackend,
    target: WriteCleanupTarget,
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
    fn new(
        backend: SmbStorageBackend,
        target: WriteCleanupTarget,
        file: File,
        generation: u64,
    ) -> Self {
        Self {
            backend,
            target,
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
        cleanup_write_target(
            &self.target,
            async {
                if let Some(file) = &self.file {
                    let close = self.backend.with_timeout("close", file.close()).await;
                    if let Err(error) = close
                        && error.is_retryable()
                    {
                        let _ = self.backend.reconnect_after(self.generation).await;
                    }
                }
            },
            |key| self.backend.remove_file(key),
        )
        .await;
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
        let target = self.target.clone();
        let generation = self.generation;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                cleanup_write_target(
                    &target,
                    async {
                        let close = backend.with_timeout("close", file.close()).await;
                        if let Err(error) = close
                            && error.is_retryable()
                        {
                            let _ = backend.reconnect_after(generation).await;
                        }
                    },
                    |key| backend.remove_file(key),
                )
                .await;
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
    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError> {
        validate_blob_key(key)?;
        self.remove_file_once(key).await
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::Smb
    }

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        // Checks the caller's key, not the longer temporary key.
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
        check_key(key)?;
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
        // A malformed token or an exhausted chain fails before the tree is
        // listed again.
        if let Some(token) = continuation.as_deref() {
            ListingContinuation::parse(token)?.next_page()?;
        }
        let keys = self.collect_keys().await?;
        listing_page(
            keys,
            prefix,
            continuation.as_deref(),
            limit,
            self.inner.config.max_listing_entries,
        )
    }

    /// Lists the tree once, within one deadline and one set of limits,
    /// instead of re-listing it for every page.
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;
        let mut keys = self.collect_keys().await?;
        keys.retain(|key| key.starts_with(prefix));
        Ok(keys)
    }

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError> {
        validate_blob_key(key)?;
        self.remove_file(key).await
    }
}

async fn remove_file_without_replay<H, O, D, F>(open: O, dispose: D) -> Result<(), StorageError>
where
    O: std::future::Future<Output = Result<H, StorageError>>,
    D: FnOnce(H) -> F,
    F: std::future::Future<Output = Result<(), StorageError>>,
{
    // A future and FnOnce encode a single open/dispose attempt. An ambiguous
    // mutation error is returned directly, never followed by a new path open.
    let handle = match open.await {
        Ok(handle) => handle,
        Err(error) if is_not_found(&error) => return Ok(()),
        Err(error) => return Err(StorageError::single_attempt(error)),
    };
    dispose(handle).await.map_err(StorageError::single_attempt)
}

/// Decides whether a listing may skip a directory it could not open.
///
/// Only the configured root may be absent, which lists as empty. A directory
/// that an enumeration reported must open: skipping it would silently return a
/// truncated listing, which callers such as backup pruning would treat as
/// authoritative. This backend never removes directories.
fn listing_open_failure(
    directory_path: &str,
    discovered: bool,
    error: StorageError,
) -> Result<(), StorageError> {
    if !discovered && is_not_found(&error) {
        return Ok(());
    }
    if discovered && is_not_found(&error) {
        return Err(incomplete_listing(
            directory_path,
            "was not found when opened",
        ));
    }
    Err(error)
}

/// A listed directory that opens as another resource type fails the listing.
fn listing_not_directory(directory_path: &str, discovered: bool) -> Result<(), StorageError> {
    if discovered {
        return Err(incomplete_listing(
            directory_path,
            "did not open as a directory",
        ));
    }
    Ok(())
}

fn incomplete_listing(directory_path: &str, reason: &str) -> StorageError {
    remote_error(
        StorageProviderErrorKind::Protocol,
        "list",
        format!(
            "listing is incomplete: enumerated directory {directory_path:?} {reason}; \
             refusing to return a partial listing"
        ),
        false,
    )
}

/// The SMB client configuration for one backend connection.
///
/// DFS referrals are not followed, so the client connects only to the
/// configured server. Server-to-client notifications are disabled: this
/// backend takes no leases or oplocks it would need them for, and the
/// vendored async notification handler does not drain its channel.
fn client_config(config: &SmbStorageConfig, expected_server_guid: Option<Uuid>) -> ClientConfig {
    let connection = ConnectionConfig {
        expected_server_guid: expected_server_guid.map(guid_from_uuid),
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
        disable_notifications: true,
        ..ConnectionConfig::default()
    };
    ClientConfig {
        connection,
        dfs: false,
        ..ClientConfig::default()
    }
}

/// Connects, authenticates and opens the share.
///
/// When `expected_server_guid` is set, every connection the client makes
/// refuses a server reporting another GUID during negotiation, before session
/// setup sends the NTLM response.
async fn connect_client(
    config: &SmbStorageConfig,
    generation: u64,
    expected_server_guid: Option<Uuid>,
) -> Result<ConnectedClient, StorageError> {
    let client = Client::new(client_config(config, expected_server_guid));
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
    let server_guid = uuid_from_guid(info.negotiation.server_guid);
    if expected_server_guid.is_some_and(|expected| expected != server_guid) {
        // Unreachable while negotiation enforces the pin; fail closed anyway.
        return Err(map_smb_error("connect", smb::Error::ServerGuidMismatch));
    }
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
            server_guid,
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

/// Collects at most `limit` bytes, failing as soon as the stream yields more.
/// Unlike [`crate::collect_storage_stream`], it never trusts a size hint.
async fn collect_bounded(stream: StorageByteStream, limit: usize) -> Result<Bytes, StorageError> {
    let mut bytes = bytes::BytesMut::with_capacity(limit);
    let mut inner = stream.into_inner();
    while let Some(chunk) = inner.next().await {
        let chunk = chunk?;
        if chunk.len() > limit - bytes.len() {
            return Err(probe_mismatch());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
}

fn probe_mismatch() -> StorageError {
    remote_error(
        StorageProviderErrorKind::Protocol,
        "probe",
        "probe file contents did not match",
        false,
    )
}

/// Converts between the SMB wire GUID and its standard UUID form.
fn guid_from_uuid(uuid: Uuid) -> smb::Guid {
    smb::Guid::from(uuid.to_bytes_le())
}

fn uuid_from_guid(guid: smb::Guid) -> Uuid {
    Uuid::from_bytes_le(<[u8; 16]>::from(guid))
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

/// Derives the unique temporary key `put_blob` writes before renaming.
///
/// The temporary name is `.<uuid>.uploading` beside the target, independent of
/// the target's name: a random v4 UUID keeps it collision-safe, and every
/// valid name component therefore fits the server's 255-unit limit. The
/// caller's key is checked against [`MAX_KEY_BYTES`] first, so a key the
/// backend accepts is never refused because of its temporary name.
fn temp_key(key: &str) -> Result<String, StorageError> {
    check_key(key)?;
    let (parent, _) = key.rsplit_once('/').unwrap_or(("", key));
    let temp_name = format!(".{}{TEMP_SUFFIX}", Uuid::new_v4());
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
            // The client rejected an unsigned or unencrypted response before
            // evaluating its status. Its operation's outcome is unknown.
            smb::Error::UnverifiedMessage(_) => StorageProviderErrorKind::SecurityNegotiation,
            // The server's identity changed; it was refused before any
            // credential was sent. Retrying cannot succeed.
            smb::Error::ServerGuidMismatch => StorageProviderErrorKind::SecurityNegotiation,
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

/// Whether the server reported that a path does not exist.
///
/// `blob_exists`, `head_blob` and deletion treat this as authoritative. That is
/// sound only because the connection refuses guest or anonymous sessions, and
/// the vendored client verifies a response's signature or encryption before it
/// evaluates its status (GHSA-439h-2cx8-xwgx), so an on-path party cannot
/// forge a not-found status.
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

    fn not_found() -> StorageError {
        remote_error(
            StorageProviderErrorKind::RemotePathNotFound,
            "list",
            "Object Name Not Found",
            false,
        )
    }

    #[test]
    fn listing_skips_only_a_missing_configured_root() {
        listing_open_failure("", false, not_found()).expect("missing root lists as empty");
        listing_open_failure("root", false, not_found()).expect("missing root lists as empty");
        listing_not_directory("root", false).expect("root file lists as empty");
    }

    #[test]
    fn listing_fails_closed_for_a_missing_enumerated_directory() {
        let error = listing_open_failure("snapshots/1234", true, not_found())
            .expect_err("an enumerated directory must not be skipped");
        assert_eq!(
            provider_error_kind(&error),
            Some(StorageProviderErrorKind::Protocol)
        );
        assert!(!error.is_retryable());
        assert!(!is_not_found(&error), "must not read as an absent key");
        assert!(error.to_string().contains("listing is incomplete"));

        let error = listing_not_directory("snapshots/1234", true)
            .expect_err("an enumerated directory must open as a directory");
        assert!(error.to_string().contains("listing is incomplete"));
    }

    #[test]
    fn listing_propagates_other_open_failures() {
        for discovered in [false, true] {
            let error = listing_open_failure(
                "objects",
                discovered,
                remote_error(
                    StorageProviderErrorKind::PermissionDenied,
                    "list",
                    "Access Denied",
                    false,
                ),
            )
            .expect_err("permission failures are not skipped");
            assert_eq!(
                provider_error_kind(&error),
                Some(StorageProviderErrorKind::PermissionDenied)
            );
        }
    }

    #[test]
    fn rejected_unverified_responses_are_security_failures() {
        let error = map_smb_error(
            "head",
            smb::Error::UnverifiedMessage("not signed".to_string()),
        );
        assert_eq!(
            provider_error_kind(&error),
            Some(StorageProviderErrorKind::SecurityNegotiation)
        );
        assert!(!error.is_retryable());
        assert!(!is_not_found(&error));

        // Other unexpected messages stay protocol failures.
        let error = map_smb_error(
            "head",
            smb::Error::InvalidMessage("Expected server-to-redir message".to_string()),
        );
        assert_eq!(
            provider_error_kind(&error),
            Some(StorageProviderErrorKind::Protocol)
        );
    }

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
    fn listing_limits_and_server_pin_defaults_and_validation() {
        let defaults = config();
        assert_eq!(defaults.max_listing_depth, DEFAULT_MAX_LISTING_DEPTH);
        assert_eq!(defaults.max_listing_entries, DEFAULT_MAX_LISTING_ENTRIES);
        assert_eq!(defaults.listing_timeout, DEFAULT_LISTING_TIMEOUT);
        assert_eq!(defaults.max_listing_bytes, DEFAULT_MAX_LISTING_BYTES);
        assert_eq!(defaults.expected_server_guid, None);
        for update in [
            (|config: &mut SmbStorageConfig| config.max_listing_depth = 0)
                as fn(&mut SmbStorageConfig),
            |config| config.max_listing_entries = 0,
            |config| config.listing_timeout = Duration::ZERO,
            |config| config.max_listing_bytes = 0,
            |config| config.expected_server_guid = Some(Uuid::nil()),
        ] {
            let mut config = config();
            update(&mut config);
            assert!(config.validate().is_err(), "{config:?}");
        }
    }

    /// A scripted directory tree for [`walk_listing`]. It records each entry
    /// in the budget as the SMB source does.
    struct FakeListing {
        entries: fn(&str) -> Vec<(String, bool)>,
        delay: Duration,
        directories_listed: AtomicU64,
    }

    impl FakeListing {
        fn new(entries: fn(&str) -> Vec<(String, bool)>) -> Self {
            Self {
                entries,
                delay: Duration::ZERO,
                directories_listed: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl ListingSource for FakeListing {
        async fn list_directory(
            &self,
            path: &str,
            _discovered: bool,
            budget: &mut ListingBudget,
        ) -> Result<Option<Vec<ListedEntry>>, StorageError> {
            self.directories_listed.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let mut listed = Vec::new();
            for (name, directory) in (self.entries)(path) {
                budget.record_entry(&name)?;
                listed.push(ListedEntry { name, directory });
            }
            Ok(Some(listed))
        }
    }

    fn limits(max_depth: usize, max_entries: usize, timeout: Duration) -> ListingLimits {
        ListingLimits {
            max_depth,
            max_entries,
            max_bytes: usize::MAX,
            timeout,
        }
    }

    /// Lists as `collect_keys` does, within one deadline.
    async fn bounded_listing<S: ListingSource>(
        source: &S,
        root: String,
        limits: ListingLimits,
    ) -> Result<Vec<String>, StorageError> {
        within_listing_deadline(limits.timeout, walk_listing(source, root, limits)).await
    }

    fn assert_partial_listing_refused(error: &StorageError, reason: &str) {
        match error {
            StorageError::RemoteProvider {
                operation,
                message,
                retryable,
                ..
            } => {
                assert_eq!(operation, "list");
                assert!(!retryable, "a listing limit must not be retried: {message}");
                assert!(message.contains(reason), "{message}");
                assert!(message.contains("refusing to return a partial listing"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn self_nesting_directory_listing_terminates_at_the_depth_limit() {
        // Every directory contains itself, as a server-side loop would.
        let source = FakeListing::new(|_| vec![("loop".to_string(), true)]);
        let error = bounded_listing(
            &source,
            "root".to_string(),
            limits(16, usize::MAX, Duration::from_secs(30)),
        )
        .await
        .expect_err("a self-nesting listing must fail");
        assert_partial_listing_refused(&error, "maximum directory depth of 16");
        assert_eq!(source.directories_listed.load(Ordering::SeqCst), 17);
    }

    #[tokio::test]
    async fn listing_entry_limit_bounds_a_flooding_directory() {
        let source = FakeListing::new(|path| {
            if path.is_empty() {
                (0..10)
                    .map(|index| (format!("directory-{index}"), true))
                    .collect()
            } else {
                (0..1_000)
                    .map(|index| (format!("file-{index}"), false))
                    .collect()
            }
        });
        let error = bounded_listing(
            &source,
            String::new(),
            limits(64, 2_500, Duration::from_secs(30)),
        )
        .await
        .expect_err("an oversized listing must fail");
        assert_partial_listing_refused(&error, "more than 2500 entries");
        assert_eq!(source.directories_listed.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn listing_deadline_bounds_a_slow_endless_listing() {
        let mut source = FakeListing::new(|_| vec![("loop".to_string(), true)]);
        source.delay = Duration::from_millis(5);
        let started = Instant::now();
        let error = bounded_listing(
            &source,
            String::new(),
            limits(usize::MAX, usize::MAX, Duration::from_millis(100)),
        )
        .await
        .expect_err("a listing that cannot finish must time out");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_partial_listing_refused(&error, "did not complete within");
        assert!(matches!(
            error,
            StorageError::RemoteProvider {
                kind: StorageProviderErrorKind::Timeout,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn listing_byte_budget_bounds_long_nested_names() {
        // Deeply nested long names, then many short names in the deepest
        // directory: every key repeats the long parent path. The names alone
        // fit the budget; the repeated parent path does not.
        const DEPTH: usize = 8;
        const FILES: usize = 2_000;
        let source = FakeListing::new(|path| {
            let depth = path.split('/').filter(|part| !part.is_empty()).count();
            if depth < DEPTH {
                vec![("n".repeat(200), true)]
            } else {
                (0..FILES)
                    .map(|index| (format!("{index}"), false))
                    .collect()
            }
        });
        let names: usize = DEPTH * (200 + LISTED_PATH_OVERHEAD)
            + (0..FILES)
                .map(|index| format!("{index}").len() + LISTED_PATH_OVERHEAD)
                .sum::<usize>();
        let parent_path = DEPTH * 201;
        let budget = 1024 * 1024;
        assert!(names < budget / 4, "the names alone must fit the budget");
        assert!(names + FILES * parent_path > budget);

        let mut limits = limits(64, usize::MAX, Duration::from_secs(30));
        limits.max_bytes = budget;
        let error = bounded_listing(&source, String::new(), limits)
            .await
            .expect_err("a listing larger than its byte budget must fail");
        assert_partial_listing_refused(&error, "more than 1048576 bytes of names");

        // The same tree with short directory names fits.
        let short = FakeListing::new(|path| {
            let depth = path.split('/').filter(|part| !part.is_empty()).count();
            if depth < DEPTH {
                vec![("d".to_string(), true)]
            } else {
                (0..FILES)
                    .map(|index| (format!("{index}"), false))
                    .collect()
            }
        });
        let files = bounded_listing(&short, String::new(), limits)
            .await
            .expect("the short-named tree fits");
        assert_eq!(files.len(), FILES);
    }

    #[tokio::test]
    async fn listing_rejects_a_name_longer_than_a_component() {
        // 255 UTF-16 code units is the MS-FSCC limit; a supplementary-plane
        // character is two units.
        let source = FakeListing::new(|_| vec![("\u{1F600}".repeat(128), false)]);
        let error = bounded_listing(
            &source,
            String::new(),
            limits(64, 10, Duration::from_secs(30)),
        )
        .await
        .expect_err("an over-long name must fail the listing");
        assert_partial_listing_refused(&error, "longer than 255 UTF-16 code units");

        let source = FakeListing::new(|_| vec![("n".repeat(255), false)]);
        let files = bounded_listing(
            &source,
            String::new(),
            limits(64, 10, Duration::from_secs(30)),
        )
        .await
        .expect("a name at the limit is listed");
        assert_eq!(files.len(), 1);
    }

    #[tokio::test]
    async fn listing_rejects_a_key_longer_than_the_maximum() {
        let source = FakeListing::new(|_| vec![("n".repeat(250), true)]);
        let error = bounded_listing(
            &source,
            String::new(),
            limits(64, usize::MAX, Duration::from_secs(30)),
        )
        .await
        .expect_err("an over-long path must fail the listing");
        assert_partial_listing_refused(&error, "path longer than 4096 bytes");
    }

    #[tokio::test]
    async fn listing_keys_are_relative_to_the_root_prefix() {
        let source = FakeListing::new(|path| match path {
            "repositories/nightly" => vec![("a".to_string(), true), ("top.bin".to_string(), false)],
            "repositories/nightly/a" => vec![("one.bin".to_string(), false)],
            other => panic!("unexpected directory {other}"),
        });
        let mut files = bounded_listing(
            &source,
            "repositories/nightly".to_string(),
            limits(8, 100, Duration::from_secs(30)),
        )
        .await
        .expect("listing");
        files.sort();
        assert_eq!(files, ["a/one.bin", "top.bin"]);
    }

    #[tokio::test]
    async fn one_deadline_covers_the_listing_retry() {
        let attempts = AtomicU64::new(0);
        let started = Instant::now();
        let result: Result<(), StorageError> = within_listing_deadline(
            Duration::from_millis(150),
            retry_listing_once(|| async {
                attempts.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                Err(remote_error(
                    StorageProviderErrorKind::ConnectionLost,
                    "list",
                    "connection lost",
                    true,
                ))
            }),
        )
        .await;
        // Per-attempt deadlines would allow 2 x 100 ms and return the
        // connection error; one deadline stops the retry at 150 ms.
        assert!(started.elapsed() < Duration::from_millis(195));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let error = result.expect_err("the listing must time out");
        assert_partial_listing_refused(&error, "did not complete within");
    }

    #[tokio::test]
    async fn listing_retries_once_after_a_retryable_failure() {
        let attempts = AtomicU64::new(0);
        let result = retry_listing_once(|| async {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(remote_error(
                    StorageProviderErrorKind::ConnectionLost,
                    "list",
                    "connection lost",
                    true,
                ))
            } else {
                Ok(7)
            }
        })
        .await;
        assert_eq!(result.expect("second attempt"), 7);
        let attempts = AtomicU64::new(0);
        let result: Result<(), StorageError> = retry_listing_once(|| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(listing_limit_exceeded("limit".to_string()))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    fn keys(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|index| format!("k{index:06}")).collect()
    }

    #[test]
    fn paged_listing_returns_every_key_once() {
        let all = keys(0..10);
        let mut collected = Vec::new();
        let mut continuation = None;
        loop {
            let page =
                listing_page(all.clone(), "", continuation.as_deref(), 3, 100).expect("page");
            collected.extend(page.keys);
            continuation = page.next_continuation;
            if continuation.is_none() {
                break;
            }
        }
        assert_eq!(collected, all);
        let prefixed =
            listing_page(vec!["a/1".into(), "b/1".into()], "b", None, 10, 100).expect("prefix");
        assert_eq!(prefixed.keys, ["b/1"]);
        assert!(listing_page(all, "", Some("k000003"), 3, 100).is_err());
    }

    #[test]
    fn paged_listing_ends_when_names_keep_appearing() {
        // Every walk returns more names than a page after the token, as a
        // server adding names between pages would.
        let mut continuation: Option<String> = None;
        let mut walks = 0_usize;
        let error = loop {
            walks += 1;
            assert!(walks <= 100, "the paged listing never ended");
            let start = walks * 1_000;
            match listing_page(
                keys(0..start + 1_001),
                "",
                continuation.as_deref(),
                1_000,
                20_000,
            ) {
                Ok(page) => continuation = page.next_continuation,
                Err(error) => break error,
            }
            assert!(continuation.is_some());
        };
        assert_partial_listing_refused(&error, "paged listing returned more than 20000 keys");
        assert_eq!(walks, 21);

        // A small page limit is bounded by the page cap.
        let mut continuation: Option<String> = None;
        let mut walks = 0_u64;
        let error = loop {
            walks += 1;
            match listing_page(keys(0..2_000), "", continuation.as_deref(), 1, 1_000_000) {
                Ok(page) => continuation = page.next_continuation,
                Err(error) => break error,
            }
        };
        assert_partial_listing_refused(&error, "exceeded 1024 pages");
        assert_eq!(walks, MAX_LISTING_PAGES + 1);
    }

    #[test]
    fn paged_listing_total_is_bounded_by_the_entry_limit() {
        let mut continuation: Option<String> = None;
        let error = loop {
            match listing_page(keys(0..50), "", continuation.as_deref(), 10, 25) {
                Ok(page) => continuation = page.next_continuation,
                Err(error) => break error,
            }
        };
        assert_partial_listing_refused(&error, "returned more than 25 keys");
    }

    #[test]
    fn an_exhausted_or_foreign_token_is_refused_without_listing() {
        // `list_blobs_page` checks this before it lists the tree.
        let last = ListingContinuation {
            pages: MAX_LISTING_PAGES,
            returned: 1,
            after: "k".to_string(),
        };
        let error = ListingContinuation::parse(&last.encode())
            .and_then(|position| position.next_page())
            .expect_err("the chain is exhausted");
        assert_partial_listing_refused(&error, "exceeded 1024 pages");
        assert!(matches!(
            ListingContinuation::parse("plain-key"),
            Err(StorageError::PreconditionFailed { .. })
        ));
    }

    #[test]
    fn continuation_tokens_round_trip_keys_with_separators() {
        let position = ListingContinuation {
            pages: 3,
            returned: 42,
            after: "a:b/c".to_string(),
        };
        assert_eq!(
            ListingContinuation::parse(&position.encode()).unwrap(),
            position
        );
        assert!(ListingContinuation::parse("a:b/c").is_err());
        assert!(ListingContinuation::parse("smb1:x:1:a").is_err());
    }

    #[tokio::test]
    async fn listing_within_limits_returns_every_file() {
        let source = FakeListing::new(|path| match path {
            "" => vec![("a".to_string(), true), ("top.bin".to_string(), false)],
            "a" => vec![("b".to_string(), true), ("one.bin".to_string(), false)],
            "a/b" => vec![("two.bin".to_string(), false)],
            other => panic!("unexpected directory {other}"),
        });
        // Exactly at the limits: depth 2 and 5 entries.
        let mut files = bounded_listing(
            &source,
            String::new(),
            limits(2, 5, Duration::from_secs(30)),
        )
        .await
        .expect("listing within limits");
        files.sort();
        assert_eq!(files, ["a/b/two.bin", "a/one.bin", "top.bin"]);
    }

    #[tokio::test]
    async fn probe_read_is_bounded_whatever_size_the_server_declares() {
        let pulled = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&pulled);
        // An endless body that claims to be as large as possible.
        let body = stream::repeat_with(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Bytes::from_static(&[0x5a; 10]))
        });
        let stream = StorageByteStream::with_size_hint(Box::pin(body), u64::MAX);
        let error = collect_bounded(stream, 16)
            .await
            .expect_err("a body larger than the probe must be rejected");
        assert!(matches!(
            error,
            StorageError::RemoteProvider {
                kind: StorageProviderErrorKind::Protocol,
                retryable: false,
                ..
            }
        ));
        assert_eq!(pulled.load(Ordering::SeqCst), 2);

        let exact = StorageByteStream::with_size_hint(
            Box::pin(stream::iter([
                Ok(Bytes::from_static(&[1; 10])),
                Ok(Bytes::from_static(&[2; 6])),
            ])),
            u64::MAX,
        );
        assert_eq!(collect_bounded(exact, 16).await.expect("exact").len(), 16);
    }

    #[test]
    fn server_guid_conversion_round_trips_in_standard_form() {
        let uuid = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap();
        let guid = guid_from_uuid(uuid);
        // The wire form stores the first three fields little-endian.
        assert_eq!(
            <[u8; 16]>::from(guid),
            [
                0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ]
        );
        assert_eq!(uuid_from_guid(guid), uuid);
    }

    #[test]
    fn client_configuration_pins_the_server_and_disables_dfs_and_notifications() {
        let guid = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap();
        let client = client_config(&config(), Some(guid));
        assert!(!client.dfs, "DFS referrals must not redirect the client");
        assert!(client.connection.disable_notifications);
        assert!(!client.connection.allow_unsigned_guest_access);
        assert!(client.connection.smb2_only_negotiate);
        assert_eq!(
            client.connection.expected_server_guid,
            Some(guid_from_uuid(guid))
        );
        assert_eq!(
            client_config(&config(), None)
                .connection
                .expected_server_guid,
            None
        );
    }

    #[test]
    fn changed_server_identity_is_a_non_retryable_security_failure() {
        let error = map_smb_error("connect", smb::Error::ServerGuidMismatch);
        assert!(!error.is_retryable());
        assert_eq!(
            provider_error_kind(&error),
            Some(StorageProviderErrorKind::SecurityNegotiation)
        );
    }

    #[test]
    fn a_failed_transport_is_reported_as_a_lost_connection() {
        // The worker closes a connection whose transport failed; later requests
        // fail with ConnectionStopped and trigger a bounded reconnect.
        let error = map_smb_error("read", smb::Error::ConnectionStopped);
        assert!(error.is_retryable());
        assert_eq!(
            provider_error_kind(&error),
            Some(StorageProviderErrorKind::ConnectionLost)
        );
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
        assert_eq!(
            map_key(Some("repositories/nightly"), "objects/data").expect("map"),
            "repositories/nightly/objects/data"
        );
        assert_eq!(map_key(None, "objects/data").expect("map"), "objects/data");
    }

    /// A key of exactly `bytes` bytes made of valid components.
    fn key_of(bytes: usize) -> String {
        let mut key = String::new();
        let mut remaining = bytes;
        loop {
            let mut take = remaining.min(MAX_NAME_UTF16_UNITS);
            // Leave room for at least "/k" after a separator.
            if remaining - take == 1 {
                take -= 1;
            }
            key.push_str(&"k".repeat(take));
            remaining -= take;
            if remaining == 0 {
                break;
            }
            key.push('/');
            remaining -= 1;
        }
        assert_eq!(key.len(), bytes);
        key
    }

    #[test]
    fn keys_longer_than_a_listable_key_are_never_written() {
        let longest = key_of(MAX_KEY_BYTES);
        assert!(check_key(&longest).is_ok());
        assert!(map_key(Some("root"), &longest).is_ok());
        let longer = key_of(MAX_KEY_BYTES + 1);
        assert!(matches!(
            check_key(&longer),
            Err(StorageError::InvalidStorageKey { .. })
        ));
        assert!(map_key(Some("root"), &longer).is_err());
    }

    #[test]
    fn put_accepts_every_key_put_if_not_exists_accepts() {
        // put_blob derives a longer temporary key from the caller's key; a key
        // at the limit must still be accepted, and an error names the caller's
        // key, never the temporary one.
        for bytes in [MAX_KEY_BYTES - 48, MAX_KEY_BYTES - 1, MAX_KEY_BYTES] {
            let key = key_of(bytes);
            let temp = temp_key(&key).expect("put_blob accepts the key");
            assert!(is_temp_key(&temp));
            assert!(temp.len() <= MAX_KEY_BYTES + TEMP_KEY_EXTRA_BYTES);
            map_key(Some("root"), &temp).expect("the temporary key maps");
            check_key(&key).expect("put_blob_if_not_exists accepts the key");
        }
        let too_long = key_of(MAX_KEY_BYTES + 1);
        match temp_key(&too_long) {
            Err(StorageError::InvalidStorageKey { key }) => assert_eq!(key, too_long),
            other => panic!("unexpected result: {other:?}"),
        }
        // A one-byte name produces the longest temporary key.
        let mut short_name = key_of(MAX_KEY_BYTES - 2);
        short_name.push_str("/k");
        let temp = temp_key(&short_name).expect("temporary key");
        assert!(temp.len() <= MAX_KEY_BYTES + TEMP_KEY_EXTRA_BYTES);
        map_key(None, &temp).expect("the longest temporary key maps");
    }

    #[test]
    fn every_valid_component_fits_its_temporary_name() {
        let name = "n".repeat(MAX_NAME_UTF16_UNITS);
        let temp = temp_key(&format!("dir/{name}")).expect("a 255-unit name is accepted");
        let temp_name = temp.rsplit('/').next().unwrap();
        assert!(temp_name.encode_utf16().count() <= MAX_NAME_UTF16_UNITS);
        // A component longer than the server allows is refused up front.
        let too_long = "\u{1F600}".repeat(128);
        assert!(matches!(
            temp_key(&format!("dir/{too_long}")),
            Err(StorageError::InvalidStorageKey { .. })
        ));
        assert!(check_key(&format!("{too_long}/x")).is_err());
    }

    #[test]
    fn interrupted_uploads_at_the_key_limit_do_not_fail_a_listing() {
        let key = key_of(MAX_KEY_BYTES);
        let temp = temp_key(&key).expect("temporary key");
        assert!(temp.len() <= key_byte_limit(&temp));
        assert_eq!(key_byte_limit(&key), MAX_KEY_BYTES);
    }

    #[test]
    fn temporary_names_are_filtered_strictly() {
        let temp = temp_key("nested/blob.bin").expect("safe temp key");
        assert!(is_temp_key(&temp));
        // Temporary files left by earlier versions remain recognised.
        assert!(is_temp_key(&format!(
            "nested/.blob.bin.{}{TEMP_SUFFIX}",
            Uuid::new_v4()
        )));
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
    #[tokio::test]
    async fn single_attempt_delete_never_reopens_after_ambiguous_disposition() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let opens = AtomicUsize::new(0);
        let replacement_alive = std::sync::atomic::AtomicBool::new(false);
        let result = remove_file_without_replay(
            async {
                opens.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |()| async {
                // Disposition deleted the old object and another writer acquired.
                replacement_alive.store(true, Ordering::SeqCst);
                Err(remote_error(
                    StorageProviderErrorKind::Timeout,
                    "close",
                    "response lost after disposition",
                    true,
                ))
            },
        )
        .await;
        let error = result.expect_err("ambiguous failure");
        assert!(!error.is_retryable());
        let StorageError::SingleAttemptFailed { source } = error else {
            panic!("original failure must be preserved as a source");
        };
        assert!(source.is_retryable());
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert!(replacement_alive.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn exclusive_write_cleanup_preserves_replacement_after_original_handle_closes() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let original_closed = AtomicBool::new(false);
        let replacement_alive = AtomicBool::new(false);
        let path_deletes = AtomicUsize::new(0);
        cleanup_write_target(
            &WriteCleanupTarget::ExclusiveHandle,
            async {
                original_closed.store(true, Ordering::SeqCst);
                // Closing the delete-on-close handle removes the old object;
                // another writer can now atomically acquire the shared key.
                replacement_alive.store(true, Ordering::SeqCst);
            },
            |_| async {
                path_deletes.fetch_add(1, Ordering::SeqCst);
                replacement_alive.store(false, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
        assert!(original_closed.load(Ordering::SeqCst));
        assert_eq!(path_deletes.load(Ordering::SeqCst), 0);
        assert!(replacement_alive.load(Ordering::SeqCst));
    }
}
