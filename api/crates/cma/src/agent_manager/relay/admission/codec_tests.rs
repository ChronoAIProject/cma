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
