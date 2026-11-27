use super::*;
use layer2_core::sandbox::store::WorkspaceAdmissionStore;
use layer2_core::sandbox::{SandboxCheckpointConversation, SandboxCheckpointRef};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};
mod public_api;

async fn completed_retirement(harness: &Harness, remove_runtime: bool, released_home: bool) {
    let mut parent = harness
        .store
        .get_sandbox(&harness.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let expected = parent.revision;
    let stamp = rfc3339_at(harness.now + Duration::from_secs(10));
    if remove_runtime {
        harness.adapter.teardown(&harness.runtime).await.unwrap();
    }
    parent.begin_retirement(stamp.clone()).unwrap();
    harness
        .store
        .save_sandbox(expected, parent.clone())
        .await
        .unwrap();
    let expected = parent.revision;
    parent.runtime = None;
    if released_home {
        let mut home = layer2_core::sandbox::SandboxHome::reserve(
            &parent.id,
            layer3_adapter::HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "fixture".into(),
                capacity_gib: 64,
                template_name: "fixture-seed".into(),
                template_generation: "generation-one".into(),
            },
        )
        .unwrap();
        home.released = true;
        parent.lifecycle.home = Some(Box::new(home));
    }
    parent.credential.delivered = false;
    parent.credential.current = None;
    parent
        .transition(SandboxStatus::Retired, stamp.clone(), None)
        .unwrap();
    parent
        .work
        .schedule(layer2_core::sandbox::SandboxWorkStep::WaitForPurge, stamp);
    assert_eq!(parent.validate(), Ok(()));
    harness.store.save_sandbox(expected, parent).await.unwrap();
}

#[tokio::test]
async fn retired_sandbox_agent_delete_removes_records_without_recreating_runtime_or_fabricating_receipts(
) {
    for (remove_runtime, retained_turn) in [(true, false), (true, true), (false, true)] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let admitted = harness.admit("retired-delete", Vec::new()).await;
        let ready = harness.drive_ready(&admitted.id).await;
        harness
            .store
            .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                target: AgentControlTarget::from(&ready),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
            })
            .await
            .unwrap();
        for _ in 0..3 {
            assert!(harness
                .worker
                .run_one(&ready.id, harness.now + Duration::from_secs(5))
                .await
                .unwrap());
        }
        let mut stopped = harness.stored(&ready.id).await;
        assert_eq!(stopped.status, AgentStatus::Stopped);
        if retained_turn {
            let revision = stopped.revision;
            stopped.conversation.active_turn = Some(9);
            stopped.revision += 1;
            harness
                .store
                .save_agent(revision, stopped.clone())
                .await
                .unwrap();
        }
        completed_retirement(&harness, remove_runtime, false).await;
        let before = harness.backend.requests().await.len();
        harness
            .store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&stopped),
                requested_at: rfc3339_at(harness.now + Duration::from_secs(11)),
            })
            .await
            .unwrap();
        for _ in 0..4 {
            harness
                .worker
                .run_one(&ready.id, harness.now + Duration::from_secs(11))
                .await
                .unwrap();
        }
        let deleted = harness.stored(&ready.id).await;
        assert_eq!(
            deleted.status,
            if remove_runtime {
                AgentStatus::Deleted
            } else {
                AgentStatus::Stopped
            }
        );
        assert_eq!(deleted.work.receipts, stopped.work.receipts);
        assert_eq!(
            deleted.conversation.active_turn,
            stopped.conversation.active_turn
        );
        assert_eq!(
            deleted.conversation.conversation_id,
            stopped.conversation.conversation_id
        );
        assert_eq!(harness.backend.requests().await.len(), before);
        let worktree = harness
            .store
            .get_worktree(&ready.worktree_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            worktree.status,
            if remove_runtime {
                WorktreeStatus::Removed
            } else {
                WorktreeStatus::Ready
            }
        );
    }
}

#[tokio::test]
async fn retired_home_settles_failed_worktree_debt_only_after_observing_runtime_absence() {
    for (remove_runtime, released_home, wrong_evidence) in [
        (true, true, None),
        (false, true, None),
        (true, false, None),
        (true, true, Some("operation")),
        (true, true, Some("digest")),
    ] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let admitted = harness.admit("retired-first-worktree", Vec::new()).await;
        harness.backend.fail_next(
            WireOperation::AddWorktree,
            "runtime_mutation_outcome_unknown",
            "worktree addition left partial Git state",
        );
        harness.run(&admitted.id, harness.now).await;
        let mut failed = harness.stored(&admitted.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        assert!(
            matches!(failed.work.cleanup_debt.as_slice(), [debt] if debt.step == AgentWorkStep::AddWorktree)
        );
        if let Some(field) = wrong_evidence {
            let debt = &mut failed.work.cleanup_debt[0];
            match field {
                "operation" => debt.operation_id = "worktree:project:wt-2:add:v1".into(),
                "digest" => debt.request_sha256 = "b".repeat(64),
                _ => unreachable!(),
            }
            let revision = failed.revision;
            failed.revision += 1;
            harness
                .store
                .save_agent(revision, failed.clone())
                .await
                .unwrap();
        }
        completed_retirement(&harness, remove_runtime, released_home).await;
        let before = harness.backend.requests().await.len();
        let at = harness.now + Duration::from_secs(11);
        harness
            .store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&failed),
                requested_at: rfc3339_at(at),
            })
            .await
            .unwrap();
        for _ in 0..4 {
            harness.worker.run_one(&failed.id, at).await.unwrap();
        }
        let agent = harness.stored(&failed.id).await;
        let worktree = harness
            .store
            .get_worktree(&failed.worktree_id)
            .await
            .unwrap()
            .unwrap();
        if remove_runtime && released_home && wrong_evidence.is_none() {
            assert_eq!(agent.status, AgentStatus::Deleted);
            assert_eq!(worktree.status, WorktreeStatus::Removed);
            assert!(agent.work.cleanup_debt.is_empty());
        } else {
            assert_eq!(agent.status, AgentStatus::Failed);
            assert_eq!(worktree.status, WorktreeStatus::Failed);
            assert_eq!(agent.work.cleanup_debt, failed.work.cleanup_debt);
        }
        assert_eq!(agent.work.receipts, failed.work.receipts);
        assert_eq!(harness.backend.requests().await.len(), before);
    }
}

