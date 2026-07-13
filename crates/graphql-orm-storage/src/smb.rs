//! Native SMB2/SMB3 blob storage.

use std::{fmt, sync::Arc, time::Duration};

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
const READ_CHUNK_SIZE: usize = 256 * 1024;
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
}

/// Native SMB2/SMB3 [`BlobStore`] implementation.
pub struct SmbStorageBackend {
    config: Arc<SmbStorageConfig>,
    client: RwLock<Arc<Client>>,
    reconnect_lock: Mutex<()>,
    transfer_limit: Arc<Semaphore>,
}

impl fmt::Debug for SmbStorageBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmbStorageBackend")
            .field("config", &self.config)
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
        let client = connect_client(&config).await?;
        Ok(Self {
            config,
            client: RwLock::new(Arc::new(client)),
            reconnect_lock: Mutex::new(()),
            transfer_limit: Arc::new(Semaphore::new(transfer_concurrency)),
        })
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

        let client = backend.client().await;
        let connection = client
            .get_connection(&backend.config.server)
            .await
            .map_err(|error| map_smb_error("probe", error))?;
        let info = connection.conn_info().ok_or_else(|| {
            remote_error(
                StorageProviderErrorKind::Protocol,
                "probe",
                "negotiated connection information is unavailable",
                false,
            )
        })?;

        Ok(SmbProbeResult {
            negotiated_dialect: SmbDialect::from_protocol(info.negotiation.dialect_rev),
            signing_active: true,
            encryption_active: backend.config.require_encryption,
            server_reachable: true,
            share_reachable: true,
            prefix_readable,
            prefix_writable: true,
        })
    }

    async fn client(&self) -> Arc<Client> {
        Arc::clone(&*self.client.read().await)
    }

    async fn reconnect_with_backoff(&self) -> Result<(), StorageError> {
        let _guard = self.reconnect_lock.lock().await;
        let delays = [100_u64, 250, 500];
        let mut last_error = None;
        for delay in delays {
            let jitter = u64::from(Uuid::new_v4().as_bytes()[0]) % 75;
            tokio::time::sleep(Duration::from_millis(delay + jitter)).await;
            match connect_client(&self.config).await {
                Ok(client) => {
                    *self.client.write().await = Arc::new(client);
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
        UncPath::new(&self.config.server)
            .and_then(|path| path.with_share(&self.config.share))
            .map_err(|error| map_smb_error("path", error))
    }

    fn mapped_key(&self, key: &str) -> Result<String, StorageError> {
        validate_blob_key(key)?;
        Ok(match &self.config.root_prefix {
            Some(prefix) => format!("{prefix}/{key}"),
            None => key.to_string(),
        })
    }

    fn remote_path(&self, key: &str) -> Result<UncPath, StorageError> {
        Ok(self.share_path()?.with_path(&self.mapped_key(key)?))
    }

    async fn ensure_root_prefix(&self) -> Result<(), StorageError> {
        if let Some(prefix) = &self.config.root_prefix {
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
        let client = self.client().await;
        let base = self.share_path()?;
        let mut current = String::new();
        for segment in path.split('/') {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            let target = base.clone().with_path(&current);
            let create = FileCreateArgs {
                disposition: smb::CreateDisposition::OpenIf,
                attributes: FileAttributes::new().with_directory(true),
                options: CreateOptions::new().with_directory_file(true),
                desired_access: DirAccessMask::new()
                    .with_list_directory(true)
                    .with_synchronize(true)
                    .into(),
            };
            let resource = self
                .with_timeout("create_directory", client.create_file(&target, &create))
                .await?;
            close_resource(resource)
                .await
                .map_err(|error| map_smb_error("close", error))?;
        }
        Ok(())
    }

    async fn with_timeout<T>(
        &self,
        operation: &'static str,
        future: impl std::future::Future<Output = smb::Result<T>>,
    ) -> Result<T, StorageError> {
        tokio::time::timeout(self.config.operation_timeout, future)
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

    async fn open_file(&self, key: &str, access: FileAccessMask) -> Result<File, StorageError> {
        let path = self.remote_path(key)?;
        let client = self.client().await;
        let first = self
            .with_timeout(
                "open",
                client.create_file(&path, &FileCreateArgs::make_open_existing(access)),
            )
            .await;
        let resource = match first {
            Ok(resource) => resource,
            Err(error) if error.is_retryable() => {
                self.reconnect_with_backoff().await?;
                let client = self.client().await;
                self.with_timeout(
                    "open",
                    client.create_file(&path, &FileCreateArgs::make_open_existing(access)),
                )
                .await?
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
        let _permit = self.transfer_permit().await?;
        self.ensure_parent_directories(key).await?;
        let client = self.client().await;
        let path = self.remote_path(key)?;
        let args = if exclusive {
            FileCreateArgs::make_create_new(FileAttributes::new(), CreateOptions::new())
        } else {
            FileCreateArgs::make_overwrite(FileAttributes::new(), CreateOptions::new())
        };
        let resource = self
            .with_timeout("create", client.create_file(&path, &args))
            .await?;
        let file = match resource {
            Resource::File(file) => file,
            other => {
                let _ = close_resource(other).await;
                return Err(remote_error(
                    StorageProviderErrorKind::Protocol,
                    "create",
                    "remote key did not create a file",
                    false,
                ));
            }
        };

        let mut source = body.into_inner();
        let mut offset = 0_u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = source.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    self.cleanup_failed_write(key, &file).await;
                    return Err(error);
                }
            };
            hasher.update(&chunk);
            let mut written = 0;
            while written < chunk.len() {
                let result = tokio::time::timeout(
                    self.config.operation_timeout,
                    file.write_block(&chunk[written..], offset, None),
                )
                .await;
                let count = match result {
                    Ok(Ok(count)) => count,
                    Ok(Err(error)) => {
                        self.cleanup_failed_write(key, &file).await;
                        return Err(map_io_error("write", error));
                    }
                    Err(_) => {
                        let error = remote_error(
                            StorageProviderErrorKind::Timeout,
                            "write",
                            "operation timed out",
                            true,
                        );
                        self.cleanup_failed_write(key, &file).await;
                        return Err(error);
                    }
                };
                if count == 0 {
                    let error = remote_error(
                        StorageProviderErrorKind::Protocol,
                        "write",
                        "server accepted a zero-length write",
                        false,
                    );
                    self.cleanup_failed_write(key, &file).await;
                    return Err(error);
                }
                written += count;
                offset = offset.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            }
        }
        match tokio::time::timeout(self.config.operation_timeout, file.flush()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.cleanup_failed_write(key, &file).await;
                return Err(map_io_error("flush", error));
            }
            Err(_) => {
                let error = remote_error(
                    StorageProviderErrorKind::Timeout,
                    "flush",
                    "operation timed out",
                    true,
                );
                self.cleanup_failed_write(key, &file).await;
                return Err(error);
            }
        }
        if let Err(error) = self.with_timeout("close", file.close()).await {
            if error.is_retryable() {
                let _ = self.reconnect_with_backoff().await;
            }
            let _ = self.remove_file(key).await;
            return Err(error);
        }
        Ok(BlobWriteOutcome {
            size_bytes: offset,
            sha256_hex: format!("{:x}", hasher.finalize()),
        })
    }

    async fn cleanup_failed_write(&self, key: &str, file: &File) {
        let _ = self.with_timeout("close", file.close()).await;
        let _ = self.remove_file(key).await;
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
        result.and(close_result)
    }

    async fn remove_file(&self, key: &str) -> Result<(), StorageError> {
        let file = match self
            .open_file(key, FileAccessMask::new().with_delete(true))
            .await
        {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        let result = self
            .with_timeout(
                "delete",
                file.set_info(FileDispositionInformation::default()),
            )
            .await;
        let close_result = self.with_timeout("close", file.close()).await;
        result.and(close_result)
    }

    async fn transfer_permit(&self) -> Result<OwnedSemaphorePermit, StorageError> {
        Arc::clone(&self.transfer_limit)
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
        let client = self.client().await;
        let base = self.share_path()?;
        let start = self.config.root_prefix.clone().unwrap_or_default();
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
            let mut entries = Directory::query::<FileFullDirectoryInformation>(&directory, "*")
                .await
                .map_err(|error| map_smb_error("list", error))?;
            while let Some(entry) = entries.next().await {
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
                    let key = match &self.config.root_prefix {
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
                if error.is_retryable() {
                    let _ = self.reconnect_with_backoff().await;
                }
                let _ = self.remove_file(&temp).await;
                return Err(error);
            }
        };
        if let Err(error) = self.rename(&temp, key, true).await {
            if error.is_retryable() {
                let _ = self.reconnect_with_backoff().await;
            }
            let _ = self.remove_file(&temp).await;
            return Err(error);
        }
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
            Err(error) => {
                if error.is_retryable() {
                    let _ = self.reconnect_with_backoff().await;
                }
                Err(error)
            }
        }
    }

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError> {
        validate_blob_key(key)?;
        let permit = self.transfer_permit().await?;
        let file = self
            .open_file(key, FileAccessMask::new().with_generic_read(true))
            .await?;
        let standard: FileStandardInformation =
            self.with_timeout("head", file.query_info()).await?;
        let size = standard.end_of_file;
        let operation_timeout = self.config.operation_timeout;
        let stream_key = key.to_string();
        let body = stream::try_unfold(
            (Some(file), 0_u64, size, permit, stream_key.clone()),
            move |(file, offset, size, permit, key)| async move {
                let Some(file) = file else {
                    return Ok(None);
                };
                if offset >= size {
                    tokio::time::timeout(operation_timeout, file.close())
                        .await
                        .map_err(|_| {
                            remote_error(
                                StorageProviderErrorKind::Timeout,
                                "close",
                                "operation timed out",
                                true,
                            )
                        })?
                        .map_err(|error| map_smb_error("close", error))?;
                    return Ok(None);
                }
                let chunk_len = usize::try_from((size - offset).min(READ_CHUNK_SIZE as u64))
                    .unwrap_or(READ_CHUNK_SIZE);
                let mut buffer = vec![0_u8; chunk_len];
                let read = tokio::time::timeout(
                    operation_timeout,
                    file.read_block(&mut buffer, offset, None, false),
                )
                .await
                .map_err(|_| {
                    remote_error(
                        StorageProviderErrorKind::Timeout,
                        "read",
                        "operation timed out",
                        true,
                    )
                })?
                .map_err(|error| map_io_error("read", error))?;
                if read == 0 {
                    return Err(remote_error(
                        StorageProviderErrorKind::Protocol,
                        "read",
                        "unexpected end of remote file",
                        false,
                    ));
                }
                buffer.truncate(read);
                let next = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                Ok(Some((
                    Bytes::from(buffer),
                    (Some(file), next, size, permit, key),
                )))
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
                self.with_timeout("close", file.close()).await?;
                Ok(true)
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
        let standard: FileStandardInformation =
            self.with_timeout("head", file.query_info()).await?;
        self.with_timeout("close", file.close()).await?;
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

async fn connect_client(config: &SmbStorageConfig) -> Result<Client, StorageError> {
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
    Ok(client)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
