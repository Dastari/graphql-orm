use uuid::Uuid;

use crate::StorageNamespace;

/// Extension of the temporary files storage backends write before renaming.
const TEMP_EXTENSION: &str = "uploading";
/// Suffix of a temporary upload file name.
#[cfg(any(feature = "local", feature = "smb"))]
pub(crate) const TEMP_SUFFIX: &str = ".uploading";

/// Returns a fresh temporary upload file name, `.<uuid>.uploading`.
///
/// The name is independent of the target's name, so it fits wherever the
/// target fits, and a random v4 UUID keeps it collision-safe.
#[cfg(any(feature = "local", feature = "smb"))]
pub(crate) fn temp_file_name() -> String {
    format!(".{}{TEMP_SUFFIX}", Uuid::new_v4())
}

/// Whether `name` is exactly a name [`temp_file_name`] produces:
/// `.<uuid>.uploading` with the UUID in its lowercase hyphenated form.
#[cfg(any(feature = "local", feature = "smb"))]
pub(crate) fn is_temp_file_name(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(TEMP_SUFFIX))
        .is_some_and(is_canonical_uuid)
}

/// Whether `text` is a UUID in the lowercase hyphenated form `Uuid` displays.
#[cfg(any(feature = "local", feature = "smb"))]
pub(crate) fn is_canonical_uuid(text: &str) -> bool {
    Uuid::try_parse(text).is_ok_and(|uuid| uuid.hyphenated().to_string() == text)
}

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
/// extension candidates. The extension `uploading`, in any case, is also
/// rejected: storage backends name their temporary upload files with it.
#[must_use]
pub fn file_extension(file_name: &str) -> Option<&str> {
    let candidate = file_name.rsplit_once('.')?.1.trim();
    if candidate.is_empty()
        || candidate.contains('/')
        || candidate.contains('\\')
        || candidate.contains('\0')
        || candidate.eq_ignore_ascii_case(TEMP_EXTENSION)
    {
        None
    } else {
        Some(candidate)
    }
}
