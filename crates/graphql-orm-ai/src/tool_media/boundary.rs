use crate::{
    AiDeploymentEgressBoundary, AiEgressCapability, AiEgressManifest, AiEgressReason, AiError,
    AiToolResultDisclosureContext, DataClassification,
};

/// Independently admitted hard disclosure boundary for one immutable registered tool and actual
/// selected provider profile. It cannot be selected by tool arguments/output JSON/source labels.
/// Ordinary/user/unproven sources remain under the general deployment boundary; Secret is always
/// denied, and current principal/resource/grant policy is still intersected on every disclosure.
#[derive(Clone, Debug)]
pub struct AiToolResultEgressBoundary {
    tool_fingerprint: String,
    provider_profile_id: String,
    destination: String,
    boundary: AiDeploymentEgressBoundary,
}
impl AiToolResultEgressBoundary {
    /// Creates a static exact-tool/profile boundary using the existing deployment limit type.
    /// Capability selection is restricted to ToolResult, ImageAnalysis and ModelInference.
    ///
    /// # Errors
    /// Returns a configuration error for malformed fingerprint/profile, Secret permission,
    /// unsupported capability, empty/excessive limits or image count above four. Runtime build
    /// additionally requires this exact fingerprint in the registered immutable tool catalogue.
    pub fn new(
        tool_fingerprint: impl Into<String>,
        provider_profile_id: impl Into<String>,
        destination: impl Into<String>,
        boundary: AiDeploymentEgressBoundary,
    ) -> Result<Self, AiError> {
        let tool_fingerprint = tool_fingerprint.into();
        let provider_profile_id = provider_profile_id.into();
        let destination = destination.into();
        if tool_fingerprint.len() != 64
            || !tool_fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || provider_profile_id.is_empty()
            || provider_profile_id.len() > 256
            || !provider_profile_id
                .bytes()
                .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\'))
            || boundary.maximum_classification == DataClassification::Secret
            || destination.is_empty()
            || destination.len() > 512
            || !destination
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\\'))
            || boundary.maximum_bytes == 0
            || boundary.maximum_bytes > 64 * 1024 * 1024
            || boundary.maximum_attachments > 4
            || boundary.allowed_destination_trust.is_empty()
            || boundary.allowed_capabilities.is_empty()
            || boundary.allowed_capabilities.iter().any(|cap| {
                !matches!(
                    cap,
                    AiEgressCapability::ToolResult
                        | AiEgressCapability::ImageAnalysis
                        | AiEgressCapability::ModelInference
                )
            })
        {
            return Err(AiError::InvalidConfiguration(
                "invalid scoped tool-result egress boundary".into(),
            ));
        }
        Ok(Self {
            tool_fingerprint,
            provider_profile_id,
            destination,
            boundary,
        })
    }
    /// Exact immutable registered tool fingerprint admitted by host configuration.
    pub fn tool_fingerprint(&self) -> &str {
        &self.tool_fingerprint
    }
    /// Actual selected provider profile admitted by host configuration.
    pub fn provider_profile_id(&self) -> &str {
        &self.provider_profile_id
    }
    /// Exact admitted redacted destination identifier; trust-class labels alone cannot select it.
    pub fn destination(&self) -> &str {
        &self.destination
    }
}

pub(crate) fn evaluate(
    general: &AiDeploymentEgressBoundary,
    boundaries: &[AiToolResultEgressBoundary],
    manifest: &AiEgressManifest,
    contexts: &[AiToolResultDisclosureContext],
) -> Result<(), AiEgressReason> {
    if manifest.maximum_classification() == DataClassification::Secret {
        return Err(AiEgressReason::SecretDataDenied);
    }
    // Scoped sources never multiply the aggregate request bytes or authorize another destination.
    if manifest.estimated_bytes > general.maximum_bytes {
        return Err(AiEgressReason::LimitExceeded);
    }
    if !general
        .allowed_destination_trust
        .contains(&manifest.destination_trust)
    {
        return Err(AiEgressReason::DeploymentDenied);
    }
    let mut ordinary = Vec::new();
    let mut media = 0_u32;
    let mut scoped_media_ceiling = None::<u32>;
    for source in &manifest.sources {
        let matching = contexts
            .iter()
            .filter(|ctx| {
                ctx.data_source() == *source
                    || (source.classification == ctx.classification()
                        && source.trust == ctx.source_trust()
                        && ctx.images().iter().any(|image| {
                            image.matches_egress_source(&ctx.source_reference(), source)
                        }))
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(AiEgressReason::PolicyDenied);
        }
        let Some(context) = matching.first() else {
            ordinary.push(source.clone());
            continue;
        };
        if manifest.scope != *context.scope()
            || manifest.session_id != Some(context.execution_provenance().session_id())
            || manifest.run_id != Some(context.current_run_id())
        {
            return Err(AiEgressReason::PolicyDenied);
        }
        let matched = boundaries
            .iter()
            .filter(|boundary| {
                boundary.tool_fingerprint == context.execution_provenance().tool_fingerprint()
                    && boundary.provider_profile_id == manifest.provider_profile_id
                    && boundary.destination == manifest.destination
            })
            .collect::<Vec<_>>();
        let [boundary] = matched.as_slice() else {
            ordinary.push(source.clone());
            continue;
        };
        let is_image = source.kind == "tool_result_image";
        let mut exact = manifest.clone();
        exact.sources = vec![source.clone()];
        exact.attachment_count = u32::from(is_image);
        boundary.boundary.evaluate(&exact)?;
        scoped_media_ceiling = Some(
            scoped_media_ceiling.map_or(boundary.boundary.maximum_attachments, |cap| {
                cap.min(boundary.boundary.maximum_attachments)
            }),
        );
        if is_image {
            media = media.checked_add(1).ok_or(AiEgressReason::LimitExceeded)?;
        }
    }
    if media > manifest.attachment_count || media > scoped_media_ceiling.unwrap_or(0) {
        return Err(AiEgressReason::LimitExceeded);
    }
    let ordinary_attachments = manifest.attachment_count - media;
    if ordinary_attachments > general.maximum_attachments {
        return Err(AiEgressReason::LimitExceeded);
    }
    if !ordinary.is_empty() || ordinary_attachments > 0 || manifest.sources.is_empty() {
        let mut remaining = manifest.clone();
        remaining.sources = ordinary;
        remaining.attachment_count = ordinary_attachments;
        general.evaluate(&remaining)?;
    }
    // Every supplied context must actually bind this transfer; extra proof markers are refused.
    if contexts.iter().any(|context| {
        !manifest.sources.iter().any(|source| {
            context.data_source() == *source
                || (source.classification == context.classification()
                    && source.trust == context.source_trust()
                    && context.images().iter().any(|image| {
                        image.matches_egress_source(&context.source_reference(), source)
                    }))
        })
    }) {
        return Err(AiEgressReason::PolicyDenied);
    }
    Ok(())
}
