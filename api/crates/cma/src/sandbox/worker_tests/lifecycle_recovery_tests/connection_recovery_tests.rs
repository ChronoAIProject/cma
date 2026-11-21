use super::*;

use crate::sandbox::agent_skills::{AgentSkillResolutionError, AgentSkillResolver};

use crate::sandbox::agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies};
use layer2_core::sandbox::store::WorkspaceStore;
use layer2_core::sandbox::{AgentWorkReceipt, SandboxLifecycleOperation, SandboxRevivePhase};

struct RecoverySkills;
#[async_trait::async_trait]
impl AgentSkillResolver for RecoverySkills {
    async fn resolve(
        &self,
        _: &Sandbox,
        refs: &[layer2_core::profiles::SkillRef],
    ) -> Result<Vec<layer2_core::sandbox::ResolvedAgentSkill>, AgentSkillResolutionError> {
        Ok(refs
            .iter()
            .map(|reference| layer2_core::sandbox::ResolvedAgentSkill {
                source: reference.source,
                id: reference.id.clone(),
                name: None,
                version: reference.version.clone().unwrap(),
                archive_sha256: "a".repeat(64),
            })
            .collect())
    }
    fn runtime_skills(
        &self,
        material: &[layer2_core::sandbox::ResolvedAgentSkill],
    ) -> Result<Vec<layer3_adapter::AgentSkill>, AgentSkillResolutionError> {
        Ok(material
            .iter()
            .map(|skill| layer3_adapter::AgentSkill {
                id: skill.id.to_string(),
                name: skill.name.clone(),
                version: Some(skill.version.clone()),
                archive_sha256: skill.archive_sha256.clone(),
                source_url: "https://platform.example/skill.zip".into(),
            })
            .collect())
    }
}

#[test]
fn archived_revival_reconnects_on_another_replica_before_publishing_active() {
    run_recovery_scenario(false, false, false);
}

#[test]
fn retained_home_recovery_reconnects_without_an_archived_checkpoint() {
    run_recovery_scenario(true, false, false);
}

#[test]
fn runtime_loss_during_revival_retries_then_recovers_the_retained_conversation() {
    run_recovery_scenario(true, true, false);
}

#[test]
fn live_skill_pending_install_resumes_before_retained_revival_publishes_active() {
    run_recovery_scenario(true, false, true);
}

