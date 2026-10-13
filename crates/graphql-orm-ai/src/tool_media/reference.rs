use crate::{AiError, AiToolImageFieldBindings, AiToolResultMediaLimits};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Exact descriptive metadata for one opaque image in a retained tool result. Deserialization
/// does not prove source authority, ownership, grant validity, byte custody or image decoding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiToolImageReference {
    reference: Uuid,
    sha256: String,
    mime: String,
    width: u32,
    height: u32,
    captured_at_ms: i64,
    retained_until_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_expires_at_ms: Option<i64>,
}
impl AiToolImageReference {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn source_prefix(&self, original: &str) -> String {
        format!(
            "v1:{original}:{}:{}:{}:{}:{}:{}:{}:{}:",
            self.reference,
            self.mime,
            self.sha256,
            self.width,
            self.height,
            self.captured_at_ms,
            self.retained_until_ms,
            self.action_expires_at_ms.unwrap_or(0)
        )
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn egress_source(
        &self,
        context: &crate::AiToolResultDisclosureContext,
        bytes: u64,
    ) -> crate::AiDataSourceRef {
        crate::AiDataSourceRef {
            kind: "tool_result_image".into(),
            reference: format!("{}{bytes}", self.source_prefix(&context.source_reference())),
            classification: context.classification(),
            trust: context.source_trust(),
        }
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn matches_egress_source(
        &self,
        original: &str,
        source: &crate::AiDataSourceRef,
    ) -> bool {
        source.kind == "tool_result_image"
            && source
                .reference
                .strip_prefix(&self.source_prefix(original))
                .and_then(|count| count.parse::<u64>().ok())
                .is_some_and(|count| count > 0 && count <= 16 * 1024 * 1024)
    }
    /// Original opaque observation/resource identifier, never a key, URL or path.
    pub const fn reference(&self) -> Uuid {
        self.reference
    }
    /// SHA-256 of the original image bytes.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    /// Original detected MIME hint, independently checked by actual decoding.
    pub fn mime(&self) -> &str {
        &self.mime
    }
    /// Original decoded pixel width, independently checked before transport.
    pub const fn width(&self) -> u32 {
        self.width
    }
    /// Original decoded pixel height, independently checked before transport.
    pub const fn height(&self) -> u32 {
        self.height
    }
    /// Original capture time in Unix milliseconds.
    pub const fn captured_at_ms(&self) -> i64 {
        self.captured_at_ms
    }
    /// Original retained-content deadline in Unix milliseconds; no reopen renews it.
    pub const fn retained_until_ms(&self) -> i64 {
        self.retained_until_ms
    }
    /// Optional original action deadline. This historical label grants no execution or media access.
    pub const fn action_expires_at_ms(&self) -> Option<i64> {
        self.action_expires_at_ms
    }
    /// Whether the original action label has elapsed at the supplied trusted Unix milliseconds.
    /// This is descriptive and must never replace current owning execution authority.
    pub fn action_label_is_current(&self, now_ms: i64) -> bool {
        self.captured_at_ms <= now_ms
            && self
                .action_expires_at_ms
                .is_some_and(|expires| now_ms < expires)
    }
    pub(crate) fn from_output(
        value: &serde_json::Value,
        fields: &AiToolImageFieldBindings,
        limits: AiToolResultMediaLimits,
        now_ms: i64,
    ) -> Result<Option<Self>, AiError> {
        let value = value.as_object().ok_or(AiError::Forbidden)?;
        let hint = value.get(fields.reference()).ok_or(AiError::Forbidden)?;
        if hint.is_null() {
            if [
                fields.sha256(),
                fields.mime(),
                fields.width(),
                fields.height(),
            ]
            .iter()
            .any(|field| value.get(*field).is_some_and(|value| !value.is_null()))
            {
                return Err(AiError::Forbidden);
            }
            return Ok(None);
        }
        let reference = Uuid::parse_str(hint.as_str().ok_or(AiError::Forbidden)?)
            .map_err(|_| AiError::Forbidden)?;
        let string = |field: &str| {
            value
                .get(field)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .ok_or(AiError::Forbidden)
        };
        let number = |field: &str| {
            value
                .get(field)
                .and_then(|value| value.as_i64())
                .ok_or(AiError::Forbidden)
        };
        let image = Self {
            reference,
            sha256: string(fields.sha256())?,
            mime: string(fields.mime())?,
            width: u32::try_from(number(fields.width())?).map_err(|_| AiError::Forbidden)?,
            height: u32::try_from(number(fields.height())?).map_err(|_| AiError::Forbidden)?,
            captured_at_ms: number(fields.captured_at_ms())?,
            retained_until_ms: number(fields.retained_until_ms())?,
            action_expires_at_ms: fields.action_expires_at_ms().map(number).transpose()?,
        };
        image.validate_at(limits, now_ms)?;
        Ok(Some(image))
    }
    pub(crate) fn validate_at(
        &self,
        limits: AiToolResultMediaLimits,
        now_ms: i64,
    ) -> Result<(), AiError> {
        let age = self
            .retained_until_ms
            .checked_sub(self.captured_at_ms)
            .ok_or(AiError::Forbidden)?;
        if self.reference.is_nil()
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(self.mime.as_str(), "image/jpeg" | "image/png")
            || self.width == 0
            || self.height == 0
            || self.width > limits.maximum_edge()
            || self.height > limits.maximum_edge()
            || u64::from(self.width) * u64::from(self.height) > limits.maximum_pixels()
            || self.captured_at_ms < 0
            || self.captured_at_ms > now_ms
            || now_ms >= self.retained_until_ms
            || age <= 0
            || u64::try_from(age).map_or(true, |age| age > limits.maximum_retention_ms())
            || self.action_expires_at_ms.is_some_and(|action| {
                action <= self.captured_at_ms || action > self.retained_until_ms
            })
        {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }
}

/// Applies the immutable complete-result aggregate count and decoded-pixel ceiling. Reference
/// shape remains descriptive; actual codecs/digests/resources are independently verified.
pub(crate) fn validate_image_reference_batch(
    images: &[AiToolImageReference],
    limits: AiToolResultMediaLimits,
) -> Result<(), AiError> {
    if images.len() > usize::from(limits.maximum_images()) {
        return Err(AiError::ResultBudgetExceeded);
    }
    let pixels = images
        .iter()
        .try_fold(0_u64, |sum, image| {
            sum.checked_add(u64::from(image.width()).checked_mul(u64::from(image.height()))?)
        })
        .ok_or(AiError::ResultBudgetExceeded)?;
    if pixels > limits.maximum_pixels() {
        return Err(AiError::ResultBudgetExceeded);
    }
    Ok(())
}
