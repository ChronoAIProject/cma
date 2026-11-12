use super::*;

struct Case {
    agent: Agent,
    command: AdmittedAgentCommand,
    log: Vec<AgentLogEntry>,
    projection: AgentLogProjectionState,
}

impl Case {
    async fn new() -> Self {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        admit(
            &store,
            &agent,
            &parent,
            "certified",
            AgentCommand::message("input"),
        )
        .await;
        let mut command = claimed(&store, &agent).await;
        command.status = AgentResponseStatus::InProgress;
        let at = crate::now_rfc3339();
        command.work.dispatch = Some(CommandDispatch {
            claim: command.work.lease.as_ref().unwrap().claim.clone(),
            operation_sequence: 3,
            turn_sequence: 4,
            runtime_generation: 3,
            fenced_at: at.clone(),
            delivery: AgentOperationDelivery::DispatchFenced,
        });
        let mut case = Self {
            agent,
            command,
            log: vec![],
            projection: Default::default(),
        };
        case.push(AgentLogKind::Created {
            workspace_id: case.agent.workspace_id.clone(),
            worktree_id: case.agent.creation_worktree_id().clone(),
        });
        case.push(AgentLogKind::ConversationOpened {
            conversation_id: "certified-native".into(),
            runtime_generation: 3,
        });
        case.push(delivery(AgentOperationDelivery::DispatchFenced));
        case.log[2].appended_at = at;
        case.push(AgentLogKind::TurnStarted {
            turn_sequence: 4,
            origin_key: Some(format!(
                "command:{}",
                case.command.receipt.envelope.command_id.as_str()
            )),
            model: None,
        });
        case.push(AgentLogKind::OwnerMessage {
            content: "input".into(),
        });
        case.agent.conversation.runtime_generation = Some(3);
        case.agent.conversation.active_turn = Some(4);
        case
    }

    fn push(&mut self, kind: AgentLogKind) {
        let entry = AgentLogEntry {
            agent_id: self.agent.id.clone(),
            seq: self.log.len() as u64 + 1,
            appended_at: self
                .command
                .work
                .dispatch
                .as_ref()
                .unwrap()
                .fenced_at
                .clone(),
            kind,
        };
        self.projection.observe(&self.agent, &entry, None).unwrap();
        self.agent.conversation.last_seq = entry.seq;
        self.log.push(entry);
    }

    fn facts(&self) -> &AgentLogTurnFacts {
        self.projection
            .active
            .as_ref()
            .or(self.projection.latest_completed.as_ref())
            .unwrap()
    }

    fn source(&self) -> CertifiedCommandSource<'_> {
        let facts = self.facts();
        let point = |sequence: u64| &self.log[sequence as usize - 1];
        let dispatch = self.command.work.dispatch.as_ref().unwrap();
        let latest = self.log.iter().rev().find(|row| matches!(row.kind,
            AgentLogKind::OperationDeliveryChanged {operation_sequence, ..} if operation_sequence == dispatch.operation_sequence)).unwrap();
        let mut pending = std::collections::BTreeMap::new();
        for row in &self.log {
            match &row.kind {
                AgentLogKind::ApprovalRequested { approval_id, .. } => {
                    pending.insert(approval_id, row);
                }
                AgentLogKind::ApprovalResolved { approval_id, .. } => {
                    pending.remove(approval_id);
                }
                _ => {}
            }
        }
        CertifiedCommandSource {
            through: self.agent.conversation.last_seq,
            facts,
            start: point(dispatch.turn_sequence),
            fence: point(dispatch.operation_sequence),
            delivery: latest,
            terminal: facts.end_sequence.map(point),
            prewrite: facts.prewrite_failure_sequence.map(point),
            last_approval: facts.approval_change_sequence.map(point),
            pending_approval: facts
                .end_sequence
                .is_none()
                .then(|| pending.values().next().copied())
                .flatten(),
        }
    }

    fn parity(&self) -> AdmittedAgentCommand {
        let mut full = CommandEvidence::new(&self.command).unwrap();
        for row in &self.log {
            full.observe(row).unwrap();
        }
        let sparse = CommandEvidence::from_certified_source(&self.command, self.source()).unwrap();
        let (mut left, mut right) = (self.command.clone(), self.command.clone());
        let mut lane = CommandLane {
            last_order: 1,
            next_fence: 2,
            lease: self.command.work.lease.clone(),
        };
        let mut other = lane.clone();
        assert_eq!(
            full.apply(&mut left, &mut lane, &self.agent),
            sparse.apply(&mut right, &mut other, &self.agent)
        );
        assert_eq!(
            serde_json::to_value(&left).unwrap(),
            serde_json::to_value(&right).unwrap()
        );
        assert_eq!(lane, other);
        left
    }
}

