use std::sync::Arc;

use layer3_adapter::{ConversationUpdate, SessionUpdate};

use super::super::{
    AgentLogStore, AgentStore, PageRequest, SandboxEventStore, SandboxLifecycleAgentMutation,
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard, SandboxReconciliationStore, SandboxRuntimeLossOutcome,
    SandboxRuntimeLossRequest, SandboxStore, SandboxStoreError,
};
use super::InMemorySandboxStore;
use crate::sandbox::lifecycle_test_support::{agent, checkpoint, sandbox, IDLE_AT, NOW};
use crate::sandbox::*;
use crate::{AgentId, SandboxId, SubjectId};

#[path = "retirement_loss_tests.rs"]
mod retirement_loss_tests;

#[tokio::test]
async fn authority_scan_includes_every_state_owner_and_bound_runtime_in_stable_order() {
    let store = InMemorySandboxStore::default();
    let statuses = [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ];
    for (index, status) in statuses.into_iter().enumerate().rev() {
        let mut value = sandbox();
        value.id = SandboxId(format!("sbx_{index:02}"));
        value.owner = SubjectId(format!("owner-{index}"));
        value.name = value.id.to_string();
        value.lifecycle.checkpoints.latest = Some(checkpoint(1, IDLE_AT));
        match status {
            SandboxStatus::Pausing => value.begin_pause(NOW.into(), None).unwrap(),
            SandboxStatus::Reviving => {
                value.status = SandboxStatus::Paused;
                value.runtime = None;
                value
                    .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
                    .unwrap();
            }
            SandboxStatus::Rotating => {
                let identity = SandboxRotationIdentity::capture(&value, &[]).unwrap();
                value
                    .begin_image_rotation("image-v2".into(), identity, NOW.into())
                    .unwrap();
            }
            _ => value.status = status,
        }
        if status == SandboxStatus::Paused {
            value.runtime = None;
        }
        if status == SandboxStatus::Failed {
            value.failure = Some(TypedFailure {
                class: "runtime_lost".into(),
                detail: "gone".into(),
            });
        }
        if status == SandboxStatus::Retired {
            value.retired_at = Some(NOW.into());
        }
        value.validate().unwrap();
        store.lock().sandboxes.insert(value.id.clone(), value);
    }
    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let page = store
            .sandbox_runtime_authority(&PageRequest::new(3, cursor).unwrap())
            .await
            .unwrap();
        found.extend(page.items);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        found
            .iter()
            .map(|sandbox| sandbox.status)
            .collect::<Vec<_>>(),
        statuses
    );
    assert_eq!(
        found
            .iter()
            .map(|sandbox| sandbox.id.as_str())
            .collect::<Vec<_>>(),
        (0..11)
            .map(|index| format!("sbx_{index:02}"))
            .collect::<Vec<_>>()
    );
    assert!(found
        .iter()
        .find(|sandbox| sandbox.status == SandboxStatus::Retired)
        .unwrap()
        .runtime
        .is_some());
    assert_eq!(
        store
            .sandbox_runtime_authority(&PageRequest {
                limit: 0,
                cursor: None
            })
            .await,
        Err(SandboxStoreError::InvalidPage)
    );
}

