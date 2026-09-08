use super::*;
use layer2_core::sandbox::{SandboxCheckpointConversation, SandboxCheckpointRef};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

async fn retire_paused_parent(harness: &Harness, ready: &Agent, remove_runtime: bool) {
    let stamp = rfc3339_at(harness.now + Duration::from_secs(5));
    let mut paused = ready.clone();
    paused
        .transition(AgentStatus::Paused, stamp.clone(), None)
        .unwrap();
    harness
        .store
        .save_agent(ready.revision, paused)
        .await
        .unwrap();
    if remove_runtime {
        harness.adapter.teardown(&harness.runtime).await.unwrap();
    }
    let mut sandbox = harness
        .store
        .get_sandbox(&ready.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    let archive = b"retained conversation".to_vec();
    let conversation_id = ready.conversation.conversation_id.clone().unwrap();
    let conversation = SandboxCheckpointConversation::new(
        ready.id.clone(),
        conversation_id.clone(),
        ConversationSnapshot {
            conversation_id,
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    sandbox.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoint/paused".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &[conversation],
            stamp.clone(),
        )
        .unwrap(),
    );
    sandbox.runtime = None;
    sandbox.status = SandboxStatus::Paused;
    sandbox.begin_retirement(stamp).unwrap();
    harness.store.save_sandbox(revision, sandbox).await.unwrap();
}

#[tokio::test]
async fn retirement_after_pause_stops_agent_without_adopting_or_recreating_runtime() {
    let harness = Harness::with_generation(Arc::new(EmptyAgentSkillResolver), 4).await;
    let admitted = harness.admit("paused-retirement", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    retire_paused_parent(&harness, &ready, true).await;
    let paused = harness.stored(&ready.id).await;
    let receipt_count = paused.work.receipts.len();
    let before_requests = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&paused),
            reason: AgentStopReason::SandboxRetired,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(6)),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness
            .run(&ready.id, harness.now + Duration::from_secs(6))
            .await;
    }
    let stopped = harness.stored(&ready.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::SandboxRetired));
    assert_eq!(stopped.work.step, AgentWorkStep::None);
    assert_eq!(stopped.work.receipts.len(), receipt_count);
    assert_eq!(
        stopped.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert_eq!(
        stopped.conversation.runtime_generation,
        ready.conversation.runtime_generation
    );
    assert_eq!(harness.backend.requests().await.len(), before_requests);
    assert!(harness
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    assert!(harness.registry.resolve(&Harness::target(&ready)).is_none());
    let logs = harness.store.agent_log(&ready.id, 0, 40).await.unwrap();
    assert!(
        matches!(logs.last().map(|entry| &entry.kind), Some(AgentLogKind::Stopped { reason }) if reason == "sandbox_retired")
    );
}

#[tokio::test]
async fn retiring_agent_without_checkpoint_provenance_keeps_cleanup_pending() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("unproven-retirement", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    retire_paused_parent(&harness, &ready, true).await;
    let mut sandbox = harness
        .store
        .get_sandbox(&ready.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.lifecycle.checkpoints.latest = None;
    sandbox.revision += 1;
    harness.store.save_sandbox(revision, sandbox).await.unwrap();
    let paused = harness.stored(&ready.id).await;
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&paused),
            reason: AgentStopReason::SandboxRetired,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(6)),
        })
        .await
        .unwrap();
    for _ in 0..2 {
        harness
            .run(&ready.id, harness.now + Duration::from_secs(6))
            .await;
    }
    let pending = harness.stored(&ready.id).await;
    assert_eq!(pending.status, AgentStatus::Paused);
    assert_eq!(pending.work.step, AgentWorkStep::CloseProcess);
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(pending.work.receipts, ready.work.receipts);
}

#[tokio::test]
async fn retiring_agent_does_not_treat_a_missing_binding_as_provider_runtime_absence() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("runtime-still-present", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    retire_paused_parent(&harness, &ready, false).await;
    let paused = harness.stored(&ready.id).await;
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&paused),
            reason: AgentStopReason::SandboxRetired,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(6)),
        })
        .await
        .unwrap();
    for _ in 0..2 {
        harness
            .run(&ready.id, harness.now + Duration::from_secs(6))
            .await;
    }
    let pending = harness.stored(&ready.id).await;
    assert_eq!(pending.status, AgentStatus::Paused);
    assert_eq!(pending.work.step, AgentWorkStep::CloseProcess);
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(pending.work.receipts, ready.work.receipts);
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
}
