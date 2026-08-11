//! Bounded, ordered provider activity suitable for protected durable replay.

use std::collections::BTreeMap;
use std::time::Instant;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use async_trait::async_trait;

use crate::{
    AiError, AiLiveDeltaBatch, AiLiveDeltaCoalescer, AiLiveDeltaCoalescerLimits, AiLiveDeltaKind,
    ProviderCitation, ProviderEvent,
};

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::{AiLiveDeltaPersistenceContext, AiRunLease};

/// One ordered, browser-presentable provider activity payload.
///
/// The enum deliberately excludes application-tool arguments/results, raw
/// provider frames, hidden reasoning, errors, credentials, and built-in tool
/// result bodies. Application-tool execution has its own authoritative durable
/// lifecycle events.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiProviderActivityPayload {
    /// Bounded visible assistant text.
    Text {
        /// UTF-8 text batch.
        text: String,
    },
    /// Bounded provider-generated visible reasoning summary.
    ReasoningSummary {
        /// UTF-8 summary batch; never hidden chain-of-thought.
        text: String,
    },
    /// A provider-hosted tool was accepted for execution.
    HostedToolStarted {
        /// Exact provider call reference.
        call_id: String,
        /// Closed normalized built-in kind.
        kind: String,
    },
    /// A previously started provider-hosted tool completed.
    HostedToolCompleted {
        /// Exact provider call reference.
        call_id: String,
        /// Closed normalized built-in kind.
        kind: String,
    },
    /// Provider-authored, validated citation metadata.
    Citation {
        /// Exact normalized citation and provider output span.
        citation: ProviderCitation,
    },
}

impl AiProviderActivityPayload {
    /// Stable content-free activity kind.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::ReasoningSummary { .. } => "reasoning_summary",
            Self::HostedToolStarted { .. } => "hosted_tool_started",
            Self::HostedToolCompleted { .. } => "hosted_tool_completed",
            Self::Citation { .. } => "citation",
        }
    }
}

/// One monotonic activity item within an exact provider turn.
///
/// The sequence is provisional turn-local ordering. The durable session stream
/// allocates the authoritative cross-turn cursor when a protected sink commits
/// the activity.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "provider activity must be persisted/delivered or explicitly discarded"]
pub struct AiProviderActivity {
    sequence: u64,
    payload: AiProviderActivityPayload,
}

impl AiProviderActivity {
    /// Monotonic activity order within this provider turn.
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Typed bounded payload.
    pub const fn payload(&self) -> &AiProviderActivityPayload {
        &self.payload
    }
}

/// Protected durable persistence boundary for one ordered provider activity.
///
/// Implementations must validate the current run fence and principal, protect
/// content, and commit to the authoritative session stream before returning.
/// They must never widen a built-in or application-tool capability.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[async_trait]
pub trait AiProviderActivitySink: Send + Sync {
    /// Persists one exact ordered activity.
    ///
    /// # Errors
    ///
    /// Returns a safe error for stale fencing, current access/protection
    /// denial, malformed metadata, or persistence failure. Failure happens
    /// after transport began and must not trigger automatic provider replay.
    async fn persist_activity(
        &self,
        lease: &AiRunLease,
        context: &AiLiveDeltaPersistenceContext,
        activity: &AiProviderActivity,
    ) -> Result<(), AiError>;
}

/// Ordered coalescing state for one exact provider turn.
///
/// Visible deltas retain the existing 50 ms / 4 KiB batching contract.
/// Pending visible content is flushed before every structured provider event,
/// so hosted-tool and citation activity cannot overtake text or summaries.
pub struct AiProviderActivityCoalescer {
    visible: AiLiveDeltaCoalescer,
    next_sequence: u64,
    hosted_calls: BTreeMap<String, HostedCallState>,
}

struct HostedCallState {
    kind: String,
    completed: bool,
}

impl AiProviderActivityCoalescer {
    /// Creates empty ordered activity state.
    pub const fn new(limits: AiLiveDeltaCoalescerLimits) -> Self {
        Self {
            visible: AiLiveDeltaCoalescer::new(limits),
            next_sequence: 0,
            hosted_calls: BTreeMap::new(),
        }
    }