fn delivery(state: AgentOperationDelivery) -> AgentLogKind {
    AgentLogKind::OperationDeliveryChanged {
        operation_sequence: 3,
        operation: AgentOperationKind::SendMessage,
        state,
    }
}

fn requested(id: &str) -> AgentLogKind {
    AgentLogKind::ApprovalRequested {
        approval_id: id.into(),
        action: Some("test".into()),
        detail: Some("test".into()),
        runtime_locator: None,
    }
}

fn resolved(id: &str) -> AgentLogKind {
    AgentLogKind::ApprovalResolved {
        approval_id: id.into(),
        decision: "allow".into(),
    }
}

#[tokio::test]
async fn certified_settlement_matches_full_fold_above_both_history_limits() {
    for terminal in [
        AgentLogKind::TurnCompleted { turn_sequence: 4 },
        AgentLogKind::TurnInterrupted { turn_sequence: 4 },
        AgentLogKind::TurnFailed {
            turn_sequence: 4,
            failure: TypedFailure {
                class: "test".into(),
                detail: "failure".into(),
            },
        },
    ] {
        let mut case = Case::new().await;
        case.push(delivery(AgentOperationDelivery::WriteConfirmed));
        for _ in 0..11_000 {
            case.push(AgentLogKind::AgentMessage {
                content: "x".repeat(400),
            });
        }
        assert!(case.log.len() > 10_128);
        assert!(serde_json::to_vec(&case.log).unwrap().len() > 4 * 1024 * 1024);
        case.push(terminal);
        case.agent.conversation.active_turn = None;
        assert!(case.parity().status.is_terminal());
    }
}

#[tokio::test]
async fn certified_settlement_keeps_multiple_approvals_and_trigger_delivery_order() {
    for trigger in [false, true] {
        let mut case = Case::new().await;
        if trigger {
            case.command.receipt.envelope.actor = AgentActor::Trigger {
                subject: case.agent.owner.clone(),
                launch_id: crate::ids::LaunchId("launch-certification".into()),
            };
        }
        case.push(delivery(AgentOperationDelivery::RecoveryRequired));
        case.parity();
        case.push(requested("one"));
        case.push(requested("two"));
        assert_eq!(case.parity().status, AgentResponseStatus::WaitingAction);
        case.push(resolved("one"));
        assert_eq!(case.parity().status, AgentResponseStatus::WaitingAction);
        case.push(resolved("two"));
        assert_eq!(case.parity().status, AgentResponseStatus::InProgress);
        case.push(delivery(AgentOperationDelivery::RecoveryRequired));
        case.parity();
    }
}

#[tokio::test]
async fn certified_settlement_preserves_failure_and_interruption_precedence() {
    for lost_first in [false, true] {
        let mut case = Case::new().await;
        let lost = delivery(AgentOperationDelivery::RuntimeLost);
        let interrupted = AgentLogKind::TurnInterrupted { turn_sequence: 4 };
        for row in if lost_first {
            [lost, interrupted]
        } else {
            [interrupted, lost]
        } {
            case.push(row);
        }
        case.agent.conversation.active_turn = None;
        assert_eq!(
            case.parity().work.failure.unwrap().code,
            CommandFailureCode::RuntimeLost
        );
    }
    let mut case = Case::new().await;
    case.push(delivery(AgentOperationDelivery::PreWriteFailed));
    case.push(AgentLogKind::TurnFailed {
        turn_sequence: 4,
        failure: TypedFailure {
            class: "runtime_unavailable".into(),
            detail: "no write".into(),
        },
    });
    case.agent.conversation.active_turn = None;
    assert_eq!(
        case.parity().work.failure.unwrap().code,
        CommandFailureCode::RuntimeUnavailable
    );
}

#[tokio::test]
async fn certified_settlement_refuses_invalid_original_facts_without_mutating_inputs() {
    let mut case = Case::new().await;
    case.push(delivery(AgentOperationDelivery::WriteConfirmed));
    let original = case.command.clone();
    for fault in 0..7 {
        let mut facts = case.facts().clone();
        let mut fence = case.log[2].clone();
        let mut source = case.source();
        match fault {
            0 => facts.runtime_generation += 1,
            1 => facts.turn_sequence += 1,
            2 => fence.appended_at = "2020-01-01T00:00:00Z".into(),
            3 => fence.agent_id.0 = "agt_wrong".into(),
            4 => source.through = 3,
            5 => facts.end_sequence = Some(6),
            _ => source.delivery = &case.log[4],
        }
        source.facts = &facts;
        source.fence = &fence;
        assert!(
            CommandEvidence::from_certified_source(&case.command, source).is_err(),
            "fault {fault}"
        );
        assert_eq!(case.command, original);
    }
    let mut sealed = CommandEvidence::from_certified_source(&case.command, case.source()).unwrap();
    let mut next = case.log.last().unwrap().clone();
    next.seq += 1;
    assert!(sealed.observe(&next).is_err());
    let mut continuous = CommandEvidence::new(&case.command).unwrap();
    assert!(continuous.observe(&case.log[1]).is_err());
}

