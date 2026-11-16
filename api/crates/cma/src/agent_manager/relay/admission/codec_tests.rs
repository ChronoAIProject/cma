use super::*;

#[test]
fn binding_policy_absence_is_distinct_from_no_assertion_across_peer_encoding() {
    for policy in [None, Some(None), Some(Some(7))] {
        let mutation = OwnerCommandMutation::Admit {
            submission: CommandSubmission {
                agent_id: AgentId("agt-binding-codec".into()),
                command: AgentCommand::message("same input"),
                idempotency_key: "retained-key".into(),
                expected_agent_revision: Some(42),
                requested_model: Some("fixed-model".into()),
            },
            binding_policy: policy,
        };
        let encoded = serde_json::to_vec(&Intent::from(mutation)).unwrap();
        let decoded: OwnerCommandMutation =
            serde_json::from_slice::<Intent>(&encoded).unwrap().into();
        let OwnerCommandMutation::Admit {
            submission,
            binding_policy,
        } = decoded
        else {
            panic!("wrong variant")
        };
        assert_eq!(binding_policy, policy);
        assert_eq!(submission.expected_agent_revision, Some(42));
        assert_eq!(submission.idempotency_key, "retained-key");
        assert_eq!(submission.requested_model.as_deref(), Some("fixed-model"));
        assert_eq!(submission.command, AgentCommand::message("same input"));
    }
}

#[test]
fn owner_refusals_keep_stale_revision_and_permanent_control_meanings() {
    let replies = [
        OwnerCommandReply::Admit(Err(CommandLedgerError::IdempotencyConflict.into())),
        OwnerCommandReply::TargetedControl(Err(TargetedControlError::NotAdmitted)),
        OwnerCommandReply::TargetedControl(Err(TargetedControlError::CapabilityUnsupported)),
        OwnerCommandReply::DirectTargetedControl(Err(CommandLedgerError::Conflict.into())),
        OwnerCommandReply::QueuedCancellation(Err(QueuedCancellationError::NotAdmitted)),
        OwnerCommandReply::QueuedInput(Err(QueuedCancellationError::Admission(
            CommandLedgerError::IdempotencyConflict.into(),
        ))),
        OwnerCommandReply::InterruptingMessage(Err(TargetedControlError::NotAdmitted)),
    ];
    for (index, reply) in replies.into_iter().enumerate() {
        let encoded = serde_json::to_vec(&Reply::from(reply)).unwrap();
        let decoded: OwnerCommandReply = serde_json::from_slice::<Reply>(&encoded).unwrap().into();
        assert!(matches!(
            (index, decoded),
            (
                0,
                OwnerCommandReply::Admit(Err(CommandAdmissionError::Ledger(
                    CommandLedgerError::IdempotencyConflict
                )))
            ) | (
                1,
                OwnerCommandReply::TargetedControl(Err(TargetedControlError::NotAdmitted))
            ) | (
                2,
                OwnerCommandReply::TargetedControl(Err(
                    TargetedControlError::CapabilityUnsupported
                ))
            ) | (
                3,
                OwnerCommandReply::DirectTargetedControl(Err(CommandAdmissionError::Ledger(
                    CommandLedgerError::Conflict
                )))
            ) | (
                4,
                OwnerCommandReply::QueuedCancellation(Err(QueuedCancellationError::NotAdmitted))
            ) | (
                5,
                OwnerCommandReply::QueuedInput(Err(QueuedCancellationError::Admission(
                    CommandAdmissionError::Ledger(CommandLedgerError::IdempotencyConflict)
                )))
            ) | (
                6,
                OwnerCommandReply::InterruptingMessage(Err(TargetedControlError::NotAdmitted))
            )
        ));
    }
}

#[test]
fn legal_queued_receipt_maximum_fits_the_shared_peer_reply_budget() {
    let long = "\\".repeat(256);
    let status = CommandStatusView {
        receipt: CommandReceiptView {
            command_id: layer2_core::ids::AgentCommandId(long.clone()),
            agent_id: AgentId(long.clone()),
            response_id: Some(layer2_core::ids::AgentResponseId(long.clone())),
            admitted_at: "2026-11-16T13:00:00.999999999+00:00".into(),
            agent_order: u64::MAX,
            event_cursor: u64::MAX,
        },
        status: AgentResponseStatus::Completed,
        queue_deadline: "2026-11-16T13:00:00.999999999+00:00".into(),
        turn_sequence: Some(u64::MAX),
        observed_cursor: Some(u64::MAX),
        delivery: None,
        failure: None,
    };
    let reply = Reply::from(OwnerCommandReply::QueuedInput(Ok(QueuedInputStatusView {
        edit: status.clone(),
        response: status,
        binding: QueuedInputBinding {
            schema_version: u16::MAX,
            command_id: layer2_core::ids::AgentCommandId(long.clone()),
            target: QueuedCancellationTarget {
                response_id: layer2_core::ids::AgentResponseId(long.clone()),
                source_command_id: layer2_core::ids::AgentCommandId(long.clone()),
                expected_command_revision: u64::MAX,
            },
            text: "\u{1}".repeat(layer2_core::sandbox::MAX_AGENT_MESSAGE_BYTES),
            command_revision: u64::MAX,
            agent_created_at: "2026-11-16T13:00:00.999999999+00:00".into(),
            request_fingerprint: "f".repeat(64),
        },
    })));
    let envelope = ResponseEnvelope {
        owner: OwnershipToken {
            agent_id: AgentId(long.clone()),
            instance_id: long,
            epoch: u64::MAX,
            protocol: layer2_core::agent_ownership::PROTOCOL_VERSION,
        },
        request_sha256: "f".repeat(64),
        reply: Ok(reply),
    };
    let bytes = serde_json::to_vec(&envelope).unwrap();
    assert!(bytes.len() > 6 * layer2_core::sandbox::MAX_AGENT_MESSAGE_BYTES);
    assert!(
        bytes.len() <= MAX_PEER_REPLY_BYTES,
        "{} > {}",
        bytes.len(),
        MAX_PEER_REPLY_BYTES
    );
    let decoded: ResponseEnvelope = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
}
