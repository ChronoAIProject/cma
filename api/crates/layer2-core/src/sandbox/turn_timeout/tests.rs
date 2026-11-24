use super::*;

pub(super) fn binding() -> TurnTimeoutBinding {
    TurnTimeoutBinding {
        agent_id: AgentId("agt_timeout".into()),
        creation_key: "creation-timeout".into(),
        creation_fingerprint: "a".repeat(64),
        command_id: AgentCommandId("cmd_timeout".into()),
        turn_sequence: 3,
        operation_sequence: 4,
        runtime_generation: 2,
        runtime_boot: "b".repeat(32),
        conversation_id: "conversation-timeout".into(),
        prompt_id: layer3_adapter::PromptIdentity::new("conversation-timeout", 2, 3)
            .as_str()
            .into(),
    }
}

fn obligation() -> TurnTimeout {
    TurnTimeout::admit(
        binding(),
        "712993f5-ce84-4605-91f9-1f61a9b644ab".into(),
        TurnTimeoutCause::ServiceDeadline,
        1_000,
        301_000,
    )
    .unwrap()
}

#[test]
fn restart_preserves_action_deadline_and_uncertain_cleanup() {
    let before = obligation();
    let after: TurnTimeout = serde_json::from_slice(&serde_json::to_vec(&before).unwrap()).unwrap();
    assert_eq!(before, after);
    assert_eq!(after.expire(1, 300_999), Err(TurnTimeoutError::NotDue));
    let terminal = after.expire(1, 301_000).unwrap();
    assert_eq!(terminal.action_id(), before.action_id());
    assert!(terminal.blocks_dispatch());
    assert_eq!(
        terminal.disposition,
        TimeoutDisposition::Terminal {
            committed_at_ms: 301_000,
            effect: TimeoutEffectState::Unknown,
        }
    );
    let restarted: TurnTimeout =
        serde_json::from_slice(&serde_json::to_vec(&terminal).unwrap()).unwrap();
    assert!(restarted.blocks_dispatch());
    assert_eq!(restarted.supersede(2), Err(TurnTimeoutError::Conflict));
    assert_eq!(
        restarted.expire(2, 401_000),
        Err(TurnTimeoutError::Conflict)
    );
}

#[test]
fn both_serial_orders_preserve_the_winning_disposition() {
    let due = obligation();
    let result_first = due.supersede(1).unwrap();
    assert!(!result_first.blocks_dispatch());
    assert_eq!(
        result_first.expire(1, 301_000),
        Err(TurnTimeoutError::Conflict)
    );
    assert_eq!(
        result_first.expire(2, 301_000),
        Err(TurnTimeoutError::Conflict)
    );
    let timeout_first = due.expire(1, 301_000).unwrap();
    assert_eq!(timeout_first.supersede(1), Err(TurnTimeoutError::Conflict));
    assert_eq!(timeout_first.supersede(2), Err(TurnTimeoutError::Conflict));
}

#[test]
fn exact_prompt_binding_rejects_stale_or_reconstructed_identity() {
    let valid = binding();
    valid.validate().unwrap();
    for change in [0, 1, 2] {
        let mut invalid = valid.clone();
        match change {
            0 => invalid.turn_sequence += 1,
            1 => invalid.runtime_generation += 1,
            _ => invalid.conversation_id.push_str("-other"),
        }
        assert_eq!(invalid.validate(), Err(TurnTimeoutError::Invalid));
    }
    let mut invalid = valid;
    invalid.runtime_boot.clear();
    assert_eq!(invalid.validate(), Err(TurnTimeoutError::Invalid));
}

#[test]
fn negative_times_bad_versions_and_unknown_fields_fail_closed() {
    let mut value = serde_json::to_value(obligation()).unwrap();
    value["version"] = 2.into();
    assert_eq!(
        serde_json::from_value::<TurnTimeout>(value.clone())
            .unwrap()
            .validate(),
        Err(TurnTimeoutError::Invalid)
    );
    value["version"] = 1.into();
    value["waiting_since_ms"] = (-1).into();
    assert_eq!(
        serde_json::from_value::<TurnTimeout>(value.clone())
            .unwrap()
            .validate(),
        Err(TurnTimeoutError::Invalid)
    );
    value["waiting_since_ms"] = 1_000.into();
    value["unrecognized_authority"] = true.into();
    assert!(serde_json::from_value::<TurnTimeout>(value).is_err());
}

