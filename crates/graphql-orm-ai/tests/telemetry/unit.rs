use std::time::Duration;

use super::{
    AiProviderCallTelemetry, AiRestorePlanTelemetry, AiRetentionTelemetry, AiTelemetryOperationId,
    AiTelemetryOutcome,
};
use crate::{
    AiRestoreIssue, AiRestoreIssueSeverity, AiRestorePlan, AiRestoredRunAction,
    AiRestoredRunDisposition, AiRunId, AiSessionRetentionReport, ProviderKind,
};

#[test]
fn provider_observations_follow_content_free_otel_mapping() {
    let operation_id = AiTelemetryOperationId::new();
    let started = AiProviderCallTelemetry::started(operation_id, ProviderKind::OpenAi);
    assert_eq!(started.otel_operation_name(), "chat");
    assert_eq!(started.otel_provider_name(), Some("openai"));
    assert_eq!(started.outcome(), None);
    assert!(!format!("{started:?}").contains(&operation_id.as_uuid().to_string()));

    let finished = AiProviderCallTelemetry::finished(
        operation_id,
        ProviderKind::OpenAiCompatible,
        AiTelemetryOutcome::Succeeded,
        Duration::from_millis(125),
        20,
        5,
        4,
    )
    .expect("valid successful observation");
    assert_eq!(finished.otel_provider_name(), None);
    assert_eq!(finished.cached_input_tokens(), 4);
    assert!(
        AiProviderCallTelemetry::finished(
            operation_id,
            ProviderKind::OpenAi,
            AiTelemetryOutcome::Uncertain,
            Duration::ZERO,
            1,
            0,
            0,
        )
        .is_err()
    );
}

#[test]
fn retention_projection_omits_cursor_and_preserves_categories() {
    let report = AiSessionRetentionReport {
        sessions_scanned: 5,
        sessions_changed: 2,
        deleting_session_inbox_payloads_purged: 3,
        message_blocks_deleted: 4,
        message_contents_purged: 1,
        deleting_session_attachment_cleanups_requested: 2,
        messages_blocked: 6,
        sessions_conflicted: 1,
        deleting_sessions_finalized: 1,
        next_session_cursor: Some("sensitive-opaque-cursor".to_owned()),
        ..AiSessionRetentionReport::default()
    };
    let projected = AiRetentionTelemetry::from_report(
        AiTelemetryOperationId::new(),
        Duration::from_secs(1),
        &report,
    );
    assert_eq!(projected.sessions_scanned(), 5);
    assert_eq!(projected.payloads_tombstoned(), 3);
    assert_eq!(projected.rows_deleted(), 4);
    assert_eq!(projected.messages_scrubbed(), 1);
    assert_eq!(projected.cleanups_requested(), 2);
    assert_eq!(projected.blocked(), 6);
    assert_eq!(projected.conflicted(), 1);
    assert!(projected.has_more());
    assert!(!format!("{projected:?}").contains("sensitive-opaque-cursor"));
}

#[test]
fn restore_projection_omits_fingerprints_issue_text_and_run_ids() {
    let run_id = AiRunId::new();
    let plan = AiRestorePlan {
        expected_module_fingerprint: "private-fingerprint".to_owned(),
        run_actions: vec![AiRestoredRunAction {
            run_id,
            disposition: AiRestoredRunDisposition::RecoveryRequired,
            clear_lease: true,
            reverify_provider_continuation: true,
            reverify_provider_file: false,
        }],
        approvals_to_revalidate: 2,
        consents_to_revalidate: 3,
        issues: vec![AiRestoreIssue {
            code: "PRIVATE_ISSUE_TEXT".to_owned(),
            severity: AiRestoreIssueSeverity::Fatal,
            resource_ref: Some("private-resource".to_owned()),
        }],
    };
    let projected = AiRestorePlanTelemetry::from_plan(
        AiTelemetryOperationId::new(),
        Duration::from_millis(2),
        &plan,
    );
    assert_eq!(projected.recovery_required(), 1);
    assert_eq!(projected.fatal_issues(), 1);
    let debug = format!("{projected:?}");
    assert!(!debug.contains("private-fingerprint"));
    assert!(!debug.contains("PRIVATE_ISSUE_TEXT"));
    assert!(!debug.contains(&run_id.0.to_string()));
}