#[tokio::test]
async fn active_loss_fences_every_agent_and_preserves_checkpoint_lineage_and_owner_logs() {
    let mut live = agent("agt_a", "conversation-a", 4);
    live.status = AgentStatus::WaitingApproval;
    live.conversation.active_turn = Some(2);
    live.conversation.operation = Some(
        AgentOperationRecord::new(
            1,
            4,
            2,
            AgentOperationIntent::send_message("owner work"),
            IDLE_AT.into(),
        )
        .unwrap(),
    );
    live.work
        .schedule(AgentWorkStep::PublishConnection, IDLE_AT.into());
    live.work
        .claim("old-worker".into(), IDLE_AT, NOW.into())
        .unwrap();
    let mut attempted = QueuedAgentMessage::new(1, "owner work".into(), IDLE_AT.into()).unwrap();
    attempted.fence_dispatch(IDLE_AT.into()).unwrap();
    live.conversation.queued_messages = vec![
        attempted,
        QueuedAgentMessage::new(4, "never attempted".into(), NOW.into()).unwrap(),
    ];
    let mut newer = agent("agt_new", "conversation-new", 1);
    newer.status = AgentStatus::Starting;
    newer.ready_at = None;
    newer.conversation.conversation_id = None;
    newer.conversation.runtime_generation = None;
    newer.conversation.first_message =
        Some(FirstMessageRecord::new("launch-new", "first work", NOW).unwrap());
    let (store, original) = fixture(vec![live.clone(), newer.clone()], true);
    let before_logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    let request = loss_request(&original);
    assert_eq!(
        store.recover_runtime_loss(request.clone()).await.unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    let recovered = store.get_sandbox(&original.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, SandboxStatus::Reviving);
    assert_eq!(recovered.runtime, None);
    assert_eq!(
        recovered.lifecycle.checkpoints,
        original.lifecycle.checkpoints
    );
    assert_eq!(recovered.profile, original.profile);
    assert_eq!(recovered.credential, original.credential);
    assert_eq!(recovered.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert_eq!(recovered.work.lease, None);
    assert!(matches!(
        recovered.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::CheckpointVerification,
            cause: SandboxReviveCause::RuntimeLost,
            checkpoint_epoch: Some(1),
            ..
        })
    ));
    assert!(recovered
        .lifecycle
        .paused_notice
        .as_ref()
        .unwrap()
        .contains(IDLE_AT));
    let parked = store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(parked.status, AgentStatus::Paused);
    assert_eq!(parked.conversation.runtime_generation, Some(5));
    assert_eq!(
        parked.conversation.conversation_id,
        live.conversation.conversation_id
    );
    assert_eq!(parked.conversation.active_turn, None);
    assert_eq!(parked.work.step, AgentWorkStep::None);
    assert_eq!(parked.work.lease, None);
    assert!(!parked.work.owns("old-worker", 1));
    let operation = parked.conversation.operation.as_ref().unwrap();
    assert_eq!(operation.state, AgentOperationDelivery::RuntimeLost);
    assert_eq!(operation.write_confirmed_at, None);
    assert_eq!(operation.execution_observed_at, None);
    assert!(!operation.blocks_dispatch());
    assert_eq!(parked.conversation.queued_messages.len(), 1);
    assert_eq!(
        parked.conversation.queued_messages[0].content,
        "never attempted"
    );
    assert_eq!(
        parked.conversation.queued_messages[0].state,
        QueuedMessageState::Queued
    );
    let logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    assert_eq!(&logs[..before_logs.len()], &before_logs);
    assert!(logs
        .iter()
        .any(|entry| entry.kind == AgentLogKind::TurnInterrupted { turn_sequence: 2 }));
    assert!(logs.iter().any(|entry| matches!(
        entry.kind,
        AgentLogKind::OperationDeliveryChanged {
            state: AgentOperationDelivery::RuntimeLost,
            ..
        }
    )));
    assert_eq!(parked.conversation.last_seq, logs.last().unwrap().seq);
    let stopped = store.get_agent(&newer.id).await.unwrap().unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::RuntimeLost));
    let first = stopped.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(first.dispatch_fenced_at, None);
    assert_eq!(first.write_confirmed_at, None);
    assert_eq!(first.text, "first work");
    first.validate().unwrap();
    let events = store.sandbox_events(&original.id, 0, 100).await.unwrap();
    assert_eq!(
        store.recover_runtime_loss(request).await.unwrap(),
        SandboxRuntimeLossOutcome::Superseded
    );
    assert_eq!(
        store.sandbox_events(&original.id, 0, 100).await.unwrap(),
        events
    );
    assert_eq!(store.agent_log(&live.id, 0, 100).await.unwrap(), logs);
}