#[test]
fn general_timeout_does_not_claim_an_approval_cause_or_no_send() {
    let failure = TurnTimeoutCause::ServiceDeadline.failure();
    assert_eq!(failure.class, "agent_turn_timeout");
    assert!(!failure.detail.contains("approval"));
    assert!(!failure.detail.contains("never sent"));
    assert_eq!(
        TurnTimeoutCause::ApprovalBudget.failure().class,
        "approval_timeout"
    );
}

#[test]
fn approval_deadline_requires_registration_and_a_pending_receipt() {
    use super::approval::*;
    let admitted = TurnTimeout::admit_approval(
        binding(),
        uuid::Uuid::new_v4().to_string(),
        ApprovalLimits::default(),
        ApprovalClockBounds {
            offset_min_ms: 0,
            offset_max_ms: 0,
            valid_until_ms: 901_000,
        },
        1_000,
    )
    .unwrap();
    assert!(!admitted.is_armed());
    assert!(admitted.expire(1, 901_000).is_err());
    let operation = ApprovalOperationBinding {
        operation_id: "bound-operation".into(),
        model_attempt_id: "bound-attempt".into(),
        provider: "external".into(),
        authority_digest: "c".repeat(64),
    };
    let receipt = PendingApprovalReceipt {
        binding: operation.clone(),
        review_id: "review".into(),
        revision: 1,
        created_at_ms: 1_000,
        expires_at_ms: 901_000,
    };
    assert!(admitted
        .observe_pending_approval(1, &receipt, 2_000)
        .is_err());
    let registered = admitted
        .register_approval_operation(1, operation, 1_000)
        .unwrap();
    assert!(!registered.is_armed());
    let pending = registered
        .observe_pending_approval(registered.revision(), &receipt, 2_000)
        .unwrap();
    assert_eq!(pending.deadline_at_ms(), 301_000);
    for field in ["deadline_at_ms", "waiting_since_ms"] {
        let mut extended = serde_json::to_value(&pending).unwrap();
        extended[field] = (extended[field].as_i64().unwrap() + 1).into();
        assert!(serde_json::from_value::<TurnTimeout>(extended)
            .unwrap()
            .validate()
            .is_err());
    }
    assert_eq!(pending.action_id(), admitted.action_id());
    let expired = pending.expire(pending.revision(), 301_000).unwrap();
    assert_eq!(expired.cause(), TurnTimeoutCause::ApprovalBudget);
    assert!(expired.blocks_dispatch());
    assert!(expired
        .observe_pending_approval(expired.revision(), &receipt, 302_000)
        .is_err());
    let accepted = pending
        .observe_approval_resolution(
            pending.revision(),
            &ResolvedApprovalReceipt {
                binding: receipt.binding,
                revision: 2,
                resolved_at_ms: 101_000,
                resolution: ApprovalResolution::Approved,
            },
            101_000,
        )
        .unwrap();
    assert!(!accepted.is_armed());
    assert!(accepted.expire(accepted.revision(), 301_000).is_err());
}

#[test]
fn denial_is_a_durable_decision_that_another_pending_receipt_cannot_delay() {
    use super::approval::*;
    let admitted = TurnTimeout::admit_approval(
        binding(),
        uuid::Uuid::new_v4().to_string(),
        ApprovalLimits::default(),
        ApprovalClockBounds {
            offset_min_ms: 0,
            offset_max_ms: 0,
            valid_until_ms: 901_000,
        },
        1_000,
    )
    .unwrap();
    let operation = ApprovalOperationBinding {
        operation_id: "bound-operation".into(),
        model_attempt_id: "bound-attempt".into(),
        provider: "external".into(),
        authority_digest: "c".repeat(64),
    };
    let registered = admitted
        .register_approval_operation(1, operation.clone(), 1_000)
        .unwrap();
    let receipt = PendingApprovalReceipt {
        binding: operation.clone(),
        review_id: "review".into(),
        revision: 1,
        created_at_ms: 1_000,
        expires_at_ms: 901_000,
    };
    let pending = registered
        .observe_pending_approval(registered.revision(), &receipt, 1_000)
        .unwrap();
    let denied = pending
        .observe_approval_resolution(
            pending.revision(),
            &ResolvedApprovalReceipt {
                binding: operation,
                revision: 2,
                resolved_at_ms: 2_000,
                resolution: ApprovalResolution::Denied,
            },
            2_000,
        )
        .unwrap();
    assert_eq!(denied.cause(), TurnTimeoutCause::ApprovalDenied);
    assert!(denied
        .observe_pending_approval(denied.revision(), &receipt, 3_000)
        .is_err());
    assert_eq!(
        denied.expire(denied.revision(), 2_000).unwrap().cause(),
        TurnTimeoutCause::ApprovalDenied
    );
}
