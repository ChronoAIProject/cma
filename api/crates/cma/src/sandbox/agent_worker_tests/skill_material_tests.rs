use super::*;
use layer2_core::sandbox::{AgentCleanupDebt, AgentWorkReceipt};

fn restarted(harness: &Harness, skills: Arc<dyn AgentSkillResolver>) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    let dependencies = AgentWorkerDependencies {
        store: harness.store.clone(),
        adapter: harness.adapter.clone(),
        skills,
        registry: registry.clone(),
        updates: harness.updates.clone(),
        operations: AgentOperationService::new(harness.store.clone(), registry),
    };
    let mut config = AgentWorkerConfig::new("restarted-worker", DEPLOYMENT_ID);
    config.lease_duration = Duration::from_secs(30);
    config.retry_delay = Duration::from_secs(1);
    config.open_timeout = Duration::from_secs(2);
    AgentWorker::new(dependencies, config)
}

pub(super) fn runtime_skill(version: &str, hash: char, locator: &str) -> AgentSkill {
    AgentSkill {
        id: "research".into(),
        name: None,
        version: Some(version.into()),
        archive_sha256: hash.to_string().repeat(64),
        source_url: format!("https://platform.example/{locator}"),
    }
}

#[tokio::test]
async fn selected_material_survives_failure_stop_and_delete_without_resolution() {
    for rejected in [false, true] {
        let skills = ScriptedSkills::new([Ok(vec![runtime_skill("7.0", 'a', "archive")])]);
        let harness = Harness::new(skills.clone()).await;
        let agent = harness
            .admit(
                "material-cleanup",
                vec![SkillRef {
                    source: SkillSource::Ornn,
                    id: SkillId::new("research"),
                    version: None,
                }],
            )
            .await;
        let terminal = if rejected {
            harness.run(&agent.id, harness.now).await;
            harness.backend.fail_next(
                WireOperation::MaterializeAgentHome,
                "request_rejected",
                "home was not created",
            );
            harness.run(&agent.id, harness.now).await;
            let failed = harness.stored(&agent.id).await;
            assert_eq!(failed.status, AgentStatus::Failed);
            failed
        } else {
            let ready = harness.drive_ready(&agent.id).await;
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
                    .run_one(&agent.id, harness.now + Duration::from_secs(5))
                    .await
                    .unwrap());
            }
            let stopped = harness.stored(&agent.id).await;
            assert_eq!(stopped.status, AgentStatus::Stopped);
            assert_eq!(stopped.work.resolved_skills, ready.work.resolved_skills);
            stopped
        };
        assert_eq!(
            terminal.work.resolved_skills.as_ref().unwrap()[0].version,
            "7.0"
        );
        harness
            .store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&terminal),
                requested_at: rfc3339_at(harness.now + Duration::from_secs(10)),
            })
            .await
            .unwrap();
        for _ in 0..4 {
            assert!(harness
                .worker
                .run_one(&agent.id, harness.now + Duration::from_secs(10))
                .await
                .unwrap());
        }
        let deleted = harness.stored(&agent.id).await;
        assert_eq!(deleted.status, AgentStatus::Deleted);
        assert_eq!(deleted.work.resolved_skills, terminal.work.resolved_skills);
        assert_eq!(deleted.profile, agent.profile);
        assert_eq!(skills.calls.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test]
async fn restarted_home_reuses_material_when_alias_changes_or_metadata_disappears() {
    for unavailable in [false, true] {
        let first = runtime_skill("7.0", 'a', "original");
        let original = ScriptedSkills::new([Ok(vec![first.clone()])]);
        let harness = Harness::new(original.clone()).await;
        let agent = harness
            .admit(
                "restart-material",
                vec![SkillRef {
                    source: SkillSource::Ornn,
                    id: SkillId::new("research"),
                    version: Some("@stable".into()),
                }],
            )
            .await;
        harness.run(&agent.id, harness.now).await;
        harness
            .backend
            .lose_next_response(WireOperation::MaterializeAgentHome);
        harness.run(&agent.id, harness.now).await;
        let retrying = harness.stored(&agent.id).await;
        let intent = retrying.work.cleanup_debt[0].clone();
        assert_eq!(
            retrying.work.resolved_skills.as_ref().unwrap()[0].version,
            "7.0"
        );
        assert_eq!(original.calls.load(Ordering::Relaxed), 1);

        let result = if unavailable {
            Err(AgentSkillResolutionError::Unavailable(
                "metadata service is offline".into(),
            ))
        } else {
            Ok(vec![runtime_skill(
                "9.0",
                'b',
                "latest-now-points-elsewhere",
            )])
        };
        let replacement = ScriptedSkills::new([result]);
        *replacement.locators.lock().unwrap() =
            VecDeque::from([vec!["https://new-platform.example/current-download".into()]]);
        let worker = restarted(&harness, replacement.clone());
        for _ in 0..4 {
            assert!(worker
                .run_one(&agent.id, harness.now + Duration::from_secs(2))
                .await
                .unwrap());
        }
        let ready = harness.stored(&agent.id).await;
        assert_eq!(ready.status, AgentStatus::Ready);
        assert_eq!(ready.work.resolved_skills, retrying.work.resolved_skills);
        assert_eq!(ready.profile, agent.profile);
        assert_eq!(replacement.calls.load(Ordering::Relaxed), 0);
        let receipt = ready
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::MaterializeHome)
            .unwrap();
        assert_eq!(receipt.operation_id, intent.operation_id);
        assert_eq!(receipt.request_sha256, intent.request_sha256);
        let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
        assert_eq!(
            snapshot.homes[&AgentSlot::new(agent.id.to_string())].skills,
            [first]
        );
    }
}

