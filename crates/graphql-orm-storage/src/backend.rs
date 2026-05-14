use std::str::FromStr;

use crate::StorageError;

/// Storage provider identifiers understood by this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StorageBackend {
    /// Local filesystem object storage.
    Local,
    /// S3-compatible object storage.
    S3,
    /// Azure Blob Storage.
    AzureBlob,
}

impl StorageBackend {
    /// Returns the stable string representation used in persisted metadata.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
            Self::AzureBlob => "azure_blob",
        }
    }
}

impl FromStr for StorageBackend {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "s3" | "s3-compatible" | "s3_compatible" => Ok(Self::S3),
            "azure_blob" | "azure-blob" | "azureblob" => Ok(Self::AzureBlob),
            other => Err(StorageError::UnsupportedBackend {
                backend: other.to_string(),
            }),
        }
    }
}

/// Logical storage namespace used as the first path segment of generated keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StorageNamespace {
    /// Original uploaded objects.
    Originals,
    /// Objects retained before permanent deletion.
    RecycleBin,
    /// Generated thumbnail objects.
    Thumbnails,
    /// Generated derivative objects.
    Derivatives,
    /// Exported objects.
    Exports,
    /// Temporary objects.
    Temp,
}

impl StorageNamespace {
    /// Returns the stable string representation used in storage keys.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Originals => "originals",
            Self::RecycleBin => "recycle_bin",
            Self::Thumbnails => "thumbnails",
            Self::Derivatives => "derivatives",
            Self::Exports => "exports",
            Self::Temp => "temp",
        }
    }
}

/// Builds an unsupported-backend error for a known provider.
#[must_use]
pub fn unsupported_backend(backend: StorageBackend) -> StorageError {
    StorageError::UnsupportedBackend {
        backend: backend.as_str().to_string(),
    }
}
