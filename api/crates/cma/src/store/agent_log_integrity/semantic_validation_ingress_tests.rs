use super::*;
use crate::store::agent_log_integrity::proof::summary_tests::policy;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use layer2_core::sandbox::{
    AgentCleanupDebt, AgentLogKind::*, AgentOperationDelivery, AgentOperationKind, AgentStatus,
    AgentWorkReceipt, AgentWorkStep, AgentWorktreeRecovery, QueuedAgentMessage, TypedFailure,
    Worktree, WorktreeStatus,
};

const AT: &str = "2026-01-01T00:00:00Z";

fn initial() -> Agent {
    let mut agent = ready_agent(&active_sandbox("sbx_semantic_owned"), "agt_semantic_owned");
    agent.conversation.last_seq = 0;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent.conversation.active_turn = None;
    agent.conversation.queued_messages.clear();
    agent
}

fn push(agent: &mut Agent, entries: &mut Vec<AgentLogEntry>, kind: AgentLogKind) {
    let seq = entries.len() as u64 + 1;
    agent.conversation.last_seq = seq;
    match &kind {
        ConversationOpened {
            conversation_id,
            runtime_generation,
        } => {
            agent.conversation.conversation_id = Some(conversation_id.clone());
            agent.conversation.runtime_generation = Some(*runtime_generation);
        }
        RuntimeGenerationRetired {
            runtime_generation, ..
        } => {
            agent.conversation.runtime_generation = Some(*runtime_generation);
        }
        TurnStarted {
            turn_sequence,
            origin_key,
            ..
        } => {
            agent.conversation.active_turn = Some(*turn_sequence);
            if let Some(origin) = origin_key {
                agent
                    .conversation
                    .queued_messages
                    .retain(|message| origin != &format!("queued-message:{}", message.sequence));
            }
        }
        OwnerMessage { content } if agent.conversation.active_turn.is_none() => {
            agent
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(seq, content.clone(), AT.into()).unwrap());
        }
        TurnCompleted { .. } | TurnFailed { .. } | TurnInterrupted { .. } => {
            agent.conversation.active_turn = None;
        }
        _ => {}
    }
    entries.push(AgentLogEntry {
        agent_id: agent.id.clone(),
        seq,
        appended_at: AT.into(),
        kind,
    });
}

fn history(tail: Vec<AgentLogKind>) -> (Agent, Vec<AgentLogEntry>) {
    let mut agent = initial();
    let mut entries = Vec::new();
    let created = Created {
        workspace_id: agent.workspace_id.clone(),
        worktree_id: agent.worktree_id.clone(),
    };
    for kind in [
        created,
        ConversationOpened {
            conversation_id: "native-semantic".into(),
            runtime_generation: 1,
        },
    ]
    .into_iter()
    .chain(tail)
    {
        push(&mut agent, &mut entries, kind);
    }
    (agent, entries)
}

async fn compare(agent: &Agent, entries: &[AgentLogEntry]) {
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let data = policy.budget(IngressClass::Data).available_bytes();
    let expected = SemanticCheckpoint::validate_retained_semantics(agent, entries);
    let operation = validate_retained_semantics_admitted(agent, entries, &policy).unwrap();
    let charge = operation.requested_bytes();
    let actual = operation.await.unwrap();
    assert!(actual.belongs_to(&policy));
    assert_eq!(*actual, expected);
    assert_eq!(idle - root.available_bytes(), charge);
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), data);
    drop(actual);
    assert_eq!(root.available_bytes(), idle);
}

#[tokio::test]
async fn unfinished_boundaries_remain_valid_without_hiding_earlier_damage() {
    for fence in [false, true] {
        let (mut agent, mut entries) = history(vec![TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        }]);
        if fence {
            push(
                &mut agent,
                &mut entries,
                OwnerMessage {
                    content: "question".into(),
                },
            );
            push(
                &mut agent,
                &mut entries,
                OperationDeliveryChanged {
                    operation_sequence: 5,
                    operation: AgentOperationKind::Steer,
                    state: AgentOperationDelivery::DispatchFenced,
                },
            );
        }
        assert!(
            !SemanticCheckpoint::from_entries(&agent, &entries)
                .unwrap()
                .valid
        );
        assert_eq!(
            SemanticCheckpoint::validate_retained_semantics(&agent, &entries),
            Ok(())
        );
        compare(&agent, &entries).await;
        for case in ["timestamp", "identity", "lineage", "sequence", "head"] {
            let mut agent = agent.clone();
            let mut entries = entries.clone();
            match case {
                "timestamp" => entries[0].appended_at = "bad".into(),
                "identity" => entries[0].agent_id.0 = "agt_other".into(),
                "lineage" => agent.workspace_id.0 = "wks_other".into(),
                "sequence" => entries[1].seq = 8,
                "head" => agent.conversation.last_seq += 1,
                _ => unreachable!(),
            }
            assert!(SemanticCheckpoint::validate_retained_semantics(&agent, &entries).is_err());
            compare(&agent, &entries).await;
        }
        push(
            &mut agent,
            &mut entries,
            OwnerMessage {
                content: "adjacent".into(),
            },
        );
        assert_eq!(
            SemanticCheckpoint::validate_retained_semantics(&agent, &entries),
            Ok(())
        );
        compare(&agent, &entries).await;
    }
}