#[tokio::test]
async fn loss_without_checkpoint_fails_once_and_stops_every_live_agent() {
    let live = agent("agt_a", "conversation-a", 1);
    let (store, original) = fixture(vec![live.clone()], false);
    assert_eq!(
        store
            .recover_runtime_loss(loss_request(&original))
            .await
            .unwrap(),
        SandboxRuntimeLossOutcome::Failed
    );
    let failed = store.get_sandbox(&original.id).await.unwrap().unwrap();
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.failure.as_ref().unwrap().class, "runtime_lost");
    assert_eq!(failed.runtime, None);
    assert_eq!(failed.work.step, SandboxWorkStep::None);
    assert_eq!(
        store.get_agent(&live.id).await.unwrap().unwrap().status,
        AgentStatus::Stopped
    );
    assert!(store.sandbox_events(&original.id, 0, 100).await.unwrap().iter().any(|event|
        matches!(&event.kind, SandboxEventKind::Failed { failure } if failure.class == "runtime_lost")));
}

#[tokio::test]
async fn recovery_preserves_unattempted_first_message_on_a_checkpoint_covered_agent() {
    let mut live = agent("agt_a", "conversation-a", 1);
    live.conversation.first_message =
        Some(FirstMessageRecord::new("launch", "first work", NOW).unwrap());
    let (store, original) = fixture(vec![live.clone()], true);
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    let parked = store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(parked.status, AgentStatus::Paused);
    assert_eq!(
        parked.conversation.first_message,
        live.conversation.first_message
    );
    assert_eq!(parked.work.step, AgentWorkStep::None);
}

#[tokio::test]
async fn covered_queued_first_message_reopen_requires_exact_fresh_dispatch_work() {
    let mut live = agent("agt_a", "conversation-a", 1);
    live.conversation.first_message =
        Some(FirstMessageRecord::new("launch", "first work", NOW).unwrap());
    let (store, original) = fixture(vec![live.clone()], true);
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    let mut current = store.get_sandbox(&original.id).await.unwrap().unwrap();
    current.runtime = original.runtime;
    current.runtime.as_mut().unwrap().provision_generation = Some(6);
    current.lifecycle.runtime_generation_high_water = 6;
    if let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
        current.lifecycle.operation.as_mut()
    {
        *phase = SandboxRevivePhase::ConversationReopen;
    }
    let fence = current
        .work
        .claim("restorer".into(), NOW, "2026-09-05T01:00:00Z".into())
        .unwrap();
    store
        .lock()
        .sandboxes
        .insert(current.id.clone(), current.clone());
    let parked = store.get_agent(&live.id).await.unwrap().unwrap();
    let mut reopened = parked.clone();
    reopened.conversation.runtime_generation = Some(6);
    reopened
        .work
        .receipts
        .push(crate::sandbox::AgentWorkReceipt {
            step: AgentWorkStep::OpenConversation,
            operation_id: format!("agent:{}:open:v1", reopened.id),
            request_sha256: "a".repeat(64),
            completed_at: NOW.into(),
        });
    reopened
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    reopened.conversation.last_seq += 2;
    let mut next = current.clone();
    next.complete_lifecycle_conversation_reopen("restorer", fence, NOW.into())
        .unwrap();
    let mut mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&current),
        sandbox: next,
        agents: vec![SandboxLifecycleAgentMutation {
            expected_revision: parked.revision,
            agent: reopened,
            logs: vec![
                AgentLogKind::ConversationOpened {
                    conversation_id: "conversation-a".into(),
                    runtime_generation: 6,
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Paused,
                    to: AgentStatus::Ready,
                },
            ],
        }],
        agent_scope: SandboxLifecycleAgentScope::AllLive,
        appended_at: NOW.into(),
        events: vec![
            SandboxEventKind::AgentStatusChanged {
                agent_id: live.id.clone(),
                from: AgentStatus::Paused,
                to: AgentStatus::Ready,
            },
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            },
        ],
    };
    assert_eq!(
        store.commit_sandbox_lifecycle(mutation.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&live.id).await.unwrap(), Some(parked));
    mutation.agents[0]
        .agent
        .work
        .schedule(AgentWorkStep::DispatchFirstMessage, NOW.into());
    store.commit_sandbox_lifecycle(mutation).await.unwrap();
    let ready = store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(
        ready.conversation.first_message,
        live.conversation.first_message
    );
}