#[tokio::test]
async fn retired_sandbox_workspace_removal_requires_provider_absence_and_finishes_pending_work() {
    use crate::sandbox::workspace_worker::{
        WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
    };
    for remove_runtime in [true, false] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        completed_retirement(&harness, remove_runtime, false).await;
        harness
            .store
            .begin_workspace_removal(
                &harness.workspace.id,
                &harness.workspace.owner,
                rfc3339_at(harness.now + Duration::from_secs(11)),
            )
            .await
            .unwrap();
        let before = harness.backend.requests().await.len();
        let worker = WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: harness.store.clone(),
                adapter: harness.adapter.clone(),
            },
            WorkspaceWorkerConfig::new("retired-cleanup", DEPLOYMENT_ID),
        );
        worker
            .run_one(&harness.workspace.id, harness.now + Duration::from_secs(11))
            .await
            .unwrap();
        let workspace = harness
            .store
            .get_workspace(&harness.workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace.status,
            if remove_runtime {
                WorkspaceStatus::Removed
            } else {
                WorkspaceStatus::Removing
            }
        );
        assert_eq!(harness.backend.requests().await.len(), before);
    }
}

async fn retire_paused_parent(harness: &Harness, ready: &Agent, remove_runtime: bool) {
    retire_paused_parent_with_pending_stop(harness, ready, remove_runtime, None).await;
}

async fn retire_paused_parent_with_pending_stop(
    harness: &Harness,
    ready: &Agent,
    remove_runtime: bool,
    reason: Option<AgentStopReason>,
) {
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
    if let Some(reason) = reason {
        let paused = harness.stored(&ready.id).await;
        harness
            .store
            .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                target: AgentControlTarget::from(&paused),
                reason,
                requested_at: stamp.clone(),
            })
            .await
            .unwrap();
    }
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
async fn owner_stop_before_retirement_survives_worker_restart_and_preserves_original_reason() {
    use crate::sandbox::worker::LiveAgentStopper;
    let harness = Harness::with_generation(Arc::new(EmptyAgentSkillResolver), 4).await;
    let admitted = harness.admit("pending-owner-stop", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    retire_paused_parent_with_pending_stop(&harness, &ready, true, Some(AgentStopReason::Owner))
        .await;
    let parent = harness
        .store
        .get_sandbox(&ready.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let stopper = crate::sandbox::live_agent_stopper::DurableLiveAgentStopper::new(
        harness.store.clone(),
        harness.registry.clone(),
    );
    for _ in 0..2 {
        assert!(stopper
            .stop_live_agents(&parent, None, "retirement")
            .await
            .is_err());
        assert_eq!(
            harness.stored(&ready.id).await.requested_stop_reason,
            Some(AgentStopReason::Owner)
        );
    }
    let now = harness.now + Duration::from_secs(6);
    assert!(harness.worker.run_one(&ready.id, now).await.unwrap());
    assert_eq!(
        harness.stored(&ready.id).await.work.step,
        AgentWorkStep::CloseProcess
    );
    let before = harness.backend.requests().await.len();
    let registry = Arc::new(LiveAgentRegistry::default());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), registry),
        },
        AgentWorkerConfig::new("replacement-worker", DEPLOYMENT_ID),
    );
    for _ in 0..2 {
        assert!(worker.run_one(&ready.id, now).await.unwrap());
    }
    let stopped = harness.stored(&ready.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(
        stopped.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert_eq!(stopped.work.receipts, ready.work.receipts);
    assert_eq!(harness.backend.requests().await.len(), before);
    assert!(harness
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    stopper
        .stop_live_agents(&parent, None, "retirement")
        .await
        .unwrap();
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
        assert!(harness
            .worker
            .run_one(&ready.id, harness.now + Duration::from_secs(6))
            .await
            .unwrap());
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
        assert!(harness
            .worker
            .run_one(&ready.id, harness.now + Duration::from_secs(6))
            .await
            .unwrap());
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
        assert!(harness
            .worker
            .run_one(&ready.id, harness.now + Duration::from_secs(6))
            .await
            .unwrap());
    }
    let pending = harness.stored(&ready.id).await;
    assert_eq!(pending.status, AgentStatus::Paused);
    assert_eq!(pending.work.step, AgentWorkStep::CloseProcess);
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(pending.work.receipts, ready.work.receipts);
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
}
