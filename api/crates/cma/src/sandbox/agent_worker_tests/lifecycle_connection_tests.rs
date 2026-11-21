use super::*;

mod maintenance;

use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome, SandboxLifecycleAgentScope,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard,
    SandboxWorkerStore,
};
use layer2_core::sandbox::{
    AgentOperationIntent, AgentOperationRecord, SandboxCheckpointConversation,
    SandboxCheckpointRef, SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxReviveCause, SandboxRevivePhase, SandboxRotationIdentity,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

async fn completed_agent(harness: &Harness) -> Agent {
    let admitted = harness.admit("restored-connection", Vec::new()).await;
    let mut ready = harness.drive_ready(&admitted.id).await;
    let revision = ready.revision;
    let mut operation = AgentOperationRecord::new(
        1,
        ready.conversation.runtime_generation.unwrap(),
        2,
        AgentOperationIntent::send_message("already completed"),
        rfc3339_at(harness.now),
    )
    .unwrap();
    operation
        .mark_execution_observed(rfc3339_at(harness.now))
        .unwrap();
    ready.conversation.operation = Some(operation);
    ready.revision += 1;
    harness
        .store
        .save_agent(revision, ready.clone())
        .await
        .unwrap();
    ready
}

async fn restore_phase(harness: &Harness, agent: &Agent, rotation: bool, publish: bool) {
    let current = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let runtime = current.runtime.clone().unwrap();
    let stamp = rfc3339_at(harness.now);
    let archive = b"recorded-conversation".to_vec();
    let conversation = SandboxCheckpointConversation::new(
        agent.id.clone(),
        agent.conversation.conversation_id.clone().unwrap(),
        ConversationSnapshot {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    let checkpoint = SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoint/restored-connection".into(),
            sha256: "e".repeat(64),
            writers: WriterState::Resumed,
        },
        &[conversation],
        stamp.clone(),
    )
    .unwrap();
    let mut next = current.clone();
    next.lifecycle.checkpoints.publish(checkpoint).unwrap();
    if rotation {
        next.runtime.as_mut().unwrap().runtime_version = "registry.example/runtime:previous".into();
        let identity =
            SandboxRotationIdentity::capture(&next, std::slice::from_ref(agent)).unwrap();
        next.begin_image_rotation(runtime.runtime_version.clone(), identity, stamp.clone())
            .unwrap();
        let Some(SandboxLifecycleOperation::ImageRotation {
            phase,
            checkpoint_epoch,
            ..
        }) = next.lifecycle.operation.as_mut()
        else {
            unreachable!()
        };
        *phase = if publish {
            SandboxImageRotationPhase::PublishActive
        } else {
            SandboxImageRotationPhase::QueuedMessageDelivery
        };
        *checkpoint_epoch = Some(1);
    } else {
        next.status = SandboxStatus::Paused;
        next.runtime = None;
        next.work.schedule(SandboxWorkStep::None, stamp.clone());
        next.begin_revive(SandboxReviveCause::TriggerLaunch, stamp)
            .unwrap();
        let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
            next.lifecycle.operation.as_mut()
        else {
            unreachable!()
        };
        *phase = if publish {
            SandboxRevivePhase::PublishActive
        } else {
            SandboxRevivePhase::QueuedMessageDelivery
        };
    }
    next.runtime = Some(runtime);
    next.validate().unwrap();
    harness
        .store
        .save_sandbox(current.revision, next)
        .await
        .unwrap();
}

fn final_publication(current: &Sandbox, now: SystemTime) -> SandboxLifecycleMutation {
    let lease = current.work.lease.as_ref().unwrap();
    let mut next = current.clone();
    next.publish_lifecycle_active(
        rfc3339_at(now + Duration::from_secs(60)),
        &lease.holder,
        lease.fence,
        rfc3339_at(now),
    )
    .unwrap();
    let event = match current.lifecycle.operation.as_ref().unwrap() {
        SandboxLifecycleOperation::Revive { .. } => SandboxEventKind::Revived,
        SandboxLifecycleOperation::ImageRotation {
            target_runtime_version,
            ..
        } => SandboxEventKind::Rotated {
            runtime_version: target_runtime_version.clone(),
        },
        _ => unreachable!(),
    };
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox: next,
        agents: Vec::new(),
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: rfc3339_at(now),
        events: vec![
            SandboxEventKind::StatusChanged {
                from: current.status,
                to: SandboxStatus::Active,
            },
            event,
        ],
    }
}

