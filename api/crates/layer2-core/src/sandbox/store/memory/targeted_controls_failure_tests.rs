use super::*;
use crate::sandbox::{AgentLogEntry, AgentLogKind};

#[tokio::test]
async fn explicit_target_refusals_leave_no_admission_effect() {
    for case in 0..9 {
        let (store, agent, mut request, response) = fixture().await;
        let expected = match case {
            0 => {
                request.target.response_id = crate::ids::AgentResponseId::generate();
                CommandLedgerError::NotFound
            }
            1 => {
                request.target.turn_sequence += 1;
                CommandLedgerError::NotAdmissible
            }
            2 => {
                request.command.expected_agent_revision += 1;
                CommandLedgerError::Conflict
            }
            3 => {
                request.observed_agent_revision += 1;
                CommandLedgerError::Conflict
            }
            4 => {
                request.agent_created_at = "2026-09-06T00:00:00Z".into();
                CommandLedgerError::NotFound
            }
            5 => {
                let mut state = store.lock();
                let source = state
                    .agent_commands
                    .get_mut(&response.receipt.envelope.command_id)
                    .unwrap();
                source.status = AgentResponseStatus::Queued;
                source.work.dispatch = None;
                CommandLedgerError::NotAdmissible
            }
            6 => {
                store
                    .lock()
                    .agent_commands
                    .get_mut(&response.receipt.envelope.command_id)
                    .unwrap()
                    .status = AgentResponseStatus::Completed;
                CommandLedgerError::NotAdmissible
            }
            7 => {
                store
                    .lock()
                    .agents
                    .get_mut(&agent.id)
                    .unwrap()
                    .conversation
                    .active_turn = Some(30);
                CommandLedgerError::NotAdmissible
            }
            _ => {
                store.lock().agents.get_mut(&agent.id).unwrap().capabilities =
                    crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]);
                CommandLedgerError::NotAdmissible
            }
        };
        let before = counts(&store);
        assert_eq!(
            store.reserve_targeted_control(request).await.unwrap_err(),
            expected,
            "case {case}"
        );
        assert_eq!(counts(&store), before, "case {case}");
    }
}

#[tokio::test]
async fn malformed_binding_or_response_provenance_fails_closed() {
    for case in 0..6 {
        let (store, _, request, _) = fixture().await;
        let original = store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap();
        {
            let mut state = store.lock();
            let binding = state
                .targeted_controls
                .get_mut(&original.binding.command_id)
                .unwrap();
            match case {
                0 => binding.schema_version += 1,
                1 => binding.command_id = crate::ids::AgentCommandId::generate(),
                2 => binding.turn_sequence += 1,
                3 => binding.request_fingerprint = "a".repeat(64),
                4 => binding.response_command_id = crate::ids::AgentCommandId::generate(),
                _ => binding.agent_created_at = "invalid timestamp".into(),
            }
        }
        assert_eq!(
            store.lookup_targeted_control(request).await.unwrap_err(),
            CommandLedgerError::Corrupt
        );
    }
}

#[tokio::test]
async fn late_sidecar_failure_rolls_back_staged_command_key_order_and_audit() {
    let (store, _, request, response) = fixture().await;
    let poisoned = request
        .bind(request.command.record(2, 0, Some(2)).unwrap(), &response)
        .unwrap();
    store
        .lock()
        .targeted_controls
        .insert(poisoned.binding.command_id.clone(), poisoned.binding);
    let before = counts(&store);
    assert_eq!(
        store.reserve_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::Corrupt
    );
    assert_eq!(counts(&store), before);
}

#[tokio::test]
async fn retained_response_cannot_target_a_recreated_agent_with_reused_turn_numbers() {
    let (store, agent, mut request, _) = fixture().await;
    request.agent_created_at = "2026-09-06T00:00:00Z".into();
    store.lock().agents.get_mut(&agent.id).unwrap().created_at = request.agent_created_at.clone();
    let before = counts(&store);
    assert_eq!(
        store.reserve_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
    assert_eq!(counts(&store), before);
}

#[tokio::test]
async fn fresh_revision_policy_incarnation_and_deadline_fence_historical_replay() {
    for case in 0..4 {
        let (store, agent, mut request, _) = fixture().await;
        store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap();
        let expected = match case {
            0 => {
                store.lock().agents.get_mut(&agent.id).unwrap().revision += 1;
                CommandLedgerError::Conflict
            }
            1 => {
                request.command.policy.policy_revision += 1;
                CommandLedgerError::Conflict
            }
            2 => {
                store.lock().agents.get_mut(&agent.id).unwrap().created_at =
                    "2026-09-06T00:00:00Z".into();
                CommandLedgerError::NotFound
            }
            _ => {
                request.command.authority_deadline = Some(tokio::time::Instant::now());
                CommandLedgerError::Conflict
            }
        };
        let before = counts(&store);
        assert_eq!(
            store.lookup_targeted_control(request).await.unwrap_err(),
            expected
        );
        assert_eq!(counts(&store), before);
    }
}

#[tokio::test]
async fn approval_requires_pending_identity_from_exact_active_turn_snapshot() {
    for case in 0..5 {
        let (store, agent, mut request, _) = fixture().await;
        set_command(
            &mut request,
            AgentCommand::AnswerApproval {
                approval_id: "permission".into(),
                decision: AgentApprovalDecision::Allow,
            },
        );
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 2,
                origin_key: None,
            },
            AgentLogKind::ApprovalRequested {
                approval_id: "permission".into(),
                runtime_locator: Some("private-locator".into()),
                action: None,
                detail: None,
            },
        ];
        match case {
            1 => kinds.push(AgentLogKind::ApprovalResolved {
                approval_id: "permission".into(),
                decision: "allow".into(),
            }),
            2 => kinds.push(AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 4,
                origin_key: None,
            }),
            3 => set_command(
                &mut request,
                AgentCommand::AnswerApproval {
                    approval_id: "wrong-id".into(),
                    decision: AgentApprovalDecision::Allow,
                },
            ),
            _ => {}
        }
        let mut entries: Vec<_> = kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: index as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .collect();
        if case == 4 {
            entries.remove(1);
        }
        {
            let mut state = store.lock();
            state
                .agents
                .get_mut(&agent.id)
                .unwrap()
                .conversation
                .last_seq = entries.last().unwrap().seq;
            state.agent_logs.insert(agent.id.clone(), entries);
        }
        let before = counts(&store);
        let result = store.reserve_targeted_control(request).await;
        if case == 0 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.unwrap_err(),
                if case == 4 {
                    CommandLedgerError::Corrupt
                } else {
                    CommandLedgerError::NotAdmissible
                }
            );
            assert_eq!(counts(&store), before);
        }
    }
}