#[tokio::test]
async fn queue_comparison_and_complete_full_fold_preserve_original_verdicts() {
    for count in [0, 1, 4, 5, 17, 64, 129] {
        let (mut agent, mut entries) = history(vec![]);
        for _ in 0..count {
            push(
                &mut agent,
                &mut entries,
                OwnerMessage {
                    content: "q".repeat(8192),
                },
            );
        }
        assert_eq!(
            SemanticCheckpoint::validate_retained_semantics(&agent, &entries),
            Ok(())
        );
        compare(&agent, &entries).await;
        if count > 0 {
            let mut drift = agent.clone();
            drift.conversation.queued_messages[0].content.push('x');
            assert!(SemanticCheckpoint::validate_retained_semantics(&drift, &entries).is_err());
            compare(&drift, &entries).await;
            drift.conversation.queued_messages[0].state = QueuedMessageState::Delivered;
            compare(&drift, &entries).await;
            let turn_sequence = entries.len() as u64 + 1;
            push(
                &mut agent,
                &mut entries,
                TurnStarted {
                    turn_sequence,
                    origin_key: Some("queued-message:3".into()),
                    model: None,
                },
            );
            push(
                &mut agent,
                &mut entries,
                TurnMetadataSet {
                    turn_sequence,
                    metadata: (0..16)
                        .map(|i| (format!("key{i}"), "字".repeat(512)))
                        .collect(),
                },
            );
            push(
                &mut agent,
                &mut entries,
                AgentMessage {
                    content: "a".repeat(256 * 1024),
                },
            );
            for outcome in [
                TurnCompleted { turn_sequence },
                TurnInterrupted { turn_sequence },
                TurnFailed {
                    turn_sequence,
                    failure: TypedFailure {
                        class: "synthetic".into(),
                        detail: "failure".repeat(65536),
                    },
                },
            ] {
                let mut agent = agent.clone();
                let mut entries = entries.clone();
                push(&mut agent, &mut entries, outcome);
                assert_eq!(
                    SemanticCheckpoint::validate_retained_semantics(&agent, &entries),
                    Ok(())
                );
                compare(&agent, &entries).await;
            }
            push(
                &mut agent,
                &mut entries,
                ConversationGap { frames: u64::MAX },
            );
            push(&mut agent, &mut entries, TurnCompleted { turn_sequence });
            compare(&agent, &entries).await;
        }
    }
}

#[tokio::test]
async fn first_failure_sequence_and_domain_errors_are_preserved_and_owned() {
    let (agent, entries) = history(vec![]);
    compare(&agent, &[]).await;
    for case in [
        "zero",
        "overflow",
        "identity",
        "duplicate",
        "timestamp",
        "generation",
        "missing-created",
    ] {
        let mut agent = agent.clone();
        let mut entries = entries.clone();
        match case {
            "zero" => entries[0].seq = 0,
            "overflow" => entries[1].seq = u64::MAX,
            "identity" => entries[1].agent_id.0 = "agt_other".into(),
            "duplicate" => entries[1].kind = entries[0].kind.clone(),
            "timestamp" => entries[0].appended_at = "x".repeat(65536),
            "generation" => agent.conversation.runtime_generation = Some(8),
            "missing-created" => entries[0].kind = entries[1].kind.clone(),
            _ => unreachable!(),
        }
        let expected =
            SemanticCheckpoint::validate_retained_semantics(&agent, &entries).unwrap_err();
        if matches!(case, "zero" | "overflow" | "identity") {
            assert_eq!(expected, SandboxStoreError::SequenceConflict);
        }
        compare(&agent, &entries).await;
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let error = validate_retained_semantics_admitted(&agent, &entries, &policy)
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
        drop((agent, entries));
        assert_eq!(*error, expected);
        let pointer = match &*error {
            SandboxStoreError::Corrupt(message) => Some(message.as_ptr()),
            _ => None,
        };
        let moved = error.map(Some);
        assert!(moved.belongs_to(&policy));
        if let Some(SandboxStoreError::Corrupt(message)) = &*moved {
            assert_eq!(Some(message.as_ptr()), pointer);
        }
        drop(moved);
        assert_eq!(root.available_bytes(), idle);
    }
}

