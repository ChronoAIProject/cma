use super::*;
use crate::sandbox::store::{
    AgentMessageCheckpoint, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxWorkerStore, WorktreeStore,
};
use crate::sandbox::{
    AgentMessageAssembly, AgentOperationIntent, AgentOperationRecord, AgentStopReason,
    AgentUpdateOutcome, RuntimeBinding, SandboxImageRotationPhase, SandboxModelError,
    SandboxPausePhase, Worktree, WorktreeStatus,
};

const HOLDER: &str = "deleted-history-update";
const END: &str = "2026-09-05T01:00:00Z";

#[tokio::test]
async fn runtime_update_deleted_history_preserves_output_and_finishes_paused_on_same_home() {
    let store = InMemorySandboxStore::default();
    let mut sandbox = crate::sandbox::runtime_update_test_support::native();
    sandbox
        .begin_pause(NOW.into(), Some("Paused by you.".into()))
        .unwrap();
    for phase in 0..4 {
        let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
        match phase {
            0 => sandbox.complete_lifecycle_persistent_pause(HOLDER, fence, NOW.into()),
            1 => sandbox.complete_lifecycle_agent_pause(HOLDER, fence, NOW.into()),
            2 => sandbox.complete_lifecycle_runtime_suspension(HOLDER, fence, NOW.into()),
            _ => sandbox.publish_lifecycle_paused(HOLDER, fence, NOW.into()),
        }
        .unwrap();
    }
    sandbox.validate().unwrap();
    sandbox
        .request_runtime_update(TARGET_IMAGE.into(), true, NOW.into())
        .unwrap();
    sandbox.revision = 1;
    store.create_sandbox(sandbox.clone()).await.unwrap();

    let mut deleted = stored_agent(&sandbox, "deleted", "preserved-conversation", 120);
    deleted.conversation.runtime_generation = Some(1);
    deleted.conversation.active_turn = Some(98);
    let mut operation = AgentOperationRecord::new(
        97,
        1,
        98,
        AgentOperationIntent::send_message("historical request"),
        NOW.into(),
    )
    .unwrap();
    operation.mark_execution_observed(NOW.into()).unwrap();
    deleted.conversation.operation = Some(operation);
    deleted
        .transition(
            AgentStatus::Stopped,
            NOW.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, NOW.into(), None)
        .unwrap();
    deleted.validate_conversation().unwrap();
    let worktree = Worktree {
        id: deleted.worktree_id.clone(),
        workspace_id: deleted.workspace_id.clone(),
        sandbox_id: deleted.sandbox_id.clone(),
        owner: deleted.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/update/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "a".repeat(40),
        status: WorktreeStatus::Removed,
        agent_id: Some(deleted.id.clone()),
        revision: 5,
    };
    let checkpoint = AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        native_conversation_id: deleted.conversation.conversation_id.clone().unwrap(),
        runtime_generation: 1,
        active_turn: Some(98),
        after_seq: 120,
        assembly: AgentMessageAssembly {
            content: "x".repeat(66),
            ..Default::default()
        },
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
        output: Some(layer3_adapter::RuntimeOutputSource {
            native_conversation_id: deleted.conversation.conversation_id.clone().unwrap(),
            stream_id: uuid::Uuid::new_v4().to_string(),
            cursor: 673,
            payload_sha256: "b".repeat(64),
        }),
    };
    checkpoint.validate().unwrap();
    let history = vec![log(deleted.id.as_str(), 120, NOW)];
    {
        let mut state = store.lock();
        state.agents.insert(deleted.id.clone(), deleted.clone());
        state
            .worktrees
            .insert(worktree.id.clone(), worktree.clone());
        state.agent_logs.insert(deleted.id.clone(), history.clone());
        state
            .agent_message_checkpoints
            .insert(deleted.id.clone(), checkpoint.clone());
    }
    assert_eq!(
        store
            .schedule_image_rotation(rotation_request(&sandbox.id, TARGET_IMAGE))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let admitted = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(admitted.status, SandboxStatus::Rotating);
    assert_eq!(admitted.lifecycle.home, sandbox.lifecycle.home);
    assert_eq!(
        admitted.retained_runtime_binding(),
        sandbox.retained_runtime_binding()
    );
    assert!(admitted
        .retained_home_recovery()
        .unwrap()
        .resume_pause
        .is_some());
    let Some(SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        &admitted.lifecycle.operation
    else {
        panic!("rotation not admitted")
    };
    assert!(
        identity.agents.is_empty(),
        "Deleted Agents are never reopened"
    );
    let source = admitted.retained_home_recovery().unwrap().clone();
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_home_verification(&source, HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.reserve_lifecycle_runtime_generation(2, HOLDER, f, NOW.into())
            .map(|_| ())
    })
    .await;
    let binding = RuntimeBinding {
        runtime_id: sandbox
            .retained_runtime_binding()
            .unwrap()
            .runtime_id
            .clone(),
        runtime_version: TARGET_IMAGE.into(),
        boot_id: Some("c".repeat(32)),
        provision_generation: Some(2),
        bound_at: NOW.into(),
    };
    progress(&store, &sandbox.id, |v, f| {
        v.bind_lifecycle_runtime(binding, HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_conversation_reopen(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_queued_message_delivery(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.publish_lifecycle_active(END.into(), HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
    })
    .await;
    progress(&store, &sandbox.id, |v, f| {
        v.publish_lifecycle_paused(HOLDER, f, NOW.into())
    })
    .await;
    let next = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(next.status, SandboxStatus::Paused);
    assert_eq!(next.lifecycle.home, sandbox.lifecycle.home);
    assert_eq!(
        next.lifecycle.paused_notice,
        sandbox.lifecycle.paused_notice
    );
    assert_eq!(
        next.retained_runtime_binding().unwrap().runtime_version,
        TARGET_IMAGE
    );
    assert!(next
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_some());
    assert_eq!(
        store.get_agent(&deleted.id).await.unwrap(),
        Some(deleted.clone())
    );
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    let state = store.lock();
    assert_eq!(
        state.agent_message_checkpoints.get(&deleted.id),
        Some(&checkpoint)
    );
    assert_eq!(state.agent_logs.get(&deleted.id), Some(&history));
}

async fn progress(
    store: &InMemorySandboxStore,
    id: &SandboxId,
    apply: impl FnOnce(&mut Sandbox, u64) -> Result<(), SandboxModelError>,
) {
    let current = store
        .claim_sandbox_work(id, HOLDER, NOW.into(), END.into())
        .await
        .unwrap()
        .unwrap();
    let mut next = current.clone();
    apply(&mut next, current.work.lease.as_ref().unwrap().fence).unwrap();
    // Round-trip every durable phase to cover control-plane restarts.
    let next: Sandbox = serde_json::from_slice(&serde_json::to_vec(&next).unwrap()).unwrap();
    let agent_phase = matches!(
        current.lifecycle.operation,
        Some(
            SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::PauseAgents,
                ..
            } | SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::PauseAgents
                    | SandboxImageRotationPhase::ConversationReopen
                    | SandboxImageRotationPhase::QueuedMessageDelivery,
                ..
            }
        )
    );
    let events = if current.status != next.status {
        vec![SandboxEventKind::StatusChanged {
            from: current.status,
            to: next.status,
        }]
    } else {
        vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }]
    };
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&current),
            sandbox: next,
            agents: vec![],
            agent_scope: if agent_phase {
                SandboxLifecycleAgentScope::AllLive
            } else {
                SandboxLifecycleAgentScope::Touched
            },
            appended_at: NOW.into(),
            events,
        })
        .await
        .unwrap();
}