#[tokio::test]
async fn certified_control_uses_original_turn_and_cannot_release_another_claim() {
    let mut case = Case::new().await;
    case.command.receipt.envelope.command = AgentCommand::Interrupt;
    case.command.receipt.envelope.request_fingerprint =
        case.command.receipt.envelope.command.fingerprint().unwrap();
    case.command.receipt.response_id = None;
    case.command
        .work
        .dispatch
        .as_mut()
        .unwrap()
        .operation_sequence = 6;
    for state in [
        AgentOperationDelivery::DispatchFenced,
        AgentOperationDelivery::WriteConfirmed,
    ] {
        case.push(AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 6,
            operation: AgentOperationKind::Interrupt,
            state,
        });
    }
    assert_eq!(case.parity().status, AgentResponseStatus::Completed);
    let evidence = CommandEvidence::from_certified_source(&case.command, case.source()).unwrap();
    let mut newer = case.command.work.lease.clone().unwrap();
    newer.claim.fence += 1;
    let mut lane = CommandLane {
        last_order: 2,
        next_fence: 3,
        lease: Some(newer.clone()),
    };
    let mut command = case.command.clone();
    evidence
        .apply(&mut command, &mut lane, &case.agent)
        .unwrap();
    assert_eq!(lane.lease, Some(newer));
    assert!(command.work.lease.is_none());
}

#[tokio::test]
async fn certified_deferred_retry_requires_original_detail_adjacency_and_live_authority() {
    for fault in 0..4 {
        let mut case = Case::new().await;
        let envelope = &mut case.command.receipt.envelope;
        envelope.authorization.authority_id = "binding:certified".into();
        envelope.authorization.grant_revision = Some(1);
        envelope.continuation = Some(ExternalEventContinuation {
            parent_command_id: crate::ids::AgentCommandId("command-parent".into()),
            parent_response_id: crate::ids::AgentResponseId("response-parent".into()),
            parent_fingerprint: "a".repeat(64),
            binding_generation: 1,
            source_generation: None,
            source_expires_at: 2_000_000_000,
            artifact_id: "artifact".into(),
            source_proof: serde_json::json!({}),
        });
        let mut detail =
            crate::sandbox::agent_io::continuation_retry::deferred_detail(envelope, 3, 4);
        if fault == 1 {
            detail.push('x');
        }
        case.push(delivery(AgentOperationDelivery::PreWriteFailed));
        if fault == 2 {
            case.push(AgentLogKind::AgentMessage {
                content: "intervening".into(),
            });
        }
        case.push(AgentLogKind::TurnFailed {
            turn_sequence: 4,
            failure: TypedFailure {
                class: SOURCE_DEFERRED_FAILURE.into(),
                detail,
            },
        });
        case.agent.conversation.active_turn = None;
        let evidence =
            CommandEvidence::from_certified_source(&case.command, case.source()).unwrap();
        let mut full = CommandEvidence::new(&case.command).unwrap();
        for row in &case.log {
            full.observe(row).unwrap();
        }
        assert_eq!(evidence.has_source_deferred(), full.has_source_deferred());
        assert_eq!(evidence.has_source_deferred(), fault == 0 || fault == 3);
        let mut command = case.command.clone();
        evidence
            .apply_with_continuation_retry(
                &mut command,
                &mut CommandLane::default(),
                &case.agent,
                fault != 3,
            )
            .unwrap();
        assert_eq!(command.status == AgentResponseStatus::Queued, fault == 0);
        assert_eq!(
            command.work.deferred_attempts.len(),
            usize::from(fault == 0)
        );
        if fault == 3 {
            assert_eq!(
                command.work.failure.unwrap().code,
                CommandFailureCode::AuthorityRevoked
            );
        }
    }
}

#[tokio::test]
async fn certified_pending_index_cannot_contradict_the_original_latest_resolution() {
    let mut case = Case::new().await;
    case.push(requested("approval"));
    let request = case.log.last().unwrap().clone();
    case.push(resolved("approval"));
    let mut source = case.source();
    source.pending_approval = Some(&request);
    assert!(CommandEvidence::from_certified_source(&case.command, source).is_err());
    case.push(requested("approval"));
    let mut source = case.source();
    source.pending_approval = Some(&request);
    assert!(CommandEvidence::from_certified_source(&case.command, source).is_err());
}
