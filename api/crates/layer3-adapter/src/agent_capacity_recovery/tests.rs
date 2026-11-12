use super::*;

pub(crate) fn request(turn: Option<u64>) -> CapacityRecoveryRequest {
    CapacityRecoveryRequest {
        operation_key: "capacity-recovery:fixture".into(),
        writer_fence: 7,
        challenge: "f".repeat(32),
        target: RetainedAgentTarget {
            sandbox_id: "sandbox".into(),
            agent_id: "agent".into(),
            runtime: RuntimeHandle::new("retained-runtime"),
            boot_id: "b".repeat(32),
            provision_generation: 9,
            home_identity: "h".repeat(43),
            conversation_id: "retained-native".into(),
            conversation_generation: 2,
            original_open_operation: "agent:agent:open:v1".into(),
            original_open_sha256: "c".repeat(64),
            active_turn: turn,
        },
    }
}

pub(crate) fn confirmation(request: &CapacityRecoveryRequest) -> CapacityRecoveryConfirmation {
    CapacityRecoveryConfirmation {
        request: request.clone(),
        native_state: request
            .target
            .active_turn
            .map_or(ConfirmedAgentState::Idle, |turn| {
                ConfirmedAgentState::Working {
                    prompt_id: PromptIdentity::new(
                        &request.target.conversation_id,
                        request.target.conversation_generation,
                        turn,
                    )
                    .as_str()
                    .into(),
                }
            }),
        output: RetainedOutputBoundary {
            stream_id: "retained-output".into(),
            acknowledged_through: 12,
            first_retained: 13,
            observed_through: 15,
        },
    }
}

#[test]
fn independently_produced_state_must_match_the_exact_admitted_prompt() {
    for turn in [None, Some(5)] {
        let request = request(turn);
        let mut observed = confirmation(&request);
        observed.validate_for(&request).unwrap();
        if let ConfirmedAgentState::Working { prompt_id } = &observed.native_state {
            observed.native_state = ConfirmedAgentState::WaitingApproval {
                prompt_id: prompt_id.clone(),
                approval_id: "approval".into(),
            };
            observed.validate_for(&request).unwrap();
            observed.native_state = ConfirmedAgentState::Idle;
        } else {
            observed.native_state = ConfirmedAgentState::Working {
                prompt_id: "unrelated".into(),
            };
        }
        assert_eq!(
            observed.validate_for(&request),
            Err(CapacityRecoveryError::StateUnconfirmed)
        );
    }
}

#[test]
fn stale_challenge_fence_or_native_identity_never_confirms_recovery() {
    let request = request(Some(5));
    for variant in 0..8 {
        let mut observed = confirmation(&request);
        match variant {
            0 => observed.request.challenge = "e".repeat(32),
            1 => observed.request.writer_fence -= 1,
            2 => observed.request.target.boot_id = "d".repeat(32),
            3 => observed.request.target.home_identity = "g".repeat(43),
            4 => observed.request.target.conversation_id = "other".into(),
            5 => observed.request.target.provision_generation += 1,
            6 => observed.request.target.original_open_sha256 = "d".repeat(64),
            _ => observed.request.operation_key = "another-operation".into(),
        }
        assert_eq!(
            observed.validate_for(&request),
            Err(CapacityRecoveryError::IdentityConflict)
        );
    }
}

#[test]
fn output_gaps_overflow_and_unscoped_streams_refuse_confirmation() {
    let request = request(None);
    for variant in 0..5 {
        let mut observed = confirmation(&request);
        match variant {
            0 => observed.output.first_retained += 1,
            1 => observed.output.observed_through = 11,
            2 => observed.output.stream_id.clear(),
            3 => observed.output.acknowledged_through = u64::MAX,
            _ => observed.output.observed_through = u64::MAX,
        }
        assert_eq!(
            observed.validate_for(&request),
            Err(CapacityRecoveryError::OutputUnconfirmed)
        );
    }
}

#[test]
fn request_contract_rejects_unbounded_unknown_or_missing_authority() {
    for variant in 0..5 {
        let mut value = request(None);
        match variant {
            0 => value.target.agent_id = "x".repeat(257),
            1 => value.writer_fence = 0,
            2 => value.target.original_open_operation = "new-open".into(),
            3 => value.target.active_turn = Some(0),
            _ => value.target.home_identity = "invalid-home".into(),
        }
        assert_eq!(
            value.validate(),
            Err(CapacityRecoveryError::IdentityConflict)
        );
    }
    let mut value = serde_json::to_value(request(None)).unwrap();
    value["input"] = serde_json::json!("must never be replayed");
    assert!(serde_json::from_value::<CapacityRecoveryRequest>(value).is_err());
}
