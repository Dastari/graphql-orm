//! Private source revalidation minted only after owning result/body admission.
use super::*;

pub(crate) struct ToolSourceDelivery {
    service: Arc<crate::OrmAiApplicationToolCallService>,
    lease: Arc<Mutex<AiRunLease>>,
    manifests: Vec<AiEgressManifest>,
    maximum_image_units: u64,
    task_context: Option<Arc<crate::AiPreparedTaskContinuationContext>>,
}
impl std::fmt::Debug for ToolSourceDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ToolSourceDelivery(<fenced owning source>)")
    }
}
impl ToolSourceDelivery {
    pub(super) fn new(
        service: Arc<crate::OrmAiApplicationToolCallService>,
        lease: Arc<Mutex<AiRunLease>>,
        manifests: &[AiEgressManifest],
        maximum_image_units: u64,
        task_context: Option<Arc<crate::AiPreparedTaskContinuationContext>>,
    ) -> Result<Option<Arc<Self>>, AiError> {
        let manifests = manifests
            .iter()
            .filter(|manifest| {
                (task_context.is_some()
                    && manifest.capability == AiEgressCapability::ModelInference)
                    || manifest.sources.iter().any(|source| {
                        source.kind == "application_tool_result"
                            && source.reference.starts_with("v2:")
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let references = manifests
            .iter()
            .flat_map(|manifest| manifest.sources.iter())
            .filter(|source| {
                source.kind == "application_tool_result" && source.reference.starts_with("v2:")
            })
            .map(|source| source.reference.as_str())
            .collect::<BTreeSet<_>>();
        if references.len() > 64 {
            return Err(AiError::ResultBudgetExceeded);
        }
        if manifests.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Arc::new(Self {
                service,
                lease,
                manifests,
                maximum_image_units,
                task_context,
            })))
        }
    }
    pub(crate) async fn authorize(&self) -> Result<(), ProviderError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let operation = async {
            let lease = self.lease.lock().await;
            self.service
                .validate_delivery_fence(&lease)
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            if let Some(context) = &self.task_context {
                let inference = self
                    .manifests
                    .iter()
                    .find(|manifest| manifest.capability == AiEgressCapability::ModelInference)
                    .ok_or(ProviderError::Rejected)?;
                self.service
                    .validate_task_continuation_context(&lease, &inference.scope, context)
                    .await
                    .map_err(|_| ProviderError::ToolSourceUnavailable)?;
            }
            let mut seen = BTreeSet::new();
            let mut count = 0_u64;
            let mut bytes = 0_u64;
            let mut pixels = 0_u64;
            for manifest in &self.manifests {
                let mut contexts = Vec::new();
                for source in &manifest.sources {
                    if source.kind != "application_tool_result"
                        || !source.reference.starts_with("v2:")
                    {
                        continue;
                    }
                    let id = crate::tool_media::source_call_hint(&source.reference)
                        .ok_or(ProviderError::ToolSourceUnavailable)?;
                    let context = self
                        .service
                        .reopen_tool_result_source(
                            &lease,
                            crate::AiToolCallId(id),
                            crate::AiToolResultDisclosurePurpose::History,
                        )
                        .await
                        .map_err(|_| ProviderError::ToolSourceUnavailable)?;
                    if context.data_source() != *source {
                        return Err(ProviderError::ToolSourceUnavailable);
                    }
                    contexts.push(context);
                }
                let ordinary = crate::tool_media::ordinary_source_component(manifest, &contexts)
                    .map_err(|_| ProviderError::EgressDenied)?;
                self.service
                    .runtime()
                    .authorize_egress(lease.principal_reference(), &ordinary)
                    .await
                    .map_err(|_| ProviderError::EgressDenied)?
                    .authorize(&ordinary)
                    .map_err(|_| ProviderError::EgressDenied)?;
                for context in &contexts {
                    if seen.insert(context.source_reference()) && !context.images().is_empty() {
                        let images = self
                            .service
                            .authorize_retained_image_analysis(&lease, context, manifest)
                            .await
                            .map_err(|_| ProviderError::ToolSourceUnavailable)?;
                        count = count
                            .checked_add(images.len() as u64)
                            .ok_or(ProviderError::BudgetDenied)?;
                        bytes = images
                            .iter()
                            .try_fold(bytes, |sum, image| sum.checked_add(image.byte_count()))
                            .ok_or(ProviderError::BudgetDenied)?;
                        pixels = images
                            .iter()
                            .try_fold(pixels, |sum, image| {
                                let reference = image.reference();
                                sum.checked_add(
                                    u64::from(reference.width()) * u64::from(reference.height()),
                                )
                            })
                            .ok_or(ProviderError::BudgetDenied)?;
                        if count > 4
                            || count > self.maximum_image_units
                            || bytes > 16 * 1024 * 1024
                            || pixels > 16_000_000
                        {
                            return Err(ProviderError::BudgetDenied);
                        }
                    }
                }
                let decision = self
                    .service
                    .runtime()
                    .authorize_tool_source_egress(lease.principal_reference(), manifest, &contexts)
                    .await
                    .map_err(|_| ProviderError::ToolSourceUnavailable)?;
                self.service
                    .record_current_source_decision(manifest, &decision)
                    .await
                    .map_err(|_| ProviderError::ToolSourceUnavailable)?;
                if decision.authorize(manifest).is_err() {
                    let ordinary =
                        crate::tool_media::ordinary_source_component(manifest, &contexts)
                            .map_err(|_| ProviderError::EgressDenied)?;
                    self.service
                        .runtime()
                        .authorize_egress(lease.principal_reference(), &ordinary)
                        .await
                        .map_err(|_| ProviderError::EgressDenied)?
                        .authorize(&ordinary)
                        .map_err(|_| ProviderError::EgressDenied)?;
                    return Err(ProviderError::ToolSourceUnavailable);
                }
                if std::time::Instant::now() >= deadline {
                    return Err(ProviderError::ToolSourceUnavailable);
                }
            }
            self.service
                .validate_delivery_fence(&lease)
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            Ok(())
        };
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            Box::pin(operation),
        )
        .await
        .map_err(|_| ProviderError::ToolSourceUnavailable)?
    }
}

impl PartialEq for ToolSourceDelivery {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.service, &other.service)
            && Arc::ptr_eq(&self.lease, &other.lease)
            && self.manifests == other.manifests
            && self.maximum_image_units == other.maximum_image_units
    }
}
