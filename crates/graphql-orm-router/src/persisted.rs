use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{RouterError, RouterErrorKind};
use sha2::{Digest, Sha256};

const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const DOCUMENT_SIZE_ERROR: &str = "persisted document exceeds 262144 bytes (256 KiB)";
pub(crate) const INTERNAL_FRAME_BYTES: usize = 64 * 1024;

const MAX_MANIFEST_ENTRIES: usize = 10_000;

#[derive(Clone)]
pub(crate) struct PersistedDocuments {
    path: PathBuf,
    pub(crate) strict: bool,
    snapshot: Option<Arc<ManifestSnapshot>>,
}

struct ManifestSnapshot {
    directory: PathBuf,
    bytes: Vec<u8>,
    path: PathBuf,
    documents: std::collections::BTreeMap<String, String>,
}

impl Drop for ManifestSnapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

fn invalid(message: &'static str) -> RouterError {
    RouterError::new(RouterErrorKind::InvalidConfiguration, message)
}

impl PersistedDocuments {
    pub(crate) fn new(path: impl AsRef<Path>, strict: bool) -> Self {
        Self {
            path: path.as_ref().to_owned(),
            strict,
            snapshot: None,
        }
    }

    pub(crate) fn validate(&mut self) -> Result<(), RouterError> {
        if self.snapshot.is_some() {
            return self.verify_snapshot();
        }
        if self.path.to_string_lossy().contains("://") || self.path.as_os_str().is_empty() {
            return Err(invalid(
                "persisted documents require a local manifest file; remote storage is not supported",
            ));
        }
        let file = File::open(&self.path)
            .map_err(|_| invalid("failed to open persisted documents manifest"))?;
        if !file
            .metadata()
            .map_err(|_| invalid("failed to inspect persisted documents manifest"))?
            .is_file()
        {
            return Err(invalid(
                "persisted documents manifest must be a regular local file",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("failed to read persisted documents manifest"))?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(invalid("persisted documents manifest exceeds 16 MiB"));
        }
        let entries: ManifestEntries = serde_json::from_slice(&bytes).map_err(|error| {
            if error.to_string().starts_with(DOCUMENT_SIZE_ERROR) {
                invalid(DOCUMENT_SIZE_ERROR)
            } else {
                invalid("persisted documents manifest must map exact SHA-256 ids to documents, with at most 10000 unique entries and no duplicate keys")
            }
        })?;
        // Hive reads the file when a selected generation initializes its runtime.
        // Keep a private snapshot alive, checking it around both planner and
        // runtime construction instead of rereading the operator's source.
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|_| invalid("failed to initialize persisted documents snapshot"))?;
        let name = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let directory = std::env::temp_dir().join(format!("graphql-orm-persisted-{name}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).map_err(|_| {
            invalid("failed to create private persisted documents snapshot directory")
        })?;
        let path = directory.join("manifest.json");
        let snapshot = Arc::new(ManifestSnapshot {
            directory,
            path,
            documents: entries.0,
            bytes,
        });
        #[cfg(unix)]
        verify_private_metadata(
            &std::fs::metadata(&snapshot.directory)
                .map_err(|_| invalid("failed to inspect persisted documents snapshot directory"))?,
            rustix::process::geteuid().as_raw(),
        )?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options
            .open(&snapshot.path)
            .map_err(|_| invalid("failed to create persisted documents snapshot"))?;
        #[cfg(unix)]
        verify_private_metadata(
            &output
                .metadata()
                .map_err(|_| invalid("failed to inspect persisted documents snapshot file"))?,
            rustix::process::geteuid().as_raw(),
        )?;
        output
            .write_all(&snapshot.bytes)
            .map_err(|_| invalid("failed to write persisted documents snapshot"))?;
        self.snapshot = Some(snapshot);
        Ok(())
    }

