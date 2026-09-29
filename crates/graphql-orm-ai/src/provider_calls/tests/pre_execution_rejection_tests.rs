use super::*;

#[tokio::test]
async fn approved_refusal_closes_the_consumed_action_without_reinstating_approval() {
    let fixture = static_automatic_fixture().await;
    let result =
        commit_broker_provider_budget(&fixture, static_automatic_result(&fixture, "review")).await;
    let (service, approvals) = consequential_test_service(&fixture);
    let requested = service
        .request_approval(
            &fixture.lease,
            &result,
            automatic_mutation_context(&fixture),
            OffsetDateTime::now_utc() + Duration::minutes(5),
            false,
        )
        .await
        .unwrap();
    let row = AiApprovalRecord::find_by_id(&fixture.database, &requested.approval_id().0)
        .await
        .unwrap()
        .unwrap();
    approvals
        .decide_approval(
            &fixture.principal,
            DecideAiApprovalInput {
                id: row.id,
                decision: AiApprovalDecision::Approve,
                expected_version: row.row_version,
            },
        )
        .await
        .unwrap();
    fixture.refusal.store(1, Ordering::SeqCst);
    let outcome = service
        .execute_approved(
            requested.lease(),
            requested.approval_id(),
            requested.tool_call_id(),
            automatic_mutation_route(),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.persisted().unwrap().state(),
        crate::AiApplicationToolCallState::ExecutionFailed
    );
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    let approval = AiApprovalRecord::find_by_id(&fixture.database, &row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(approval.state, "consumed");
    assert_eq!(approval.consumed_uses, 1);
    assert!(
        service
            .execute_approved(
                requested.lease(),
                requested.approval_id(),
                requested.tool_call_id(),
                automatic_mutation_route()
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_refusal_binding_rejects_request_and_attempt_substitution() {
    let fixture = static_automatic_fixture().await;
    let result = static_automatic_result(&fixture, "54");
    let descriptor = fixture
        .runtime
        .tool_catalog()
        .descriptor(&AiToolId::parse("records.automatic").unwrap())
        .unwrap();
    let request = ToolGraphqlRequest {
        document: descriptor.document.clone(),
        operation_name: descriptor
            .graphql_contract
            .as_ref()
            .unwrap()
            .operation_name
            .clone(),
        contract: descriptor.graphql_contract.clone().unwrap(),
        variables: json!({"recordId":"54"}),
        invocation: crate::GraphqlInvocationContext {
            run_id: fixture.lease.run_id(),
            tool_call_id: crate::AiToolCallId(Uuid::new_v4()),
            scope: fixture.scope.clone(),
            correlation_id: "binding-test".into(),
            causation_id: "binding-test".into(),
            delegation_reference: None,
            idempotency_key: Some("idempotency-1".into()),
        },
    };
    let provenance = crate::AiToolExecutionProvenance::from_provider_call(
        &fixture.lease,
        &result,
        "static-mutation-call",
        descriptor,
        &request,
        None,
    )
    .unwrap();
    let direct_binding =
        crate::AiRegisteredToolExecutionBinding::static_operation(descriptor, &request).unwrap();
    assert!(matches!(
        crate::execution_rejection::before_executor_error(
            &direct_binding,
            &request,
            ToolExecutionError::Authorization
        ),
        ToolExecutionError::Authorization
    ));
    let binding = crate::AiRegisteredToolExecutionBinding::static_operation(descriptor, &request)
        .unwrap()
        .with_provenance(provenance.clone(), &request)
        .unwrap();
    let proof = binding
        .reject_before_execution(
            &request,
            crate::ToolPreExecutionRejectionReason::ConsentRequired,
        )
        .unwrap();
    assert!(matches!(
        crate::execution_rejection::before_executor_error(
            &binding, &request, ToolExecutionError::Authorization
        ),
        ToolExecutionError::RejectedBeforeExecution(proof)
            if proof.reason() == crate::ToolPreExecutionRejectionReason::AuthorizationDenied
    ));
    let foreign = crate::ToolPreExecutionRejection::attest_trusted(
        "a".repeat(64),
        crate::ToolPreExecutionRejectionReason::ConsentRequired,
    )
    .unwrap();
    assert!(matches!(
        crate::execution_rejection::before_executor_error(
            &binding,
            &request,
            ToolExecutionError::RejectedBeforeExecution(foreign)
        ),
        ToolExecutionError::Execution
    ));
    assert!(matches!(
        crate::execution_rejection::validate_rejection(&binding, &request, proof.clone()),
        ToolExecutionError::RejectedBeforeExecution(_)
    ));
    for index in 0..7 {
        let mut changed = request.clone();
        match index {
            0 => changed.variables = json!({"recordId":"different"}),
            1 => changed.invocation.tool_call_id = crate::AiToolCallId(Uuid::new_v4()),
            2 => changed.invocation.run_id = crate::AiRunId(Uuid::new_v4()),
            3 => changed.invocation.idempotency_key = Some("different".into()),
            4 => changed.invocation.scope.tenant_id = Some("different".into()),
            5 => changed.invocation.correlation_id = "different".into(),
            _ => changed.contract.schema_fingerprint = "different".into(),
        }
        assert!(matches!(
            crate::execution_rejection::validate_rejection(&binding, &changed, proof.clone()),
            ToolExecutionError::Execution
        ));
    }
    for field in [
        "attempt_id",
        "lease_generation",
        "approval_id",
        "policy_version",
        "authorization_state_digest",
    ] {
        let mut changed = serde_json::to_value(&provenance).unwrap();
        changed[field] = match field {
            "attempt_id" | "approval_id" => json!(Uuid::new_v4()),
            "lease_generation" => json!(999),
            _ => json!("different"),
        };
        if field == "approval_id" {
            changed["approval_binding_hash"] = json!("b".repeat(64));
        }
        // Deserialized provenance alone grants no authority. Here a trusted test
        // changes its exact binding to establish that old proof cannot transfer.
        let origin = serde_json::from_value(changed).unwrap();
        let changed_binding = binding.clone().with_provenance(origin, &request).unwrap();
        assert!(matches!(
            crate::execution_rejection::validate_rejection(
                &changed_binding,
                &request,
                proof.clone()
            ),
            ToolExecutionError::Execution
        ));
    }
}

#[tokio::test]
async fn trusted_pre_execution_refusal_is_durable_and_keeps_the_run_alive() {
    // Exercise static and generated automatic operations through the real
    // protected SQLite lifecycle, not only an enum classification.
    for generated in [false, true] {
        let fixture = if generated {
            automatic_mutation_fixture().await
        } else {
            static_automatic_fixture().await
        };
        fixture.refusal.store(1, Ordering::SeqCst);
        let result = if generated {
            automatic_mutation_result(&fixture)
        } else {
            static_automatic_result(&fixture, "54")
        };
        let service = automatic_mutation_service(&fixture, fixture.audit.clone());
        let outcome = service
            .execute_automatic_mutation(
                &fixture.lease,
                &result,
                automatic_mutation_context(&fixture),
                automatic_mutation_route(),
            )
            .await
            .unwrap();
        let persisted = outcome
            .persisted()
            .expect("trusted refusal must close the tool call");
        assert_eq!(
            persisted.state(),
            crate::AiApplicationToolCallState::ExecutionFailed
        );
        let Some(ModelInputBlock::ToolResult { output, .. }) = persisted.model_input() else {
            panic!("the model must receive the refusal through authorized egress");
        };
        assert_eq!(
            output,
            &json!({"version": 1, "ok": false,
            "code": "not_started_authentication_required", "retryable": false})
        );
        assert!(matches!(
            persisted.durable_outcome(),
            Some(crate::AiDurableApplicationToolOutcome::Failure { .. })
        ));
        assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
        let call = AiToolCallRecord::find_by_id(&fixture.database, &outcome.tool_call_id().0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.state, "execution_failed");
        assert_eq!(
            call.authorization_code.as_deref(),
            Some("not_started_authentication_required")
        );
        let run = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.state, AiRunState::Running.as_str());
        assert!(run.error_code.is_none());
        assert!(
            service
                .execute_automatic_mutation(
                    &fixture.lease,
                    &result,
                    automatic_mutation_context(&fixture),
                    automatic_mutation_route()
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn foreign_refusal_and_bare_graphql_codes_do_not_prove_no_execution() {
    for mode in [2, 3, 0] {
        let fixture = static_automatic_fixture().await;
        fixture.refusal.store(mode, Ordering::SeqCst);
        fixture.fail_execution.store(mode == 0, Ordering::SeqCst);
        let result = static_automatic_result(&fixture, "54");
        let service = automatic_mutation_service(&fixture, fixture.audit.clone());
        let outcome = service
            .execute_automatic_mutation(
                &fixture.lease,
                &result,
                automatic_mutation_context(&fixture),
                automatic_mutation_route(),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                AiConsequentialToolCallOutcome::RecoveryRequired { .. }
            ),
            "mode {mode}"
        );
        assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
        let run = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.state, AiRunState::RecoveryRequired.as_str());
        assert_eq!(
            run.error_code.as_deref(),
            Some("automatic_mutation_uncertain")
        );
    }
}

#[test]
fn refusal_categories_are_closed_content_free_and_never_blindly_retryable() {
    use crate::ToolPreExecutionRejectionReason as Reason;
    for reason in [
        Reason::AuthenticationRequired,
        Reason::AuthorizationDenied,
        Reason::ConsentRequired,
        Reason::ResourceUnavailable,
        Reason::CapabilityUnsupported,
        Reason::CapabilityStale,
        Reason::InvalidArguments,
        Reason::ResourceLimitExceeded,
        Reason::TemporarilyUnavailable,
    ] {
        let proof =
            crate::ToolPreExecutionRejection::attest_trusted("b".repeat(64), reason).unwrap();
        let code = crate::classify_safe_application_tool_error(
            &AiError::ToolRejectedBeforeExecution(proof),
        )
        .unwrap();
        let output = crate::AiApplicationToolFailureEnvelope::new(code).to_json();
        assert_eq!(output["code"], reason.failure_code());
        assert_eq!(output["retryable"], false);
        assert_eq!(output.as_object().unwrap().len(), 4);
        assert!(!output.to_string().contains(&"b".repeat(64)));
        assert_eq!(
            Reason::from_failure_code(reason.failure_code()),
            Some(reason)
        );
    }
    assert!(Reason::from_failure_code("private resolver message").is_none());
    assert!(
        crate::ToolPreExecutionRejection::attest_trusted(
            "private".to_owned(),
            Reason::ConsentRequired
        )
        .is_err()
    );
}
