//! Immutable output-image references in reviewed tool projections. These types grant no access.

use crate::{AiDisclosureDisposition, AiDisclosureShape, AiError, DataClassification};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Closed field mappings in one selected output object. Values are descriptive hints only;
/// the trusted owning resolver must independently match the original protected capture record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiToolImageFieldBindings {
    reference: String,
    sha256: String,
    mime: String,
    width: String,
    height: String,
    captured_at_ms: String,
    retained_until_ms: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_expires_at_ms: Option<String>,
}

impl AiToolImageFieldBindings {
    /// Binds exact selected scalar field names, with timestamps expressed in Unix milliseconds.
    ///
    /// # Errors
    /// Returns a configuration error for unknown, duplicate or unsafe field names.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reference: impl Into<String>,
        sha256: impl Into<String>,
        mime: impl Into<String>,
        width: impl Into<String>,
        height: impl Into<String>,
        captured_at_ms: impl Into<String>,
        retained_until_ms: impl Into<String>,
    ) -> Result<Self, AiError> {
        let fields = Self {
            reference: reference.into(),
            sha256: sha256.into(),
            mime: mime.into(),
            width: width.into(),
            height: height.into(),
            captured_at_ms: captured_at_ms.into(),
            retained_until_ms: retained_until_ms.into(),
            action_expires_at_ms: None,
        };
        fields.validate()?;
        Ok(fields)
    }
    /// Adds the original action deadline as a historical label. It never grants image access,
    /// renews action freshness or replaces the independently retained image deadline.
    ///
    /// # Errors
    /// Returns a configuration error for an unsafe or duplicate field name.
    pub fn with_action_expires_at_ms(mut self, field: impl Into<String>) -> Result<Self, AiError> {
        self.action_expires_at_ms = Some(field.into());
        self.validate()?;
        Ok(self)
    }
    /// Opaque image reference field.
    pub fn reference(&self) -> &str {
        &self.reference
    }
    /// Exact content digest field.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    /// Detected image MIME field.
    pub fn mime(&self) -> &str {
        &self.mime
    }
    /// Pixel width field.
    pub fn width(&self) -> &str {
        &self.width
    }
    /// Pixel height field.
    pub fn height(&self) -> &str {
        &self.height
    }
    /// Original capture timestamp field, in Unix milliseconds.
    pub fn captured_at_ms(&self) -> &str {
        &self.captured_at_ms
    }
    /// Original retained-content deadline field, in Unix milliseconds.
    pub fn retained_until_ms(&self) -> &str {
        &self.retained_until_ms
    }
    /// Optional original action deadline field, in Unix milliseconds.
    pub fn action_expires_at_ms(&self) -> Option<&str> {
        self.action_expires_at_ms.as_deref()
    }
    fn names(&self) -> Vec<&str> {
        let mut fields = vec![
            self.reference(),
            self.sha256(),
            self.mime(),
            self.width(),
            self.height(),
            self.captured_at_ms(),
            self.retained_until_ms(),
        ];
        fields.extend(self.action_expires_at_ms());
        fields
    }
    fn validate(&self) -> Result<(), AiError> {
        let fields = self.names();
        if fields.iter().any(|field| !valid_name(field))
            || fields.iter().copied().collect::<BTreeSet<_>>().len() != fields.len()
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Independently bounded media limits. A host can narrow these ceilings but not enlarge them
/// through tool arguments, output JSON or an owning resolver's response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiToolResultMediaLimits {
    maximum_images: u32,
    maximum_raw_bytes: u64,
    maximum_edge: u32,
    maximum_pixels: u64,
    maximum_retention_ms: u64,
    maximum_resolution_ms: u64,
}
impl AiToolResultMediaLimits {
    /// Creates limits for one complete result: at most four images, 16 MiB total encoded bytes,
    /// 8192 pixels per edge, 16 million total decoded pixels, one-day retention and five-second I/O.
    ///
    /// # Errors
    /// Returns a configuration error for zero or excessive ceilings.
    pub fn new(
        maximum_images: u32,
        maximum_raw_bytes: u64,
        maximum_edge: u32,
        maximum_pixels: u64,
        maximum_retention_ms: u64,
        maximum_resolution_ms: u64,
    ) -> Result<Self, AiError> {
        let limits = Self {
            maximum_images,
            maximum_raw_bytes,
            maximum_edge,
            maximum_pixels,
            maximum_retention_ms,
            maximum_resolution_ms,
        };
        limits.validate()?;
        Ok(limits)
    }
    /// Maximum image count in one complete output.
    pub const fn maximum_images(self) -> u32 {
        self.maximum_images
    }
    /// Maximum aggregate encoded image bytes before provider expansion.
    pub const fn maximum_raw_bytes(self) -> u64 {
        self.maximum_raw_bytes
    }
    /// Maximum decoded width or height.
    pub const fn maximum_edge(self) -> u32 {
        self.maximum_edge
    }
    /// Maximum aggregate decoded pixels.
    pub const fn maximum_pixels(self) -> u64 {
        self.maximum_pixels
    }
    /// Maximum original capture-to-retention interval in milliseconds.
    pub const fn maximum_retention_ms(self) -> u64 {
        self.maximum_retention_ms
    }
    /// Whole resolution/decode/current-policy deadline in milliseconds.
    pub const fn maximum_resolution_ms(self) -> u64 {
        self.maximum_resolution_ms
    }
    fn validate(self) -> Result<(), AiError> {
        if !(1..=4).contains(&self.maximum_images)
            || !(1..=16 * 1024 * 1024).contains(&self.maximum_raw_bytes)
            || !(1..=8192).contains(&self.maximum_edge)
            || !(1..=16_000_000).contains(&self.maximum_pixels)
            || !(1..=86_400_000).contains(&self.maximum_retention_ms)
            || !(1..=5_000).contains(&self.maximum_resolution_ms)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Static output-media policy fingerprinted with its exact registered tool. Paths contain only
/// selected object fields; no wildcard, URL, filesystem path, storage key or resolver argument.
/// Registration discovers this contract and never authorizes media or provider disclosure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiToolResultMediaPolicy {
    resolver_id: String,
    object_paths: Vec<Vec<String>>,
    fields: AiToolImageFieldBindings,
    limits: AiToolResultMediaLimits,
    classification: DataClassification,
}
impl AiToolResultMediaPolicy {
    /// Binds a deployment-owned logical resolver identity and exact selected output objects.
    ///
    /// # Errors
    /// Returns a configuration error for unsafe identity/path, duplicate paths, excessive limits
    /// or Secret classification. Every path must later match the finished disclosure projection.
    pub fn new(
        resolver_id: impl Into<String>,
        object_paths: Vec<Vec<String>>,
        fields: AiToolImageFieldBindings,
        limits: AiToolResultMediaLimits,
        classification: DataClassification,
    ) -> Result<Self, AiError> {
        let policy = Self {
            resolver_id: resolver_id.into(),
            object_paths,
            fields,
            limits,
            classification,
        };
        policy.validate()?;
        Ok(policy)
    }
    /// Fixed host resolver identity; never a request-selected URL or storage locator.
    pub fn resolver_id(&self) -> &str {
        &self.resolver_id
    }
    /// Exact field paths from the disclosure root to selected image metadata objects.
    pub fn object_paths(&self) -> &[Vec<String>] {
        &self.object_paths
    }
    /// Immutable selected field mappings.
    pub fn fields(&self) -> &AiToolImageFieldBindings {
        &self.fields
    }
    /// Owning immutable complete-result limits.
    pub const fn limits(&self) -> AiToolResultMediaLimits {
        self.limits
    }
    /// Static minimum confidentiality of resolved bytes; runtime facts may only tighten it.
    pub const fn classification(&self) -> DataClassification {
        self.classification
    }
    /// Validates static shape and hard ceilings, including deserialized metadata.
    ///
    /// # Errors
    /// Returns a configuration error for malformed or excessive policy.
    pub fn validate(&self) -> Result<(), AiError> {
        self.fields.validate()?;
        self.limits.validate()?;
        if self.resolver_id.is_empty()
            || self.resolver_id.len() > 128
            || !self
                .resolver_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            || self.object_paths.is_empty()
            || self.object_paths.len() > self.limits.maximum_images as usize
            || self.object_paths.iter().collect::<BTreeSet<_>>().len() != self.object_paths.len()
            || self
                .object_paths
                .iter()
                .any(|path| path.len() > 8 || path.iter().any(|name| !valid_name(name)))
            || self.classification == DataClassification::Secret
        {
            return Err(invalid());
        }
        Ok(())
    }
    /// Requires every metadata field to be an exportable selected scalar beneath exact static
    /// object paths. This prevents runtime JSON from inventing a reference projection.
    ///
    /// # Errors
    /// Returns a configuration error for missing paths/fields, lists, non-scalar metadata or a
    /// NeverExport/Secret ancestor. Ordinary current source policy is still mandatory.
    pub fn validate_disclosure(&self, root: &AiDisclosureShape) -> Result<(), AiError> {
        self.validate()?;
        for path in &self.object_paths {
            let mut node = root;
            for name in path {
                let AiDisclosureShape::Object { rule, fields } = node else {
                    return Err(invalid());
                };
                require_exportable(rule.classification, rule.disposition)?;
                node = fields.get(name).ok_or_else(invalid)?;
            }
            let AiDisclosureShape::Object { rule, fields } = node else {
                return Err(invalid());
            };
            require_exportable(rule.classification, rule.disposition)?;
            for name in self.fields.names() {
                let Some(AiDisclosureShape::Scalar { rule }) = fields.get(name) else {
                    return Err(invalid());
                };
                require_exportable(rule.classification, rule.disposition)?;
            }
        }
        Ok(())
    }
}
fn require_exportable(
    classification: DataClassification,
    disposition: AiDisclosureDisposition,
) -> Result<(), AiError> {
    if classification == DataClassification::Secret
        || disposition != AiDisclosureDisposition::Exportable
    {
        return Err(invalid());
    }
    Ok(())
}
fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    !name.starts_with("__")
        && name.len() <= 128
        && matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| matches!(byte,b'A'..=b'Z'|b'a'..=b'z'|b'0'..=b'9'|b'_'))
}
fn invalid() -> AiError {
    AiError::InvalidConfiguration("tool result media policy is invalid".to_owned())
}

#[cfg(test)]
mod tests;