    /// Accepts one already-normalized provider event.
    ///
    /// Text and visible summaries are coalesced. Hosted-tool lifecycle and
    /// citation metadata are emitted immediately after flushing preceding
    /// visible content. Other structured events act as ordering barriers but
    /// do not create browser activity.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::ProviderFailed`] for malformed or duplicate hosted
    /// lifecycle, invalid citations, or sequence overflow.
    pub fn push_event(
        &mut self,
        event: &ProviderEvent,
        now: Instant,
    ) -> Result<Vec<AiProviderActivity>, AiError> {
        if matches!(
            event,
            ProviderEvent::TextDelta { .. } | ProviderEvent::ReasoningSummaryDelta { .. }
        ) {
            let visible = self.visible.push_event(event, now)?;
            return self.map_visible(visible);
        }

        let visible = self.visible.flush_all()?;
        let mut activities = self.map_visible(visible)?;
        let payload = match event {
            ProviderEvent::BuiltinToolStarted { call_id, kind } => {
                if !valid_reference(call_id)
                    || !valid_hosted_kind(kind)
                    || self.hosted_calls.len() >= 4_096
                    || self.hosted_calls.contains_key(call_id)
                {
                    return Err(AiError::ProviderFailed);
                }
                self.hosted_calls.insert(
                    call_id.clone(),
                    HostedCallState {
                        kind: kind.clone(),
                        completed: false,
                    },
                );
                Some(AiProviderActivityPayload::HostedToolStarted {
                    call_id: call_id.clone(),
                    kind: kind.clone(),
                })
            }
            ProviderEvent::BuiltinToolCompleted { call_id, .. } => {
                let Some(call) = self.hosted_calls.get_mut(call_id) else {
                    return Err(AiError::ProviderFailed);
                };
                if call.completed {
                    return Err(AiError::ProviderFailed);
                }
                call.completed = true;
                Some(AiProviderActivityPayload::HostedToolCompleted {
                    call_id: call_id.clone(),
                    kind: call.kind.clone(),
                })
            }
            ProviderEvent::Citation { citation } => {
                citation.validate().map_err(|_| AiError::ProviderFailed)?;
                Some(AiProviderActivityPayload::Citation {
                    citation: citation.clone(),
                })
            }
            ProviderEvent::ResponseStarted { .. }
            | ProviderEvent::ToolCallStarted { .. }
            | ProviderEvent::ToolArgumentsDelta { .. }
            | ProviderEvent::ToolCallCompleted { .. }
            | ProviderEvent::Usage { .. }
            | ProviderEvent::ResponseCompleted { .. }
            | ProviderEvent::Unknown { .. } => None,
            ProviderEvent::TextDelta { .. } | ProviderEvent::ReasoningSummaryDelta { .. } => {
                unreachable!("visible events returned above")
            }
        };
        if let Some(payload) = payload {
            activities.push(self.activity(payload)?);
        }
        Ok(activities)
    }

    /// Flushes visible activity whose delay has elapsed.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::ProviderFailed`] on sequence overflow.
    pub fn flush_due(&mut self, now: Instant) -> Result<Vec<AiProviderActivity>, AiError> {
        let visible = self.visible.flush_due(now)?;
        self.map_visible(visible)
    }

    /// Flushes all pending visible activity.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::ProviderFailed`] on sequence overflow.
    pub fn flush_all(&mut self) -> Result<Vec<AiProviderActivity>, AiError> {
        let visible = self.visible.flush_all()?;
        self.map_visible(visible)
    }

    fn map_visible(
        &mut self,
        batches: Vec<AiLiveDeltaBatch>,
    ) -> Result<Vec<AiProviderActivity>, AiError> {
        batches
            .into_iter()
            .map(|batch| {
                let payload = match batch.kind() {
                    AiLiveDeltaKind::Text => AiProviderActivityPayload::Text {
                        text: batch.text().to_owned(),
                    },
                    AiLiveDeltaKind::ReasoningSummary => {
                        AiProviderActivityPayload::ReasoningSummary {
                            text: batch.text().to_owned(),
                        }
                    }
                };
                self.activity(payload)
            })
            .collect()
    }

    fn activity(
        &mut self,
        payload: AiProviderActivityPayload,
    ) -> Result<AiProviderActivity, AiError> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(AiError::ProviderFailed)?;
        Ok(AiProviderActivity { sequence, payload })
    }
}

