use super::*;

async fn running(
    ambiguous: bool,
) -> (
    std::sync::Arc<InMemorySandboxStore>,
    Agent,
    AdmittedAgentCommand,
    CommandLease,
) {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "paced",
        AgentCommand::message("one"),
    )
    .await;
    let work = claimed(&store, &agent).await;
    let result = if ambiguous {
        Err(AdapterError::Rejected("receipt lost".into()))
    } else {
        Ok(())
    };
    let (sender, _) = recording_sender(result);
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert_eq!(
        service
            .execute_claimed_command(&owner(), &work)
            .await
            .is_err(),
        ambiguous
    );
    let command = store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    (store, agent, command, work.work.lease.unwrap())
}

fn head(agent: &Agent) -> CommandReconcileHead {
    CommandReconcileHead {
        agent_id: agent.id.clone(),
        last_seq: agent.conversation.last_seq,
        live: agent.status.is_live(),
        runtime_generation: agent.conversation.runtime_generation,
        active_turn: agent.conversation.active_turn,
    }
}

#[tokio::test]
async fn current_predicate_requires_every_reconciliation_invariant() {
    let (_, agent, command, lease) = running(false).await;
    let current = head(&agent);
    assert!(command.reconciliation_current(&current));
    let mut wrong = current.clone();
    wrong.agent_id = crate::AgentId::generate();
    assert!(!command.reconciliation_current(&wrong));
    let mut wrong = current.clone();
    wrong.last_seq += 1;
    assert!(!command.reconciliation_current(&wrong));
    let mut wrong = current.clone();
    wrong.last_seq -= 1;
    assert!(!command.reconciliation_current(&wrong));
    let mut wrong = current.clone();
    wrong.live = false;
    assert!(!command.reconciliation_current(&wrong));
    for generation in [None, Some(current.runtime_generation.unwrap() + 1)] {
        let mut wrong = current.clone();
        wrong.runtime_generation = generation;
        assert!(!command.reconciliation_current(&wrong));
    }
    for turn in [None, Some(current.active_turn.unwrap() + 1)] {
        let mut wrong = current.clone();
        wrong.active_turn = turn;
        assert!(!command.reconciliation_current(&wrong));
        let mut control = command.clone();
        control.receipt.envelope.command = AgentCommand::Interrupt;
        assert!(control.reconciliation_current(&wrong));
    }
    let mut wrong = command.clone();
    wrong.work.dispatch = None;
    assert!(!wrong.reconciliation_current(&current));
    let mut wrong = command.clone();
    wrong.work.observation = None;
    assert!(!wrong.reconciliation_current(&current));
    let mut wrong = command.clone();
    wrong.status = AgentResponseStatus::Completed;
    assert!(!wrong.reconciliation_current(&current));
    let mut wrong = command.clone();
    wrong.work.failure = Some(CommandFailure {
        code: CommandFailureCode::RuntimeLost,
        at: crate::now_rfc3339(),
    });
    assert!(!wrong.reconciliation_current(&current));
    for delivery in [
        AgentOperationDelivery::WriteConfirmed,
        AgentOperationDelivery::ExecutionObserved,
    ] {
        let mut wrong = command.clone();
        wrong.work.observation.as_mut().unwrap().delivery = delivery;
        wrong.work.lease = Some(lease.clone());
        assert!(!wrong.reconciliation_current(&current));
        wrong.work.lease = None;
        assert!(wrong.reconciliation_current(&current));
    }
    let (_, agent, uncertain, _) = running(true).await;
    assert!(uncertain.work.lease.is_some());
    assert!(uncertain.reconciliation_current(&head(&agent)));
}

#[tokio::test]
async fn generated_current_prefixes_are_fixed_points_of_complete_evidence_folding() {
    // Exhaust the live-status and delivery classes, and generate growing valid
    // prefixes. The oracle is the production fold, not a copy of the predicate.
    for ambiguous in [false, true] {
        let (store, mut agent, command, _) = running(ambiguous).await;
        let prefix = store.agent_log(&agent.id, 0, 50).await.unwrap();
        for status in ALL_AGENT_STATUSES
            .into_iter()
            .filter(|status| status.is_live())
        {
            agent.status = status;
            for suffix in [0, 1, 2, 7, 16, 17, 63, 128] {
                let mut entries = prefix.clone();
                for _ in 0..suffix {
                    entries.push(AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: entries.len() as u64 + 1,
                        appended_at: crate::now_rfc3339(),
                        kind: AgentLogKind::WorkStepCompleted {
                            step: AgentWorkStep::None,
                        },
                    });
                }
                if suffix % 2 == 1 {
                    entries.push(AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: entries.len() as u64 + 1,
                        appended_at: crate::now_rfc3339(),
                        kind: AgentLogKind::ApprovalRequested {
                            approval_id: "pending".into(),
                            action: None,
                            detail: None,
                            runtime_locator: None,
                        },
                    });
                }
                agent.conversation.last_seq = entries.len() as u64;
                let mut folded = command.clone();
                let mut lane = CommandLane::default();
                let mut evidence = CommandEvidence::new(&folded).unwrap();
                for entry in &entries {
                    evidence.observe(entry).unwrap();
                }
                evidence.apply(&mut folded, &mut lane, &agent).unwrap();
                assert!(folded.reconciliation_current(&head(&agent)));
                let before = folded.clone();
                let mut evidence = CommandEvidence::new(&folded).unwrap();
                for entry in &entries {
                    evidence.observe(entry).unwrap();
                }
                assert!(!evidence.apply(&mut folded, &mut lane, &agent).unwrap());
                assert_eq!(folded, before);
            }
        }
    }
}

#[tokio::test]
async fn stopped_or_replaced_runtime_is_reconciled_as_lost() {
    for replaced in [false, true] {
        let (store, mut agent, mut command, _) = running(true).await;
        if replaced {
            agent.conversation.runtime_generation = Some(99);
        } else {
            agent.status = AgentStatus::Stopped;
        }
        assert!(!command.reconciliation_current(&head(&agent)));
        let mut evidence = CommandEvidence::new(&command).unwrap();
        for entry in store.agent_log(&agent.id, 0, 50).await.unwrap() {
            evidence.observe(&entry).unwrap();
        }
        assert!(evidence
            .apply(&mut command, &mut CommandLane::default(), &agent)
            .unwrap());
        assert_eq!(
            command.work.failure.unwrap().code,
            CommandFailureCode::RuntimeLost
        );
    }
}

#[tokio::test]
async fn memory_heads_are_bounded_deduplicated_and_omit_absent_agents() {
    let (store, agent, _, _) = running(false).await;
    let heads = store
        .command_reconcile_heads(&[
            agent.id.clone(),
            agent.id.clone(),
            crate::AgentId::generate(),
        ])
        .await
        .unwrap();
    assert_eq!(heads, vec![head(&agent)]);
    assert_eq!(
        store
            .command_reconcile_heads(&vec![agent.id; MAX_COMMAND_RECONCILE_HEADS + 1])
            .await,
        Err(CommandLedgerError::Invalid)
    );
}