#[tokio::test]
async fn recovery_parks_two_captured_conversations_and_leaves_terminal_history_unchanged() {
    use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
    use sha2::{Digest, Sha256};
    let first = agent("agt_a", "conversation-a", 1);
    let second = agent("agt_b", "conversation-b", 1);
    let mut terminal = agent("agt_c", "conversation-c", 1);
    terminal
        .transition(
            AgentStatus::Stopped,
            IDLE_AT.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    let agents = vec![first.clone(), second.clone(), terminal.clone()];
    let (store, mut original) = fixture(agents.clone(), false);
    let captures = agents
        .iter()
        .map(|agent| {
            let conversation_id = agent.conversation.conversation_id.clone().unwrap();
            let archive = conversation_id.as_bytes().to_vec();
            SandboxCheckpointConversation::new(
                agent.id.clone(),
                &conversation_id,
                ConversationSnapshot {
                    conversation_id: conversation_id.clone(),
                    archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                    archive,
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    original.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoint-all".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &captures,
            IDLE_AT,
        )
        .unwrap(),
    );
    store
        .lock()
        .sandboxes
        .insert(original.id.clone(), original.clone());
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    for before in [first, second] {
        let after = store.get_agent(&before.id).await.unwrap().unwrap();
        assert_eq!(after.status, AgentStatus::Paused);
        assert_eq!(
            after.conversation.conversation_id,
            before.conversation.conversation_id
        );
        assert_eq!(after.worktree_id, before.worktree_id);
        assert_eq!(after.workspace_id, before.workspace_id);
        assert_eq!(after.title, before.title);
        assert_eq!(after.capabilities, before.capabilities);
    }
    assert_eq!(store.get_agent(&terminal.id).await.unwrap(), Some(terminal));
}

#[tokio::test]
async fn recovery_fails_attempted_first_message_without_inventing_write_confirmation() {
    for confirmed in [false, true] {
        let mut live = agent("agt_a", "conversation-a", 3);
        let mut first = FirstMessageRecord::new("launch", "first work", IDLE_AT).unwrap();
        first.fence_dispatch(2, 1, IDLE_AT.into()).unwrap();
        if confirmed {
            first.mark_write_confirmed(IDLE_AT.into()).unwrap();
        }
        live.status = AgentStatus::Working;
        live.conversation.active_turn = Some(2);
        live.conversation.first_message = Some(first.clone());
        let (store, original) = fixture(vec![live.clone()], true);
        store
            .recover_runtime_loss(loss_request(&original))
            .await
            .unwrap();
        let parked = store.get_agent(&live.id).await.unwrap().unwrap();
        let settled = parked.conversation.first_message.unwrap();
        assert_eq!(settled.state, FirstMessageDelivery::Failed);
        assert_eq!(settled.write_confirmed_at, first.write_confirmed_at);
        assert_eq!(settled.execution_observed_at, first.execution_observed_at);
        assert!(!settled.failure.as_ref().unwrap().retryable);
    }
}

#[tokio::test]
async fn lost_runtime_never_reopens_an_agent_with_prior_owner_stop_intent() {
    let mut live = agent("agt_a", "conversation-a", 1);
    live.request_stop(AgentStopReason::Owner, IDLE_AT.into())
        .unwrap();
    let (store, original) = fixture(vec![live.clone()], true);
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    let stopped = store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(stopped.requested_stop_reason, None);
    assert_eq!(stopped.work.step, AgentWorkStep::None);
}

#[tokio::test]
async fn replacement_loss_allocates_new_lifecycle_authority_and_preserves_rotation_target() {
    for rotating in [false, true] {
        for phase in [
            SandboxRevivePhase::CredentialDelivery,
            SandboxRevivePhase::HomeRestore,
            SandboxRevivePhase::ConversationReopen,
            SandboxRevivePhase::QueuedMessageDelivery,
            SandboxRevivePhase::PublishActive,
        ] {
            let live = agent("agt_a", "conversation-a", 1);
            let (store, mut original) = fixture(vec![live.clone()], true);
            if rotating {
                let identity = SandboxRotationIdentity::capture(&original, &[live]).unwrap();
                original
                    .begin_image_rotation("image-v2".into(), identity, IDLE_AT.into())
                    .unwrap();
                if let Some(SandboxLifecycleOperation::ImageRotation {
                    phase: current,
                    checkpoint_epoch,
                    ..
                }) = original.lifecycle.operation.as_mut()
                {
                    *checkpoint_epoch = Some(1);
                    *current = match phase {
                        SandboxRevivePhase::CredentialDelivery => {
                            SandboxImageRotationPhase::CredentialDelivery
                        }
                        SandboxRevivePhase::HomeRestore => SandboxImageRotationPhase::HomeRestore,
                        SandboxRevivePhase::ConversationReopen => {
                            SandboxImageRotationPhase::ConversationReopen
                        }
                        SandboxRevivePhase::QueuedMessageDelivery => {
                            SandboxImageRotationPhase::QueuedMessageDelivery
                        }
                        _ => SandboxImageRotationPhase::PublishActive,
                    };
                }
            } else {
                original.status = SandboxStatus::Paused;
                original.runtime = None;
                original
                    .begin_revive(SandboxReviveCause::ExplicitRequest, IDLE_AT.into())
                    .unwrap();
                if let Some(SandboxLifecycleOperation::Revive { phase: current, .. }) =
                    original.lifecycle.operation.as_mut()
                {
                    *current = phase;
                }
            }
            original.runtime = sandbox().runtime;
            original
                .work
                .claim("old-worker".into(), IDLE_AT, NOW.into())
                .unwrap();
            original.validate().unwrap();
            let authority = original
                .lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .clone();
            store
                .lock()
                .sandboxes
                .insert(original.id.clone(), original.clone());
            assert_eq!(
                store
                    .recover_runtime_loss(loss_request(&original))
                    .await
                    .unwrap(),
                SandboxRuntimeLossOutcome::RecoveryScheduled
            );
            let next = store.get_sandbox(&original.id).await.unwrap().unwrap();
            let operation = next.lifecycle.operation.unwrap();
            assert!(operation.authority().generation > authority.generation);
            assert_ne!(operation.authority().operation_id, authority.operation_id);
            assert!(next.runtime.is_none());
            assert!(next.work.lease.is_none());
            if rotating {
                assert!(
                    matches!(operation, SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::CheckpointVerification,
                    target_runtime_version, checkpoint_epoch: Some(1), ..
                } if target_runtime_version == "image-v2")
                );
                assert_eq!(next.lifecycle.rotation, original.lifecycle.rotation);
            } else {
                assert!(matches!(
                    operation,
                    SandboxLifecycleOperation::Revive {
                        phase: SandboxRevivePhase::CheckpointVerification,
                        cause: SandboxReviveCause::RuntimeLost,
                        ..
                    }
                ));
            }
        }
    }
}

#[tokio::test]
async fn runtime_loss_mid_pause_finishes_the_durable_pause_instead_of_reprovisioning() {
    for phase in [
        SandboxPausePhase::CaptureAndPublishCheckpoint,
        SandboxPausePhase::PauseAgents,
        SandboxPausePhase::TeardownRuntime,
    ] {
        let live = agent("agt_a", "conversation-a", 1);
        let (store, mut original) = fixture(vec![live.clone()], true);
        original
            .begin_pause(IDLE_AT.into(), Some("idle notice".into()))
            .unwrap();
        if let Some(SandboxLifecycleOperation::Pause {
            phase: current,
            checkpoint_epoch,
            ..
        }) = original.lifecycle.operation.as_mut()
        {
            *current = phase;
            *checkpoint_epoch =
                (phase != SandboxPausePhase::CaptureAndPublishCheckpoint).then_some(1);
        }
        original
            .work
            .claim("old-worker".into(), IDLE_AT, NOW.into())
            .unwrap();
        let old_authority = original
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .generation;
        store
            .lock()
            .sandboxes
            .insert(original.id.clone(), original.clone());
        store
            .recover_runtime_loss(loss_request(&original))
            .await
            .unwrap();
        let mut next = store.get_sandbox(&original.id).await.unwrap().unwrap();
        assert_eq!(next.status, SandboxStatus::Pausing);
        assert!(matches!(
            next.lifecycle.operation,
            Some(SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::PublishPaused,
                checkpoint_epoch: Some(1),
                ..
            })
        ));
        assert!(
            next.lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .generation
                > old_authority
        );
        assert_eq!(next.lifecycle.paused_notice.as_deref(), Some("idle notice"));
        assert_eq!(
            store.get_agent(&live.id).await.unwrap().unwrap().status,
            AgentStatus::Paused
        );
        let fence = next
            .work
            .claim("new-worker".into(), NOW, "2026-09-05T01:00:00Z".into())
            .unwrap();
        next.publish_lifecycle_paused("new-worker", fence, NOW.into())
            .unwrap();
        assert_eq!(next.status, SandboxStatus::Paused);
        assert_eq!(next.runtime, None);
    }
}

#[tokio::test]
async fn stale_or_mismatched_runtime_observations_never_mutate_newer_authority() {
    let live = agent("agt_a", "conversation-a", 1);
    let (store, original) = fixture(vec![live.clone()], true);
    for mismatch in 0..4 {
        let mut request = loss_request(&original);
        match mismatch {
            0 => request.expected_revision += 1,
            1 => request.expected_runtime.as_mut().unwrap().runtime_id = "replacement".into(),
            2 => request.expected_runtime.as_mut().unwrap().runtime_version = "image-v2".into(),
            _ => request.expected_runtime.as_mut().unwrap().bound_at = NOW.into(),
        }
        assert_eq!(
            store.recover_runtime_loss(request).await.unwrap(),
            SandboxRuntimeLossOutcome::Superseded
        );
        assert_eq!(
            store.get_sandbox(&original.id).await.unwrap(),
            Some(original.clone())
        );
        assert_eq!(store.get_agent(&live.id).await.unwrap(), Some(live.clone()));
    }
    assert!(store
        .sandbox_events(&original.id, 0, 100)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unbound_recovery_steps_are_intentional_but_active_without_binding_is_lost() {
    for active in [false, true] {
        let (store, mut original) = fixture(vec![], true);
        original.runtime = None;
        if !active {
            original.status = SandboxStatus::Paused;
            original
                .begin_revive(SandboxReviveCause::ExplicitRequest, IDLE_AT.into())
                .unwrap();
        }
        store
            .lock()
            .sandboxes
            .insert(original.id.clone(), original.clone());
        assert_eq!(
            store
                .recover_runtime_loss(loss_request(&original))
                .await
                .unwrap(),
            if active {
                SandboxRuntimeLossOutcome::RecoveryScheduled
            } else {
                SandboxRuntimeLossOutcome::NotEligible
            }
        );
    }
}

#[tokio::test]
async fn concurrent_recovery_claims_commit_all_state_once() {
    let live = agent("agt_a", "conversation-a", 1);
    let (store, original) = fixture(vec![live.clone()], true);
    let store = Arc::new(store);
    let request = loss_request(&original);
    let (left, right) = tokio::join!(
        store.recover_runtime_loss(request.clone()),
        store.recover_runtime_loss(request)
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == SandboxRuntimeLossOutcome::RecoveryScheduled)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == SandboxRuntimeLossOutcome::Superseded)
            .count(),
        1
    );
    assert_eq!(
        store
            .get_sandbox(&original.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        original.revision + 1
    );
    assert_eq!(
        store.get_agent(&live.id).await.unwrap().unwrap().revision,
        live.revision + 1
    );
}

#[tokio::test]
async fn corrupt_evidence_rolls_back_sandbox_all_agents_logs_and_events() {
    for corruption in 0..5 {
        let live = agent("agt_a", "conversation-a", 1);
        let second = agent("agt_b", "conversation-b", 1);
        let (store, original) = fixture(vec![live.clone(), second.clone()], true);
        {
            let mut state = store.lock();
            match corruption {
                0 => state.agent_logs.get_mut(&second.id).unwrap()[0].seq = 2,
                1 => {
                    state
                        .agents
                        .get_mut(&second.id)
                        .unwrap()
                        .conversation
                        .last_seq = 2
                }
                2 => {
                    state.agent_logs.get_mut(&second.id).unwrap()[0].agent_id =
                        AgentId("foreign".into())
                }
                3 => state
                    .sandbox_events
                    .entry(original.id.clone())
                    .or_default()
                    .push(SandboxEvent {
                        sandbox_id: original.id.clone(),
                        seq: 2,
                        appended_at: IDLE_AT.into(),
                        kind: SandboxEventKind::Created,
                    }),
                _ => state.agents.get_mut(&second.id).unwrap().owner = SubjectId("foreign".into()),
            }
        }
        let before = store.lock().clone();
        assert!(store
            .recover_runtime_loss(loss_request(&original))
            .await
            .is_err());
        let after = store.lock().clone();
        assert_eq!(after.sandboxes, before.sandboxes);
        assert_eq!(after.agents, before.agents);
        assert_eq!(after.agent_logs, before.agent_logs);
        assert_eq!(after.sandbox_events, before.sandbox_events);
    }
}

#[tokio::test]
async fn retired_runtime_generation_rejects_delayed_stream_updates_before_reopen() {
    let live = agent("agt_a", "conversation-a", 1);
    let (store, original) = fixture(vec![live.clone()], true);
    let store = Arc::new(store);
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    let before = store.agent_log(&live.id, 0, 100).await.unwrap();
    let result = recorder
        .record_update(
            &live.id,
            4,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                    "late output",
                )),
            )),
            NOW.into(),
        )
        .await;
    assert_eq!(result, Err(AgentConversationError::StaleGeneration));
    assert_eq!(store.agent_log(&live.id, 0, 100).await.unwrap(), before);
}

