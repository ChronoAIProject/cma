use super::*;
use layer2_core::sandbox::{agent_live_skills::effective, AgentSkillInstallStatus};
use layer2_core::{Authorization, Caller, Capability};

struct Versions {
    versions: Mutex<VecDeque<Vec<AgentSkill>>>,
    calls: AtomicUsize,
}
impl Versions {
    fn new(versions: Vec<Vec<AgentSkill>>) -> Arc<Self> {
        Arc::new(Self {
            versions: Mutex::new(versions.into()),
            calls: AtomicUsize::new(0),
        })
    }
}
#[async_trait::async_trait]
impl AgentSkillResolver for Versions {
    async fn resolve(
        &self,
        _: &Sandbox,
        _: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .versions
            .lock()
            .unwrap()
            .pop_front()
            .expect("no fresh resolution on replay")
            .into_iter()
            .map(|skill| ResolvedAgentSkill {
                source: SkillSource::Ornn,
                id: SkillId::new(skill.id),
                name: skill.name,
                version: skill.version.unwrap(),
                archive_sha256: skill.archive_sha256,
            })
            .collect())
    }
    fn runtime_skills(
        &self,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError> {
        Ok(material
            .iter()
            .map(|m| AgentSkill {
                id: m.id.as_str().into(),
                name: m.name.clone(),
                version: Some(m.version.clone()),
                archive_sha256: m.archive_sha256.clone(),
                source_url: format!("https://skills.example/archive/{}/{}", m.id, m.version),
            })
            .collect())
    }
}
fn reference(version: &str) -> SkillRef {
    SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        version: Some(version.into()),
    }
}
#[tokio::test]
async fn ten_idle_upgrades_of_profile_and_live_skills_preserve_conversation_and_replay_lost_responses(
) {
    for profile in [true, false] {
        let original = runtime_skill("1.0", 'a', "original");
        let mut materials = vec![if profile {
            vec![original.clone()]
        } else {
            vec![]
        }];
        if !profile {
            materials.push(vec![original]);
        }
        for number in 1..=10 {
            materials.push(vec![runtime_skill(
                &format!("1.{number}"),
                'b',
                "replacement",
            )]);
        }
        let versions = Versions::new(materials);
        let harness = Harness::with_upgrade_support(versions.clone()).await;
        let admitted = harness
            .admit(
                "ten-upgrades",
                if profile {
                    vec![reference("1.0")]
                } else {
                    vec![]
                },
            )
            .await;
        let ready = harness.drive_ready(&admitted.id).await;
        let caller = Caller::new(
            ready.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let operations =
            AgentOperationService::new(harness.store.clone(), harness.registry.clone())
                .with_skill_upgrades_enabled(true);
        if !profile {
            operations
                .install_skill(&caller, &ready.id, reference("1.0"))
                .await
                .unwrap();
            harness.run(&ready.id, SystemTime::now()).await;
        }
        for number in 1..=10 {
            let previous = harness.stored(&ready.id).await;
            let pending = operations
                .install_skill(&caller, &ready.id, reference(&format!("1.{number}")))
                .await
                .unwrap();
            assert_eq!(
                operations
                    .install_skill(&caller, &ready.id, reference(&format!("1.{number}")))
                    .await
                    .unwrap(),
                pending
            );
            harness
                .backend
                .lose_next_response(WireOperation::InstallAgentSkill);
            harness.run(&ready.id, SystemTime::now()).await;
            let pinned = harness.stored(&ready.id).await;
            assert_eq!(
                pinned.work.live_skills.last().unwrap().status,
                AgentSkillInstallStatus::Pending,
                "{:?}",
                pinned.work.live_skills.last()
            );
            assert_eq!(
                effective(&pinned.work, &reference("").id),
                effective(&previous.work, &reference("").id)
            );
            let no_catalog = Versions::new(vec![]);
            let worker = restarted(&harness, no_catalog.clone());
            assert!(worker
                .run_one(&ready.id, SystemTime::now() + Duration::from_secs(2))
                .await
                .unwrap());
            let installed = harness.stored(&ready.id).await;
            assert_eq!(installed.work.step, AgentWorkStep::None);
            assert_eq!(
                installed.work.live_skills.last().unwrap().status,
                AgentSkillInstallStatus::Installed
            );
            assert_eq!(
                effective(&installed.work, &reference("").id)
                    .unwrap()
                    .version,
                format!("1.{number}")
            );
            assert_eq!(installed.work.resolved_skills, ready.work.resolved_skills);
            assert_eq!(installed.profile, ready.profile);
            assert_eq!(
                installed.conversation.conversation_id,
                ready.conversation.conversation_id
            );
            assert_eq!(no_catalog.calls.load(Ordering::Relaxed), 0);
            let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
            let skills = &snapshot.homes[&AgentSlot::new(ready.id.as_str())].skills;
            assert_eq!(skills.len(), 1);
            assert_eq!(
                skills[0].version.as_deref(),
                Some(format!("1.{number}").as_str())
            );
        }
        assert_eq!(
            versions.calls.load(Ordering::Relaxed),
            if profile { 11 } else { 12 }
        );
    }
}
#[tokio::test]
async fn rejected_upgrade_retains_effective_previous_and_all_failure_evidence() {
    let versions = Versions::new(vec![
        vec![runtime_skill("1.0", 'a', "original")],
        vec![runtime_skill("1.1", 'b', "new")],
        vec![runtime_skill("1.2", 'c', "newer")],
    ]);
    let harness = Harness::with_upgrade_support(versions).await;
    let admitted = harness
        .admit("failed-upgrade", vec![reference("1.0")])
        .await;
    let ready = harness.drive_ready(&admitted.id).await;
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
        .with_skill_upgrades_enabled(true);
    operations
        .install_skill(&caller, &ready.id, reference("1.1"))
        .await
        .unwrap();
    harness.backend.fail_next(
        WireOperation::InstallAgentSkill,
        "request_rejected",
        "unexpected destination",
    );
    harness.run(&ready.id, SystemTime::now()).await;
    let failed = harness.stored(&ready.id).await;
    assert_eq!(
        failed.work.live_skills[0].status,
        AgentSkillInstallStatus::Failed
    );
    assert_eq!(
        effective(&failed.work, &reference("").id).unwrap().version,
        "1.0"
    );
    operations
        .install_skill(&caller, &ready.id, reference("1.2"))
        .await
        .unwrap();
    harness.run(&ready.id, SystemTime::now()).await;
    let installed = harness.stored(&ready.id).await;
    assert_eq!(installed.work.live_skills[0], failed.work.live_skills[0]);
    assert_eq!(
        effective(&installed.work, &reference("").id)
            .unwrap()
            .version,
        "1.2"
    );
}

#[tokio::test]
async fn worker_restart_after_material_pin_before_runtime_mutation_uses_no_catalog_lookup() {
    let versions = Versions::new(vec![vec![runtime_skill("1.0", 'a', "original")]]);
    let harness = Harness::with_upgrade_support(versions).await;
    let admitted = harness
        .admit("pinned-upgrade", vec![reference("1.0")])
        .await;
    let ready = harness.drive_ready(&admitted.id).await;
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    AgentOperationService::new(harness.store.clone(), harness.registry.clone())
        .with_skill_upgrades_enabled(true)
        .install_skill(&caller, &ready.id, reference("1.1"))
        .await
        .unwrap();
    let now = SystemTime::now();
    let stamp = rfc3339_at(now);
    let claimed = harness
        .store
        .claim_agent_work(
            &ready.id,
            "crashed-worker",
            stamp.clone(),
            rfc3339_at(now + Duration::from_secs(1)),
        )
        .await
        .unwrap()
        .unwrap();
    let mut pinned = claimed.clone();
    pinned.work.live_skills.last_mut().unwrap().material = Some(ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        name: None,
        version: "1.1".into(),
        archive_sha256: "b".repeat(64),
    });
    pinned.revision += 1;
    pinned.updated_at = stamp.clone();
    harness
        .store
        .commit_agent_work(layer2_core::sandbox::store::AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "crashed-worker".into(),
            fence: claimed.work.lease.as_ref().unwrap().fence,
            agent: pinned,
            worktree: None,
            appended_at: stamp,
            sandbox_event: None,
            agent_logs: vec![],
        })
        .await
        .unwrap();
    let before = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(
        before.homes[&AgentSlot::new(ready.id.as_str())].skills[0]
            .version
            .as_deref(),
        Some("1.0")
    );
    let no_catalog = Versions::new(vec![]);
    assert!(restarted(&harness, no_catalog.clone())
        .run_one(&ready.id, now + Duration::from_secs(2))
        .await
        .unwrap());
    let installed = harness.stored(&ready.id).await;
    assert_eq!(
        effective(&installed.work, &reference("").id)
            .unwrap()
            .version,
        "1.1"
    );
    assert_eq!(no_catalog.calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        installed.conversation.conversation_id,
        ready.conversation.conversation_id
    );
}

