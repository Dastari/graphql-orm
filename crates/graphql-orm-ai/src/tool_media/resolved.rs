use crate::{AiError, AiToolImageReference, AiToolResultMediaLimits};
use image::{ImageDecoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, io::Cursor, sync::Arc};
use uuid::Uuid;

/// Logical owning resolver and opaque resource identity. This contains no storage locator or
/// credential and is descriptive correlation, never current resource authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiToolMediaResourceId {
    owner: String,
    reference: Uuid,
}
impl AiToolMediaResourceId {
    /// Creates one bounded logical resource identity from trusted owning metadata.
    ///
    /// # Errors
    /// Returns a safe error for a nil reference or unsafe owner identity.
    pub fn new(owner: impl Into<String>, reference: Uuid) -> Result<Self, AiError> {
        let owner = owner.into();
        if reference.is_nil()
            || owner.is_empty()
            || owner.len() > 128
            || !owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(AiError::Forbidden);
        }
        Ok(Self { owner, reference })
    }
    /// Deployment-owned logical resolver identity.
    pub fn owner(&self) -> &str {
        &self.owner
    }
    /// Exact opaque owning resource reference.
    pub const fn reference(&self) -> Uuid {
        self.reference
    }
}

/// Exact decoded/digest-verified image payload returned by the trusted owning resolver. Its
/// constructor proves byte integrity and image shape only; runtime still requires current source
/// authority, retained original bindings, active run fence, exact egress and atomic budget.
#[derive(Clone, PartialEq, Eq)]
pub struct AiResolvedToolImage {
    reference: AiToolImageReference,
    resource: AiToolMediaResourceId,
    bytes: Arc<[u8]>,
}
impl AiResolvedToolImage {
    /// Verifies exact JPEG/PNG bytes against descriptive metadata with hard dimension/allocation
    /// ceilings. Never construct from a model-selected URL/path or cache between disclosures.
    ///
    /// # Errors
    /// Returns a safe error for wrong resource, digest/MIME/dimensions, malformed encoding,
    /// unsupported color depth, or excessive bytes/decoded allocation.
    pub fn new(
        reference: AiToolImageReference,
        resource: AiToolMediaResourceId,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<Self, AiError> {
        let bytes = bytes.into();
        if resource.reference != reference.reference() {
            return Err(AiError::Forbidden);
        }
        let limits =
            AiToolResultMediaLimits::new(4, 16 * 1024 * 1024, 8192, 16_000_000, 86_400_000, 5000)?;
        decode_exact(&reference, &bytes, limits)?;
        Ok(Self {
            reference,
            resource,
            bytes,
        })
    }
    /// Original exact reference and checksum verified against these bytes.
    pub fn reference(&self) -> &AiToolImageReference {
        &self.reference
    }
    /// Logical owning resource identity to compare with immutable resolver policy.
    pub fn resource(&self) -> &AiToolMediaResourceId {
        &self.resource
    }
    /// Actual encoded byte length, independently derived from reopened payload.
    pub fn byte_count(&self) -> u64 {
        self.bytes.len() as u64
    }
    /// Sensitive verified bytes for exact authorized transport only. Do not log, retain, expose
    /// through GraphQL or carry this payload into another delivery without reopening authority.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn validate_policy(
        &self,
        resolver: &str,
        limits: AiToolResultMediaLimits,
        now_ms: i64,
    ) -> Result<(), AiError> {
        self.reference.validate_at(limits, now_ms)?;
        if self.resource.owner() != resolver || self.byte_count() > limits.maximum_raw_bytes() {
            return Err(AiError::Forbidden);
        }
        // Construction decoded these exact immutable Arc bytes; narrower bounds are applied again.
        Ok(())
    }
}
impl fmt::Debug for AiResolvedToolImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiResolvedToolImage")
            .field("reference", &self.reference)
            .field("resource", &self.resource)
            .field("byte_count", &self.byte_count())
            .field("bytes", &"[REDACTED]")
            .finish()
    }
}
fn decode_exact(
    reference: &AiToolImageReference,
    bytes: &[u8],
    limits: AiToolResultMediaLimits,
) -> Result<(), AiError> {
    if bytes.is_empty()
        || bytes.len() as u64 > limits.maximum_raw_bytes()
        || hex::encode(Sha256::digest(bytes)) != reference.sha256()
    {
        return Err(AiError::Forbidden);
    }
    let format = image::guess_format(bytes).map_err(|_| AiError::Forbidden)?;
    if !matches!(
        (format, reference.mime()),
        (ImageFormat::Jpeg, "image/jpeg") | (ImageFormat::Png, "image/png")
    ) {
        return Err(AiError::Forbidden);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut bounds = image::Limits::default();
    bounds.max_image_width = Some(limits.maximum_edge());
    bounds.max_image_height = Some(limits.maximum_edge());
    bounds.max_alloc = Some(
        limits
            .maximum_pixels()
            .checked_mul(4)
            .ok_or(AiError::Forbidden)?,
    );
    reader.limits(bounds);
    let decoder = reader.into_decoder().map_err(|_| AiError::Forbidden)?;
    let (width, height) = decoder.dimensions();
    if (width, height) != (reference.width(), reference.height())
        || width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > limits.maximum_pixels()
        || decoder.color_type().bits_per_pixel() > 32
    {
        return Err(AiError::Forbidden);
    }
    image::DynamicImage::from_decoder(decoder).map_err(|_| AiError::Forbidden)?;
    Ok(())
}
