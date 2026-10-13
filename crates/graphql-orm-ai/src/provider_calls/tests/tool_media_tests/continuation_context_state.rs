use super::*;
use crate::AiAgentContinuationTaskContextState as State;

#[tokio::test]
async fn native_task_context_classification_preserves_exact_completed_checkpoint() {
    Box::pin(approved_media_lifecycle(false, false, 0, Some(15))).await;
}

pub(super) async fn assert_native_context_states(
    fixture: &Fixture,
    custody: &Arc<Custody>,
    application: &OrmAiApplicationToolCallService,
    lease: &AiRunLease,
    continuation: &crate::AiAgentContinuation,
) {
    assert!(lease.lease_generation() > fixture.lease.lease_generation());
    assert_ne!(lease.attempt_id(), fixture.lease.attempt_id());
    let original = continuation.checkpoint_value();
    assert_eq!(
        continuation.task_context_state().unwrap(),
        State::VerifiedNativeNeedsTaskContext
    );
    assert_eq!(continuation.checkpoint_value(), original);
    let serialized_only =
        crate::AiAgentContinuation::from_checkpoint_value(original.clone()).unwrap();
    assert!(matches!(
        serialized_only.task_context_state(),
        Err(AiError::Conflict)
    ));

    let prepared = application
        .prepare_task_continuation_context(lease, &fixture.scope)
        .await
        .unwrap();
    let bound = match continuation.task_context_state().unwrap() {
        State::VerifiedNativeNeedsTaskContext => continuation
            .clone()
            .with_prepared_task_context(prepared.clone())
            .unwrap(),
        _ => panic!("owning native checkpoint must need its original task context"),
    };
    let context = bound.task_context.as_ref().unwrap().clone();
    let bytes = bound.conservative_input_bytes().unwrap();
    let bound = match bound.task_context_state().unwrap() {
        State::VerifiedNativeHasTaskContext => bound,
        _ => panic!("prepared native continuation must reuse the same opaque context"),
    };
    assert!(Arc::ptr_eq(bound.task_context.as_ref().unwrap(), &context));
    assert_eq!(bound.conservative_input_bytes().unwrap(), bytes);
    assert!(
        bound
            .clone()
            .with_prepared_task_context(prepared.clone())
            .is_err()
    );

    let mut old_lease_context = prepared.clone();
    old_lease_context.attempt_id = fixture.lease.attempt_id();
    old_lease_context.lease_generation = fixture.lease.lease_generation();
    assert!(
        continuation
            .clone()
            .with_prepared_task_context(old_lease_context)
            .is_err()
    );
    assert!(
        application
            .prepare_task_continuation_context(&fixture.lease, &fixture.scope)
            .await
            .is_err()
    );
    let mut foreign_scope = fixture.scope.clone();
    foreign_scope.id.push_str("-foreign");
    assert!(
        application
            .prepare_task_continuation_context(lease, &foreign_scope)
            .await
            .is_err()
    );

    for field in 0..4 {
        let mut changed = bound.clone();
        let changed_context = Arc::make_mut(changed.task_context.as_mut().unwrap());
        match field {
            0 => changed_context.session_id = crate::AiSessionId(Uuid::new_v4()),
            1 => changed_context.run_id = crate::AiRunId(Uuid::new_v4()),
            2 => changed_context.attempt_id = Uuid::new_v4(),
            3 => changed_context.lease_generation += 1,
            _ => unreachable!(),
        }
        assert!(matches!(
            changed.task_context_state(),
            Err(AiError::Conflict)
        ));
    }
    let mut unverified = continuation.clone();
    unverified.native_reset_evidence = None;
    assert!(matches!(
        unverified.task_context_state(),
        Err(AiError::Conflict)
    ));
    let mut wrong_hash = continuation.clone();
    Arc::make_mut(wrong_hash.native_reset_evidence.as_mut().unwrap()).outcome_hash = "f".repeat(64);
    assert!(matches!(
        wrong_hash.task_context_state(),
        Err(AiError::Conflict)
    ));
    let mut wrong_call = continuation.clone();
    Arc::make_mut(wrong_call.native_reset_evidence.as_mut().unwrap()).tool_call_id = Uuid::new_v4();
    assert!(matches!(
        wrong_call.task_context_state(),
        Err(AiError::Conflict)
    ));
    let mut changed_snapshot = original.clone();
    changed_snapshot["input"][0]["value"]["output"] = json!({"changed":true});
    assert!(crate::AiAgentContinuation::from_checkpoint_value(changed_snapshot).is_err());
    let outcome = &original["input"][0]["value"];
    let mut ordinary_snapshot = original.clone();
    ordinary_snapshot["formatVersion"] = json!(3);
    ordinary_snapshot["input"] = json!([ModelInputBlock::ToolResult {
        call_id: outcome["providerCallId"].as_str().unwrap().into(),
        tool_id: outcome["toolId"].as_str().unwrap().into(),
        output: outcome["output"].clone(),
    }]);
    let ordinary = crate::AiAgentContinuation::from_checkpoint_value(ordinary_snapshot).unwrap();
    let mut ordinary_with_evidence = ordinary.clone();
    ordinary_with_evidence.native_reset_evidence = continuation.native_reset_evidence.clone();
    assert!(matches!(
        ordinary_with_evidence.task_context_state(),
        Err(AiError::Conflict)
    ));
    let mut ordinary_with_context = ordinary;
    ordinary_with_context.task_context = Some(context.clone());
    assert!(matches!(
        ordinary_with_context.task_context_state(),
        Err(AiError::Conflict)
    ));

    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
    assert_eq!(continuation.checkpoint_value(), original);
    assert!(Arc::ptr_eq(bound.task_context.as_ref().unwrap(), &context));
}