#[tokio::test]
async fn terminal_upgrade_reasons_reach_the_owner_and_keep_the_confirmed_version() {
    for reason in [
        layer3_adapter::AgentSkillUpgradeRefusal::Modified,
        layer3_adapter::AgentSkillUpgradeRefusal::PreviousUnavailable,
        layer3_adapter::AgentSkillUpgradeRefusal::RetentionFull,
        layer3_adapter::AgentSkillUpgradeRefusal::RecoveryConflict,
        layer3_adapter::AgentSkillUpgradeRefusal::PreparationExhausted,
    ] {
        let versions = Versions::new(vec![
            vec![runtime_skill("1.0", 'a', "original")],
            vec![runtime_skill("1.1", 'b', "next")],
        ]);
        let harness = Harness::with_upgrade_support(versions).await;
        let admitted = harness.admit("named-upgrade", vec![reference("1.0")]).await;
        let ready = harness.drive_ready(&admitted.id).await;
        let caller = Caller::new(
            ready.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let operations =
            AgentOperationService::new(harness.store.clone(), harness.registry.clone());
        operations
            .with_skill_upgrades_enabled(true)
            .install_skill(&caller, &ready.id, reference("1.1"))
            .await
            .unwrap();
        // A new default-closed service refuses new admission; the admitted worker still settles.
        assert!(!operations.skill_upgrades_enabled());
        harness.backend.fail_next(
            WireOperation::InstallAgentSkill,
            reason.code(),
            "owner action required",
        );
        harness.run(&ready.id, SystemTime::now()).await;
        let failed = harness.stored(&ready.id).await;
        let item = failed.work.live_skills.last().unwrap();
        assert_eq!(item.status, AgentSkillInstallStatus::Failed);
        assert_eq!(
            item.error.as_deref(),
            Some(format!("{}: {reason}", reason.code()).as_str())
        );
        assert_eq!(
            effective(&failed.work, &reference("").id).unwrap().version,
            "1.0"
        );
        assert!(layer2_core::sandbox::agent_live_skills::idle(&failed));
        let outcome = operations
            .send_message(
                &caller,
                &ready.id,
                "Continue after the refused upgrade.".into(),
            )
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            layer2_core::sandbox::AgentMessageOutcome::Dispatched(_)
        ));
    }
}

#[path = "skill_upgrade_retry_tests.rs"]
mod retry_tests;