#[tokio::test]
async fn runtime_loss_keeps_public_transcript_and_usage_readable_before_reopen() {
    use crate::sandbox::agent_reads_fold::fold_agent_log;
    let mut live = agent("agt_a", "conversation-a", 6);
    live.status = AgentStatus::Working;
    live.conversation.active_turn = Some(3);
    let (store, original) = fixture(vec![live.clone()], true);
    let kinds = vec![
        AgentLogKind::Created {
            workspace_id: live.workspace_id.clone(),
            worktree_id: live.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-a".into(),
            runtime_generation: 4,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Keep my work visible".into(),
        },
        AgentLogKind::AgentMessage {
            content: "The completed portion".into(),
        },
        AgentLogKind::Usage {
            input_tokens: 12,
            output_tokens: 5,
            context_used: 7,
            context_window: None,
        },
    ];
    let logs = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: live.id.clone(),
            seq: u64::try_from(index).unwrap() + 1,
            appended_at: IDLE_AT.into(),
            kind,
        })
        .collect::<Vec<_>>();
    let before = fold_agent_log(&live, &logs).unwrap();
    store.lock().agent_logs.insert(live.id.clone(), logs);
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    let parked = store.get_agent(&live.id).await.unwrap().unwrap();
    let mut logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    let after = fold_agent_log(&parked, &logs).unwrap();
    assert_eq!(after.usage.usage, before.usage.usage);
    assert_eq!(
        after.transcript.turns[0].owner_message,
        "Keep my work visible"
    );
    assert_eq!(
        after.transcript.turns[0].agent_messages,
        vec!["The completed portion"]
    );
    assert!(matches!(
        after.transcript.turns[0].outcome,
        AgentTurnOutcome::Interrupted { .. }
    ));
    let marker = logs
        .iter_mut()
        .find(|entry| matches!(entry.kind, AgentLogKind::RuntimeGenerationRetired { .. }))
        .unwrap();
    marker.kind = AgentLogKind::RuntimeGenerationRetired {
        previous_generation: 3,
        runtime_generation: 5,
    };
    assert!(fold_agent_log(&parked, &logs).is_err());
}