// This fixture drives both independent workers through recovery. Keep the large
// debug-only combined future off Rust's small default test-thread stack.
fn run_recovery_scenario(retained: bool, interrupt_revival: bool, pending_skill: bool) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(recovery_scenario(
                    retained,
                    interrupt_revival,
                    pending_skill,
                ));
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn recovery_scenario(retained: bool, interrupt_revival: bool, pending_skill: bool) {
    let mut harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let (worker, adapter) = if retained {
        let homes = home_tests::Homes::new(&harness);
        (
            home_tests::worker(&harness, homes.clone(), "recovery-worker"),
            home_tests::adapter(&harness, homes),
        )
    } else {
        (harness.worker.clone(), harness.adapter.clone())
    };
    harness.adapter = adapter.clone();
    for _ in 0..8 {
        if harness.stored(&created.id).await.status == SandboxStatus::Active {
            break;
        }
        Box::pin(worker.run_one(&created.id, now)).await.unwrap();
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    crate::sandbox::runtime_attachment::adopt_bound_runtime(adapter.as_ref(), &active, "test")
        .await
        .unwrap();
    let mut agent = prepare_agent(&harness, &active, now, 1).await;
    harness
        .store
        .create_workspace(creation_revival::workspace_for(&agent, now))
        .await
        .unwrap();
    let worktree = harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let open = AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        runtime_options(&agent, &active, &worktree),
        None,
    );
    let revision = agent.revision;
    agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: open.operation_id,
        request_sha256: open.request_sha256,
        completed_at: rfc3339_at(now),
    });
    agent.revision += 1;
    harness
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    if pending_skill {
        agent = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
            .install_skill(
                &caller("owner", [Capability::UseOwnSandboxes]),
                &agent.id,
                layer2_core::profiles::SkillRef {
                    source: layer2_core::profiles::SkillSource::Ornn,
                    id: layer2_core::SkillId::new("research"),
                    version: Some("1.0".into()),
                },
            )
            .await
            .unwrap();
    }
    let recovery_at = after_durable_boundary(&active).await;
    let reconciler = SandboxReconciler::new(
        harness.store.clone(),
        adapter.clone(),
        Arc::new(harness.policy.clone()),
        "test".into(),
    );
    if retained {
        assert!(active.lifecycle.checkpoints.latest.is_none());
        harness
            .backend
            .restart_persistent_process(&RuntimeHandle(
                active.runtime.as_ref().unwrap().runtime_id.clone(),
            ))
            .await
            .unwrap();
        reconciler.initialize(recovery_at).await.unwrap();
        let retired = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
        assert!(!retired
            .work
            .receipts
            .iter()
            .any(|receipt| receipt.step == AgentWorkStep::OpenConversation));
    } else {
        harness
            .store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                active.id.clone(),
                active.owner.clone(),
                rfc3339_at(recovery_at),
                None,
            ))
            .await
            .unwrap();
        for _ in 0..4 {
            Box::pin(worker.run_one(&active.id, recovery_at))
                .await
                .unwrap();
        }
        assert_eq!(
            harness.stored(&active.id).await.status,
            SandboxStatus::Paused
        );
        harness
            .store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                active.id.clone(),
                active.owner.clone(),
                rfc3339_at(recovery_at),
                SandboxReviveCause::ExplicitRequest,
            ))
            .await
            .unwrap();
    }
    drive_to_reopened(&harness, &worker, &active.id, recovery_at).await;
    let settle_at = recovery_at + Duration::from_secs(31);
    if interrupt_revival {
        let reopened = harness.stored(&active.id).await;
        let handle = RuntimeHandle(reopened.runtime.as_ref().unwrap().runtime_id.clone());
        // The lifecycle worker sees loss before the reconciler gets its next turn.
        crate::sandbox::runtime_attachment::adopt_bound_runtime(
            adapter.as_ref(),
            &reopened,
            "test",
        )
        .await
        .unwrap();
        adapter.teardown(&handle).await.unwrap();
        Box::pin(worker.run_one(&active.id, recovery_at))
            .await
            .unwrap();
        let waiting = harness.stored(&active.id).await;
        assert_eq!(waiting.status, SandboxStatus::Reviving);
        assert!(waiting.failure.is_none());
        assert_eq!(waiting.runtime, reopened.runtime);
        reconciler.initialize(SystemTime::now()).await.unwrap();
        drive_to_reopened(&harness, &worker, &active.id, settle_at).await;
    }

    // A fresh replica has no in-memory attachment and must use the durable receipt.
    let registry = Arc::new(LiveAgentRegistry::default());
    let agent_worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            skills: Arc::new(RecoverySkills),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), registry.clone()),
            registry,
        },
        AgentWorkerConfig::new("reconnected-agent-worker", "test"),
    );
    if pending_skill {
        let reopened = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(reopened.work.step, AgentWorkStep::InstallSkill);
        assert_eq!(
            reopened.work.live_skills[0].status,
            layer2_core::sandbox::AgentSkillInstallStatus::Pending
        );
    } else {
        assert_eq!(
            agent_worker.reconcile_connections(settle_at).await.unwrap(),
            1
        );
    }
    assert!(Box::pin(agent_worker.run_one(&agent.id, settle_at))
        .await
        .unwrap());
    let reconnected = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        reconnected.work.step,
        AgentWorkStep::None,
        "{:?}",
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap()
    );
    if pending_skill {
        assert_eq!(
            reconnected.work.live_skills[0].status,
            layer2_core::sandbox::AgentSkillInstallStatus::Installed
        );
        assert!(reconnected.work.live_skills[0].error.is_none());
    }
    assert_eq!(
        reconnected.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        reconnected
            .work
            .receipts
            .iter()
            .filter(|receipt| receipt.step == AgentWorkStep::OpenConversation)
            .count(),
        1
    );
    assert_eq!(
        reconnected
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
            .unwrap()
            .completed_at,
        rfc3339_at(if interrupt_revival {
            settle_at
        } else {
            recovery_at
        })
    );
    for _ in 0..2 {
        Box::pin(worker.run_one(&active.id, settle_at))
            .await
            .unwrap();
    }
    let restored = harness.stored(&active.id).await;
    assert_eq!(restored.status, SandboxStatus::Active);
    assert_eq!(restored.lifecycle.home, active.lifecycle.home);
    assert_eq!(reconnected.worktree_id, agent.worktree_id);
}

async fn after_durable_boundary(sandbox: &Sandbox) -> SystemTime {
    let durable = stamp_time(&sandbox.updated_at);
    if let Ok(delay) = durable
        .duration_since(SystemTime::now())
        .map(|delay| delay + Duration::from_millis(10))
    {
        tokio::time::sleep(delay).await;
    }
    SystemTime::now()
}

async fn drive_to_reopened(
    harness: &Harness,
    worker: &SandboxWorker,
    id: &SandboxId,
    now: SystemTime,
) {
    for _ in 0..8 {
        let current = harness.stored(id).await;
        if matches!(
            current.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::QueuedMessageDelivery,
                ..
            })
        ) {
            return;
        }
        assert!(
            Box::pin(worker.run_one(id, now)).await.unwrap(),
            "{current:?}"
        );
    }
    panic!(
        "recovery did not reopen conversations: {:?}",
        harness.stored(id).await
    );
}