#[tokio::test]
async fn persisted_connection_retry_settles_before_revive_or_rotation_publication() {
    for rotation in [false, true] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let ready = completed_agent(&harness).await;
        restore_phase(&harness, &ready, rotation, true).await;
        let request = AgentConnectionRecoveryCandidate::from_agent(&ready)
            .unwrap()
            .restart_request(rfc3339_at(harness.now));
        assert_eq!(
            harness
                .store
                .schedule_agent_connection_recovery(request)
                .await
                .unwrap(),
            AgentConnectionRecoveryOutcome::Scheduled
        );
        let mut pending = harness.stored(&ready.id).await;
        let revision = pending.revision;
        pending.work.attempts = 7;
        pending.revision += 1;
        harness.store.save_agent(revision, pending).await.unwrap();

        let current = harness
            .store
            .claim_sandbox_work(
                &ready.sandbox_id,
                "restore-worker",
                rfc3339_at(harness.now),
                rfc3339_at(harness.now + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(harness
            .store
            .commit_sandbox_lifecycle(final_publication(&current, harness.now))
            .await
            .is_err());

        harness.run(&ready.id, harness.now).await;
        let recovered = harness.stored(&ready.id).await;
        assert_eq!(recovered.work.step, AgentWorkStep::None);
        assert!(recovered.work.lease.is_none());
        assert_eq!(recovered.status, AgentStatus::Ready);
        let mut expected_conversation = ready.conversation.clone();
        expected_conversation.last_seq = recovered.conversation.last_seq;
        assert_eq!(recovered.conversation, expected_conversation);
        assert!(harness.registry.resolve(&Harness::target(&ready)).is_some());
        harness
            .store
            .commit_sandbox_lifecycle(final_publication(&current, harness.now))
            .await
            .unwrap();
        assert_eq!(
            harness
                .store
                .get_sandbox(&ready.sandbox_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            SandboxStatus::Active
        );
    }
}

#[tokio::test]
async fn reconciliation_after_reopen_settles_in_each_remaining_restore_phase() {
    for rotation in [false, true] {
        for publish in [false, true] {
            let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
            let ready = completed_agent(&harness).await;
            restore_phase(&harness, &ready, rotation, publish).await;
            harness.registry.fence(
                &ready.id,
                ready.conversation.runtime_generation,
                ready.work.next_fence - 1,
            );
            assert_eq!(
                harness
                    .worker
                    .reconcile_connections(harness.now)
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(
                harness.stored(&ready.id).await.work.step,
                AgentWorkStep::PublishConnection
            );
            harness.run(&ready.id, harness.now).await;
            assert_eq!(
                harness.stored(&ready.id).await.work.step,
                AgentWorkStep::None
            );
            assert!(harness.registry.resolve(&Harness::target(&ready)).is_some());
        }
    }
}

#[tokio::test]
async fn restored_connection_does_not_admit_unrestored_or_foreign_identity() {
    for invalid in [
        "generation",
        "conversation",
        "member",
        "phase",
        "dispatch",
        "credential",
    ] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let ready = completed_agent(&harness).await;
        let mut checkpoint_agent = ready.clone();
        if invalid == "member" {
            checkpoint_agent.id = AgentId::generate();
        }
        restore_phase(&harness, &checkpoint_agent, false, true).await;
        let mut agent = ready.clone();
        agent
            .work
            .schedule(AgentWorkStep::PublishConnection, rfc3339_at(harness.now));
        match invalid {
            "generation" => agent.conversation.runtime_generation = Some(2),
            "conversation" => {
                agent.conversation.conversation_id = Some("different-conversation".into())
            }
            "member" => {}
            "dispatch" => agent.work.step = AgentWorkStep::DispatchFirstMessage,
            "phase" | "credential" => {
                let current = harness
                    .store
                    .get_sandbox(&ready.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let mut next = current.clone();
                if invalid == "phase" {
                    let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
                        next.lifecycle.operation.as_mut()
                    else {
                        unreachable!()
                    };
                    *phase = SandboxRevivePhase::ConversationReopen;
                } else {
                    next.credential.delivered = false;
                }
                next.revision += 1;
                harness
                    .store
                    .save_sandbox(current.revision, next)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let pending_step = agent.work.step;
        agent.revision += 1;
        harness
            .store
            .save_agent(ready.revision, agent)
            .await
            .unwrap();
        harness.registry.fence(
            &ready.id,
            ready.conversation.runtime_generation,
            ready.work.next_fence - 1,
        );
        harness.run(&ready.id, harness.now).await;
        let pending = harness.stored(&ready.id).await;
        assert_eq!(pending.work.step, pending_step, "{invalid}");
        assert_eq!(pending.work.attempts, 1, "{invalid}");
        assert!(pending.work.lease.is_none());
    }
}

#[tokio::test]
async fn connection_recovery_still_waits_for_checkpoint_writer_resumption() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let ready = completed_agent(&harness).await;
    let capture_at = harness.now + Duration::from_secs(120);
    let current = harness
        .store
        .get_sandbox(&ready.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut capturing = current.clone();
    capturing
        .begin_checkpoint_capture(rfc3339_at(capture_at))
        .unwrap();
    harness
        .store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&current),
            sandbox: capturing,
            agents: Vec::new(),
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: rfc3339_at(capture_at),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
    harness.registry.fence(
        &ready.id,
        ready.conversation.runtime_generation,
        ready.work.next_fence - 1,
    );
    assert_eq!(
        harness
            .worker
            .reconcile_connections(capture_at)
            .await
            .unwrap(),
        1
    );
    assert!(!harness.worker.run_one(&ready.id, capture_at).await.unwrap());
    let retry = harness.stored(&ready.id).await;
    assert_eq!(retry.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(retry.work.attempts, 0);
    assert!(retry.work.lease.is_none());
    assert!(harness.registry.resolve(&Harness::target(&ready)).is_none());
}