#[tokio::test]
async fn unmatched_agent_keeps_never_attempted_owner_input_readable_without_scheduling_it() {
    use crate::sandbox::agent_reads_fold::fold_agent_log;
    let mut live = agent("agt_a", "conversation-a", 3);
    live.conversation.queued_messages =
        vec![QueuedAgentMessage::new(3, "Queued owner work".into(), IDLE_AT.into()).unwrap()];
    let (store, original) = fixture(vec![live.clone()], false);
    let kinds = vec![
        AgentLogKind::Created {
            workspace_id: live.workspace_id.clone(),
            worktree_id: live.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-a".into(),
            runtime_generation: 4,
        },
        AgentLogKind::OwnerMessage {
            content: "Queued owner work".into(),
        },
    ];
    let logs = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: live.id.clone(),
            seq: u64::try_from(index).unwrap() + 1,
            appended_at: IDLE_AT.into(),
            kind,
        })
        .collect::<Vec<_>>();
    store.lock().agent_logs.insert(live.id.clone(), logs);
    store
        .recover_runtime_loss(loss_request(&original))
        .await
        .unwrap();
    let stopped = store.get_agent(&live.id).await.unwrap().unwrap();
    let logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.work.step, AgentWorkStep::None);
    assert_eq!(
        stopped.conversation.queued_messages,
        live.conversation.queued_messages
    );
    fold_agent_log(&stopped, &logs).unwrap();
}

fn fixture(agents: Vec<Agent>, with_checkpoint: bool) -> (InMemorySandboxStore, Sandbox) {
    let store = InMemorySandboxStore::default();
    let mut value = sandbox();
    if with_checkpoint {
        value.lifecycle.checkpoints.latest = Some(checkpoint(1, IDLE_AT));
    }
    {
        let mut state = store.lock();
        state.sandboxes.insert(value.id.clone(), value.clone());
        for agent in agents {
            let logs = (1..=agent.conversation.last_seq)
                .map(|seq| AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: IDLE_AT.into(),
                    kind: AgentLogKind::OwnerMessage {
                        content: format!("durable owner message {seq}"),
                    },
                })
                .collect();
            state.agent_logs.insert(agent.id.clone(), logs);
            state.agents.insert(agent.id.clone(), agent);
        }
    }
    (store, value)
}

fn loss_request(value: &Sandbox) -> SandboxRuntimeLossRequest {
    SandboxRuntimeLossRequest {
        sandbox_id: value.id.clone(),
        expected_revision: value.revision,
        expected_runtime: value.runtime.clone(),
        observed_at: NOW.into(),
    }
}