#[tokio::test]
async fn validation_uses_only_transformation_and_refuses_before_polling() {
    let (agent, entries) = history(vec![OwnerMessage {
        content: "queued".into(),
    }]);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let operation = validate_retained_semantics_admitted(&agent, &entries, &policy).unwrap();
    assert!(root.available_bytes() < idle);
    drop(operation);
    assert_eq!(root.available_bytes(), idle);
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "data pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let result = validate_retained_semantics_admitted(&agent, &entries, &policy)
        .unwrap()
        .await
        .unwrap();
    assert_eq!(*result, Ok(()));
    drop(result);
    drop(pressure);
    assert_eq!(root.available_bytes(), idle);
    let pressure = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "validation pressure",
                &[policy
                    .budget(IngressClass::Transformation)
                    .available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        validate_retained_semantics_admitted(&agent, &entries, &policy),
        Err(AdmittedError::Refused(_))
    ));
    drop(pressure);
    assert_eq!(root.available_bytes(), idle);
    let plan = ComputationPlan {
        working: AllocationPlan::checked("maximum", &[u32::MAX as usize]).unwrap(),
        retained: AllocationPlan::checked("one", &[1]).unwrap(),
    };
    assert!(matches!(
        plan.validation(),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
}

#[tokio::test]
async fn recovery_and_full_fold_diagnostics_preserve_the_original_error() {
    let (mut agent, mut entries) = history(vec![]);
    let name = "wt-1";
    let workspace = "s".repeat(4096);
    let base = "b".repeat(65536);
    let request = layer3_adapter::WorktreeRequest::new(
        format!("worktree:{workspace}:{name}:add:v1"),
        workspace.clone(),
        name,
        base.clone(),
    );
    let original_id = agent.worktree_id.clone();
    agent.worktree_id.0 = "wkt_successor".into();
    agent.revision = 2;
    agent.work.next_fence = 2;
    agent.work.worktree_recoveries.push(AgentWorktreeRecovery {
        operation_id: "recover-synthetic".into(),
        requested_by: agent.owner.clone(),
        admitted_at: AT.into(),
        original_worktree: Worktree {
            id: original_id,
            workspace_id: agent.workspace_id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            owner: agent.owner.clone(),
            name: name.into(),
            path: format!("/home/cma/{workspace}/.worktrees/{name}"),
            branch: format!("cma/{name}"),
            base_commit: base,
            status: WorktreeStatus::Failed,
            agent_id: Some(agent.id.clone()),
            revision: 1,
        },
        original_failure: TypedFailure {
            class: "agent_runtime_rejected".into(),
            detail: "runtime_mutation_outcome_unknown: synthetic".into(),
        },
        original_cleanup_debt: AgentCleanupDebt {
            step: AgentWorkStep::AddWorktree,
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
        },
        original_agent_revision: 1,
        original_log_sequence: 1,
        original_next_fence: 1,
        successor_worktree_id: agent.worktree_id.clone(),
    });
    agent.validate_worktree_recoveries().unwrap();
    assert!(
        SemanticCheckpoint::from_entries(&agent, &entries)
            .unwrap()
            .valid
    );
    assert_eq!(
        SemanticCheckpoint::validate_retained_semantics(&agent, &entries),
        Ok(())
    );
    compare(&agent, &entries).await;
    agent.work.worktree_recoveries[0]
        .original_cleanup_debt
        .request_sha256 = "bad".into();
    assert!(
        !SemanticCheckpoint::from_entries(&agent, &entries)
            .unwrap()
            .valid
    );
    assert_eq!(
        SemanticCheckpoint::validate_retained_semantics(&agent, &entries),
        Err(SandboxStoreError::Corrupt(
            "command ledger is corrupt".into()
        ))
    );
    compare(&agent, &entries).await;
    agent.work.worktree_recoveries.clear();
    agent.worktree_id = match &entries[0].kind {
        Created { worktree_id, .. } => worktree_id.clone(),
        _ => unreachable!(),
    };
    agent.status = AgentStatus::Starting;
    agent.ready_at = None;
    agent.work.step = AgentWorkStep::PublishReady;
    agent.work.receipts = vec![AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", agent.id),
        request_sha256: "c".repeat(64),
        completed_at: AT.into(),
    }];
    entries.truncate(1);
    push(
        &mut agent,
        &mut entries,
        WorkStepCompleted {
            step: AgentWorkStep::OpenConversation,
        },
    );
    assert!(layer2_core::sandbox::agent_io::answer_context_for_log(&agent, &entries).is_ok());
    compare(&agent, &entries).await;
}
