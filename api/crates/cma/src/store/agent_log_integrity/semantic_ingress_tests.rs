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
    let transformation = policy
        .budget(IngressClass::Transformation)
        .available_bytes();
    let expected = SemanticCheckpoint::from_entries(agent, entries);
    let actual = checkpoint_from_entries_admitted(agent, entries, &policy)
        .unwrap()
        .await
        .unwrap();
    assert!(actual.belongs_to(&policy));
    assert_eq!(*actual, expected);
    if actual.is_ok() {
        assert_eq!(
            policy
                .budget(IngressClass::Transformation)
                .available_bytes(),
            transformation
        );
        assert_eq!(
            idle - root.available_bytes(),
            ComputationPlan::inspect(agent, entries)
                .unwrap()
                .retained
                .requested_bytes()
        );
    }
    drop(actual);
    assert_eq!(root.available_bytes(), idle);
}

#[tokio::test]
async fn all_turn_boundaries_and_untrusted_fallbacks_match_the_original_fold() {
    for tail in [
        vec![TurnCompleted { turn_sequence: 3 }],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            AgentMessage {
                content: "  ".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![TurnFailed {
            turn_sequence: 3,
            failure: TypedFailure {
                class: "synthetic".into(),
                detail: "failure".repeat(100),
            },
        }],
        vec![TurnInterrupted { turn_sequence: 3 }],
        vec![
            ConversationGap { frames: u64::MAX },
            AgentMessage {
                content: "partial".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            OperationDeliveryChanged {
                operation_sequence: 5,
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
            },
            OwnerMessage {
                content: "steering".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            TurnMetadataSet {
                turn_sequence: 3,
                metadata: (0..16)
                    .map(|i| (format!("key{i}"), "字".repeat(512)))
                    .collect(),
            },
            Usage {
                input_tokens: 4,
                output_tokens: 5,
                context_used: 6,
                context_window: Some(10),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
    ] {
        let (mut agent, mut entries) = history(vec![]);
        compare(&agent, &entries).await;
        push(
            &mut agent,
            &mut entries,
            TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
        );
        assert!(
            !SemanticCheckpoint::from_entries(&agent, &entries)
                .unwrap()
                .valid
        );
        compare(&agent, &entries).await;
        push(
            &mut agent,
            &mut entries,
            OwnerMessage {
                content: "question".into(),
            },
        );
        for kind in tail {
            push(&mut agent, &mut entries, kind);
            compare(&agent, &entries).await;
        }
    }
}

#[tokio::test]
async fn empty_and_growing_queues_retain_only_independent_checkpoint_data() {
    for count in [0, 1, 4, 5, 17, 64, 129] {
        let (mut agent, mut entries) = history(vec![]);
        for i in 0..count {
            push(
                &mut agent,
                &mut entries,
                OwnerMessage {
                    content: format!("{i}{}", "x".repeat(4096)),
                },
            );
        }
        compare(&agent, &entries).await;
        compare(&agent, &[]).await;
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let expected = SemanticCheckpoint::from_entries(&agent, &entries).unwrap();
        let expected_bytes = ComputationPlan::inspect(&agent, &entries)
            .unwrap()
            .retained
            .requested_bytes();
        let retained = checkpoint_from_entries_admitted(&agent, &entries, &policy)
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        drop((agent, entries));
        assert_eq!(*retained, expected);
        assert_eq!(idle - root.available_bytes(), expected_bytes);
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
    }
}

#[tokio::test]
async fn repeated_queued_origins_large_messages_and_replaced_conversations_match() {
    let (mut agent, mut entries) = history(vec![]);
    push(
        &mut agent,
        &mut entries,
        OwnerMessage {
            content: "q".repeat(256 * 1024),
        },
    );
    for _ in 0..12 {
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
            AgentMessage {
                content: "a".repeat(256 * 1024),
            },
        );
        push(&mut agent, &mut entries, TurnCompleted { turn_sequence });
    }
    compare(&agent, &entries).await;
    assert!(
        !SemanticCheckpoint::from_entries(&agent, &entries)
            .unwrap()
            .valid
    );
    for generation in 2..8 {
        push(
            &mut agent,
            &mut entries,
            ConversationOpened {
                conversation_id: "c".repeat(512),
                runtime_generation: generation,
            },
        );
    }
    compare(&agent, &entries).await;
    for bytes in [0, 1, 512, 513, 65536] {
        let (agent, entries) = history(vec![ConversationOpened {
            conversation_id: "c".repeat(bytes),
            runtime_generation: 2,
        }]);
        compare(&agent, &entries).await;
    }
}

#[tokio::test]
async fn original_sequence_errors_and_semantic_damage_keep_their_classification() {
    let (agent, entries) = history(vec![]);
    for sequence in [0, 2, u64::MAX] {
        let mut changed = entries.clone();
        changed[0].seq = sequence;
        assert_eq!(
            SemanticCheckpoint::from_entries(&agent, &changed),
            Err(SandboxStoreError::SequenceConflict)
        );
        compare(&agent, &changed).await;
    }
    for case in [
        "timestamp",
        "identity",
        "lineage",
        "head",
        "duplicate",
        "generation",
    ] {
        let mut agent = agent.clone();
        let mut changed = entries.clone();
        match case {
            "timestamp" => changed[0].appended_at = "x".repeat(16384),
            "identity" => changed[0].agent_id.0 = "agt_other".into(),
            "lineage" => agent.workspace_id.0 = "wks_other".into(),
            "head" => agent.conversation.last_seq += 1,
            "duplicate" => changed[1].kind = changed[0].kind.clone(),
            "generation" => agent.conversation.runtime_generation = Some(7),
            _ => unreachable!(),
        }
        assert!(
            !SemanticCheckpoint::from_entries(&agent, &changed)
                .unwrap()
                .valid
        );
        compare(&agent, &changed).await;
    }
}

#[tokio::test]
async fn unpolled_drop_and_both_class_refusals_release_all_attempted_storage() {
    let (agent, entries) = history(vec![OwnerMessage {
        content: "queued".into(),
    }]);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let operation = checkpoint_from_entries_admitted(&agent, &entries, &policy).unwrap();
    assert!(root.available_bytes() < idle);
    drop(operation);
    assert_eq!(root.available_bytes(), idle);
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked(
                    "semantic pressure",
                    &[policy.budget(class).available_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        let result = checkpoint_from_entries_admitted(&agent, &entries, &policy);
        if class == IngressClass::Transformation {
            assert!(matches!(result, Err(AdmittedError::Refused(_))));
        } else {
            assert!(matches!(
                result.unwrap().await,
                Err(AdmittedError::Refused(_))
            ));
        }
        drop(pressure);
        assert_eq!(root.available_bytes(), idle);
    }
    let mut bytes = Bytes::default();
    assert!(matches!(
        bytes.copies(usize::MAX, 2),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert!(matches!(
        bytes.copies(u32::MAX as usize, 2),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
}

#[tokio::test]
async fn worktree_recovery_and_unpublished_identity_helpers_preserve_the_original_result() {
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
    compare(&agent, &entries).await;
    agent.work.worktree_recoveries[0]
        .original_cleanup_debt
        .request_sha256 = "bad".into();
    assert!(
        !SemanticCheckpoint::from_entries(&agent, &entries)
            .unwrap()
            .valid
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