#[tokio::test]
async fn empty_skill_selection_is_committed_once_across_a_lost_reply_and_restart() {
    let original = ScriptedSkills::new([Ok(vec![])]);
    let harness = Harness::new(original.clone()).await;
    let agent = harness.admit("restart-empty", vec![]).await;
    assert_eq!(agent.work.resolved_skills, None);
    harness.run(&agent.id, harness.now).await;
    harness
        .backend
        .lose_next_response(WireOperation::MaterializeAgentHome);
    harness.run(&agent.id, harness.now).await;
    assert_eq!(
        harness.stored(&agent.id).await.work.resolved_skills,
        Some(vec![])
    );
    let replacement = ScriptedSkills::new([Err(AgentSkillResolutionError::Unavailable(
        "must not resolve".into(),
    ))]);
    replacement.locators.lock().unwrap().push_back(vec![]);
    let worker = restarted(&harness, replacement.clone());
    for _ in 0..4 {
        assert!(worker
            .run_one(&agent.id, harness.now + Duration::from_secs(2))
            .await
            .unwrap());
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.resolved_skills, Some(vec![]));
    assert_eq!(original.calls.load(Ordering::Relaxed), 1);
    assert_eq!(replacement.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn legacy_home_evidence_without_material_fails_closed_before_resolution_or_effects() {
    for completed in [false, true] {
        let skills = ScriptedSkills::new([]);
        let harness = Harness::new(skills.clone()).await;
        let agent = harness.admit("legacy-home", vec![]).await;
        harness.run(&agent.id, harness.now).await;
        let mut legacy = harness.stored(&agent.id).await;
        let revision = legacy.revision;
        let operation_id = format!("agent:{}:home:create:v1", agent.id);
        if completed {
            legacy.work.receipts.push(AgentWorkReceipt {
                step: AgentWorkStep::MaterializeHome,
                operation_id,
                request_sha256: "c".repeat(64),
                completed_at: rfc3339_at(harness.now),
            });
        } else {
            legacy.work.cleanup_debt.push(AgentCleanupDebt {
                step: AgentWorkStep::MaterializeHome,
                operation_id,
                request_sha256: "c".repeat(64),
            });
        }
        legacy.revision += 1;
        harness
            .store
            .save_agent(revision, legacy.clone())
            .await
            .unwrap();
        harness.run(&agent.id, harness.now).await;
        let failed = harness.stored(&agent.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        assert_eq!(
            failed.failure.as_ref().unwrap().class,
            "agent_skill_material_missing"
        );
        assert_eq!(failed.work.resolved_skills, None);
        assert_eq!(failed.work.cleanup_debt, legacy.work.cleanup_debt);
        assert_eq!(failed.work.receipts, legacy.work.receipts);
        assert_eq!(skills.calls.load(Ordering::Relaxed), 0);
        assert!(harness
            .backend
            .snapshot(&harness.runtime)
            .await
            .unwrap()
            .homes
            .is_empty());
    }
}

#[tokio::test]
async fn invalid_resolved_material_and_runtime_locators_never_create_home_intent() {
    for case in 0..5 {
        let mut material = runtime_skill("7.0", 'a', "archive");
        match case {
            0 => material.id = "different".into(),
            1 => material.version = Some("@stable".into()),
            2 => material.archive_sha256 = "A".repeat(64),
            3 => material.source_url = "http://platform.example/archive".into(),
            _ => material.source_url = "https://user@platform.example/archive".into(),
        }
        let skills = ScriptedSkills::new([Ok(vec![material])]);
        let harness = Harness::new(skills.clone()).await;
        let agent = harness
            .admit(
                "invalid-material",
                vec![SkillRef {
                    source: SkillSource::Ornn,
                    id: SkillId::new("research"),
                    version: None,
                }],
            )
            .await;
        harness.run(&agent.id, harness.now).await;
        harness.run(&agent.id, harness.now).await;
        let failed = harness.stored(&agent.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        assert_eq!(
            failed.failure.as_ref().unwrap().class,
            "agent_skill_invalid"
        );
        assert_eq!(failed.work.resolved_skills, None);
        assert!(failed.work.cleanup_debt.is_empty());
        assert!(harness
            .backend
            .snapshot(&harness.runtime)
            .await
            .unwrap()
            .homes
            .is_empty());
    }
}

#[tokio::test]
async fn live_skill_install_replays_pinned_material_after_lost_response() {
    use layer2_core::sandbox::AgentSkillInstallStatus;
    use layer2_core::{Authorization, Caller, Capability};
    let original_skill = runtime_skill("7.0", 'a', "archive");
    let skills = ScriptedSkills::new([Ok(vec![]), Ok(vec![original_skill.clone()])]);
    let harness = Harness::new(skills.clone()).await;
    let admitted = harness.admit("live-skill", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone());
    operations
        .install_skill(
            &caller,
            &ready.id,
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                version: Some("7.0".into()),
            },
        )
        .await
        .unwrap();
    harness
        .backend
        .lose_next_response(WireOperation::InstallAgentSkill);
    harness.run(&ready.id, SystemTime::now()).await;
    let pending = harness.stored(&ready.id).await;
    assert_eq!(pending.work.step, AgentWorkStep::InstallSkill);
    assert_eq!(
        pending.work.live_skills[0].status,
        AgentSkillInstallStatus::Pending
    );
    assert_eq!(
        pending.work.live_skills[0]
            .material
            .as_ref()
            .unwrap()
            .version,
        "7.0"
    );
    assert_eq!(skills.calls.load(Ordering::Relaxed), 2);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(
        snapshot.homes[&AgentSlot::new(ready.id.as_str())].skills,
        vec![original_skill.clone()]
    );

    let unavailable = ScriptedSkills::new([Err(AgentSkillResolutionError::Unavailable(
        "catalog down".into(),
    ))]);
    *unavailable.locators.lock().unwrap() =
        VecDeque::from([vec![original_skill.source_url.clone()]]);
    let worker = restarted(&harness, unavailable.clone());
    // A retained runtime can park pending filesystem work while reconnecting.
    // Connection publication must re-schedule it after the process is attached.
    let mut parked = pending.clone();
    parked
        .work
        .schedule(AgentWorkStep::None, layer2_core::now_rfc3339());
    parked.revision += 1;
    harness
        .store
        .save_agent(pending.revision, parked)
        .await
        .unwrap();
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        1
    );
    assert!(worker.run_one(&ready.id, SystemTime::now()).await.unwrap());
    assert_eq!(
        harness.stored(&ready.id).await.work.step,
        AgentWorkStep::InstallSkill
    );
    assert!(worker
        .run_one(&ready.id, SystemTime::now() + Duration::from_secs(2))
        .await
        .unwrap());
    let installed = harness.stored(&ready.id).await;
    assert_eq!(installed.work.step, AgentWorkStep::None);
    assert_eq!(
        installed.work.live_skills[0].status,
        AgentSkillInstallStatus::Installed
    );
    assert_eq!(
        installed.work.live_skills[0].material,
        pending.work.live_skills[0].material
    );
    assert_eq!(installed.profile, ready.profile);
    assert_eq!(installed.work.resolved_skills, ready.work.resolved_skills);
    assert_eq!(installed.work.receipts, ready.work.receipts);
    assert_eq!(unavailable.calls.load(Ordering::Relaxed), 0);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(
        snapshot.homes[&AgentSlot::new(ready.id.as_str())].skills,
        vec![original_skill]
    );
}

#[tokio::test]
async fn live_skill_resolution_failure_does_not_fail_the_agent_or_publish_installation() {
    use layer2_core::sandbox::AgentSkillInstallStatus;
    use layer2_core::{Authorization, Caller, Capability};
    let skills = ScriptedSkills::new([
        Ok(vec![]),
        Err(AgentSkillResolutionError::NotFound(
            "not owner accessible".into(),
        )),
    ]);
    let harness = Harness::new(skills).await;
    let admitted = harness.admit("live-skill-denied", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone());
    operations
        .install_skill(
            &caller,
            &ready.id,
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                version: Some("7.0".into()),
            },
        )
        .await
        .unwrap();
    harness.run(&ready.id, SystemTime::now()).await;
    let failed = harness.stored(&ready.id).await;
    assert_eq!(failed.status, AgentStatus::Ready);
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert_eq!(
        failed.work.live_skills[0].status,
        AgentSkillInstallStatus::Failed
    );
    assert!(failed.work.live_skills[0].material.is_none());
    assert_eq!(failed.profile, ready.profile);
    assert!(harness
        .backend
        .snapshot(&harness.runtime)
        .await
        .unwrap()
        .homes[&AgentSlot::new(ready.id.as_str())]
        .skills
        .is_empty());
}

#[path = "skill_upgrade_tests.rs"]
mod upgrade_tests;

#[path = "skill_parent_fence_tests/mod.rs"]
mod parent_fence;

#[path = "skill_resolution_contract_tests.rs"]
mod resolution_contract_tests;