fn valid_reference(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 1_024
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

fn valid_hosted_kind(value: &str) -> bool {
    matches!(
        value,
        "web_search" | "file_search" | "code_interpreter" | "image_generation"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn citation() -> ProviderCitation {
        ProviderCitation::new(
            "https://example.test/source".to_owned(),
            Some("Example".to_owned()),
            "output-1".to_owned(),
            0,
            0,
            2,
            8,
        )
        .expect("test citation should validate")
    }

    #[test]
    fn citation_rejects_wildcard_authorities() {
        assert!(
            ProviderCitation::new(
                "https://*.example.test/source".to_owned(),
                None,
                "output-1".to_owned(),
                0,
                0,
                0,
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn structured_activity_flushes_visible_content_in_provider_order() {
        let now = Instant::now();
        let mut coalescer = AiProviderActivityCoalescer::new(AiLiveDeltaCoalescerLimits::default());
        assert!(
            coalescer
                .push_event(
                    &ProviderEvent::ReasoningSummaryDelta {
                        text: "checking".to_owned(),
                    },
                    now,
                )
                .expect("summary should buffer")
                .is_empty()
        );
        let activities = coalescer
            .push_event(
                &ProviderEvent::BuiltinToolStarted {
                    call_id: "search-1".to_owned(),
                    kind: "web_search".to_owned(),
                },
                now,
            )
            .expect("hosted start should validate");
        assert_eq!(activities.len(), 2);
        assert!(matches!(
            activities[0].payload(),
            AiProviderActivityPayload::ReasoningSummary { text } if text == "checking"
        ));
        assert!(matches!(
            activities[1].payload(),
            AiProviderActivityPayload::HostedToolStarted { call_id, kind }
                if call_id == "search-1" && kind == "web_search"
        ));
        assert_eq!(activities[0].sequence(), 0);
        assert_eq!(activities[1].sequence(), 1);
    }

    #[test]
    fn citation_and_completion_retain_order_without_result_body() {
        let now = Instant::now();
        let mut coalescer = AiProviderActivityCoalescer::new(AiLiveDeltaCoalescerLimits::default());
        let started = coalescer
            .push_event(
                &ProviderEvent::BuiltinToolStarted {
                    call_id: "search-1".to_owned(),
                    kind: "web_search".to_owned(),
                },
                now,
            )
            .expect("start should validate");
        let cited = coalescer
            .push_event(
                &ProviderEvent::Citation {
                    citation: citation(),
                },
                now,
            )
            .expect("citation should validate");
        let completed = coalescer
            .push_event(
                &ProviderEvent::BuiltinToolCompleted {
                    call_id: "search-1".to_owned(),
                    result: serde_json::json!({"secret": "must-not-enter-activity"}),
                },
                now,
            )
            .expect("completion should validate");

        assert_eq!(started[0].sequence(), 0);
        assert_eq!(cited[0].sequence(), 1);
        assert_eq!(completed[0].sequence(), 2);
        assert!(matches!(
            completed[0].payload(),
            AiProviderActivityPayload::HostedToolCompleted { kind, .. }
                if kind == "web_search"
        ));
    }

    #[test]
    fn duplicate_or_unpaired_hosted_lifecycle_fails_closed() {
        let now = Instant::now();
        let mut coalescer = AiProviderActivityCoalescer::new(AiLiveDeltaCoalescerLimits::default());
        assert!(
            coalescer
                .push_event(
                    &ProviderEvent::BuiltinToolCompleted {
                        call_id: "missing".to_owned(),
                        result: serde_json::Value::Null,
                    },
                    now,
                )
                .is_err()
        );
        coalescer
            .push_event(
                &ProviderEvent::BuiltinToolStarted {
                    call_id: "search-1".to_owned(),
                    kind: "web_search".to_owned(),
                },
                now,
            )
            .expect("start should validate");
        assert!(
            coalescer
                .push_event(
                    &ProviderEvent::BuiltinToolStarted {
                        call_id: "search-1".to_owned(),
                        kind: "web_search".to_owned(),
                    },
                    now,
                )
                .is_err()
        );
    }
}