    pub(crate) fn verify_snapshot(&self) -> Result<(), RouterError> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| invalid("persisted documents snapshot is not initialized"))?;
        #[cfg(unix)]
        verify_private_metadata(
            &std::fs::metadata(&snapshot.directory).map_err(|_| {
                invalid("persisted documents snapshot directory cannot be inspected")
            })?,
            rustix::process::geteuid().as_raw(),
        )?;
        if !std::fs::metadata(&snapshot.path).is_ok_and(|metadata| metadata.is_file()) {
            return Err(invalid(
                "persisted documents snapshot is missing or is not a regular file",
            ));
        }
        let file = File::open(&snapshot.path)
            .map_err(|_| invalid("persisted documents snapshot cannot be opened"))?;
        #[cfg(unix)]
        verify_private_metadata(
            &file
                .metadata()
                .map_err(|_| invalid("persisted documents snapshot file cannot be inspected"))?,
            rustix::process::geteuid().as_raw(),
        )?;
        let mut bytes = Vec::new();
        file.take(snapshot.bytes.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("persisted documents snapshot cannot be read"))?;
        if bytes != snapshot.bytes {
            return Err(invalid(
                "persisted documents snapshot differs from the validated startup manifest",
            ));
        }
        Ok(())
    }

    pub(crate) fn prepare_websocket_payload(
        &self,
        message: &mut serde_json::Value,
    ) -> Result<(), &'static str> {
        use serde_json::Value;
        let payload = message
            .get_mut("payload")
            .and_then(Value::as_object_mut)
            .ok_or("PERSISTED_DOCUMENT_EXTRACTION_FAILED")?;
        let id = payload
            .get("documentId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .or_else(|| {
                payload
                    .get("extensions")?
                    .get("persistedQuery")?
                    .get("sha256Hash")?
                    .as_str()
                    .filter(|id| !id.is_empty())
            })
            .map(str::to_owned);
        if !self.strict
            && payload
                .get("query")
                .and_then(Value::as_str)
                .is_some_and(|q| !q.is_empty())
        {
            return Ok(());
        }
        let Some(id) = id else {
            return if self.strict {
                Err("PERSISTED_DOCUMENT_ID_REQUIRED")
            } else {
                Ok(())
            };
        };
        let document = self
            .snapshot
            .as_ref()
            .expect("validated manifest")
            .documents
            .get(&id)
            .ok_or("PERSISTED_DOCUMENT_NOT_FOUND")?;
        payload.insert("query".into(), Value::String(document.clone()));
        let extensions = payload
            .entry("extensions")
            .or_insert_with(|| serde_json::json!({}));
        let extensions = extensions
            .as_object_mut()
            .ok_or("PERSISTED_DOCUMENT_EXTRACTION_FAILED")?;
        extensions.insert(
            "persistedQuery".into(),
            serde_json::json!({"version": 1, "sha256Hash": id}),
        );
        Ok(())
    }

    pub(crate) fn hive_config(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": true, "require_id": self.strict,
            "storage": {"type": "file", "path": self.snapshot.as_ref().expect("validated manifest").path, "watch": false},
            "selectors": [
                {"type": "json_path", "path": "documentId"},
                {"type": "json_path", "path": "extensions.persistedQuery.sha256Hash"}
            ]
        })
    }
}

struct ManifestEntries(std::collections::BTreeMap<String, String>);

impl<'de> serde::Deserialize<'de> for ManifestEntries {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ManifestEntries;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an object mapping SHA-256 ids to documents")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                use serde::de::Error as _;
                let mut entries = std::collections::BTreeMap::new();
                while let Some((id, document)) = map.next_entry::<String, String>()? {
                    if entries.len() >= MAX_MANIFEST_ENTRIES {
                        return Err(M::Error::custom("too many manifest entries"));
                    }
                    if document.len() > MAX_DOCUMENT_BYTES {
                        return Err(M::Error::custom(DOCUMENT_SIZE_ERROR));
                    }
                    if id != format!("{:x}", Sha256::digest(document.as_bytes())) {
                        return Err(M::Error::custom(
                            "manifest id does not match exact document SHA-256",
                        ));
                    }
                    if entries.insert(id, document).is_some() {
                        return Err(M::Error::custom("duplicate manifest key"));
                    }
                }
                Ok(ManifestEntries(entries))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

#[cfg(unix)]
pub(crate) fn verify_private_metadata(
    metadata: &std::fs::Metadata,
    effective_uid: u32,
) -> Result<(), RouterError> {
    use std::os::unix::fs::MetadataExt;
    if metadata.mode() & 0o077 != 0 || metadata.uid() != effective_uid {
        return Err(invalid(
            "persisted documents snapshot must belong to the router's effective user with no group or other permissions; the temporary filesystem must honor Unix permissions",
        ));
    }
    Ok(())
}
