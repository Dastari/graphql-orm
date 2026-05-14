use uuid::Uuid;

use crate::StorageNamespace;

/// Builds a sharded, provider-neutral object key.
///
/// Keys use the format `{namespace}/{uuid[0..2]}/{uuid[2..4]}/{uuid}.{extension}`.
#[must_use]
pub fn build_storage_key(
    namespace: StorageNamespace,
    object_id: &Uuid,
    extension: Option<&str>,
) -> String {
    let object_text = object_id.to_string();
    let shard_a = &object_text[0..2];
    let shard_b = &object_text[2..4];
    let file_name = match extension {
        Some(ext) if !ext.is_empty() => format!("{object_text}.{}", ext.to_ascii_lowercase()),
        _ => object_text.clone(),
    };

    format!(
        "{}/{}/{}/{}",
        namespace.as_str(),
        shard_a,
        shard_b,
        file_name
    )
}

/// Extracts a safe filename extension candidate.
///
/// The original filename is never used as a path. This helper copies only the
/// final extension segment and rejects empty, path-like, or NUL-containing
/// extension candidates.
#[must_use]
pub fn file_extension(file_name: &str) -> Option<&str> {
    let candidate = file_name.rsplit_once('.')?.1.trim();
    if candidate.is_empty()
        || candidate.contains('/')
        || candidate.contains('\\')
        || candidate.contains('\0')
    {
        None
    } else {
        Some(candidate)
    }
}
