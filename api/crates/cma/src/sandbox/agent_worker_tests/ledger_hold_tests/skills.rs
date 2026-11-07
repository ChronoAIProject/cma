use super::*;
use crate::sandbox::agent_worker_tests::skill_material_tests::runtime_skill;
use layer2_core::sandbox::AgentSkillInstallStatus;
use layer2_core::{Authorization, Caller, Capability};

#[tokio::test]
async fn ledger_hold_live_skill_keeps_material_pending_until_success() {
    let skills = ScriptedSkills::new([Ok(vec![]), Ok(vec![runtime_skill("7.0", 'a', "archive")])]);
    skills.locators.lock().unwrap().extend([
        vec![runtime_skill("7.0", 'a', "archive").source_url],
        vec![runtime_skill("7.0", 'a', "archive").source_url],
    ]);
    let h = Harness::new(skills).await;
    let initial = h.admit("ledger-skill", vec![]).await;
    let ready = h.drive_ready(&initial.id).await;
    let service = AgentOperationService::new(h.store.clone(), h.registry.clone());
    service
        .install_skill(
            &Caller::new(
                ready.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            &ready.id,
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                version: Some("7.0".into()),
            },
        )
        .await
        .unwrap();
    let mut at = SystemTime::now();
    let mut material = None;
    for delay in [30, 60] {
        refuse(&h, WireOperation::InstallAgentSkill);
        h.run(&initial.id, at).await;
        let held = h.stored(&initial.id).await;
        assert_eq!(held.work.step, AgentWorkStep::InstallSkill);
        assert_eq!(
            held.work.live_skills[0].status,
            AgentSkillInstallStatus::Pending
        );
        if let Some(expected) = &material {
            assert_eq!(&held.work.live_skills[0].material, expected);
        }
        material = Some(held.work.live_skills[0].material.clone());
        assert_eq!(
            layer2_core::seconds_from_rfc3339(&held.work.ready_at).unwrap()
                - layer2_core::seconds_from_rfc3339(&held.updated_at).unwrap(),
            delay
        );
        assert!(held.work.lease.is_none());
        at = due(&held);
    }
    h.run(&initial.id, at).await;
    let installed = h.stored(&initial.id).await;
    assert_eq!(installed.work.step, AgentWorkStep::None);
    assert_eq!(installed.work.attempts, 0);
    assert_eq!(
        installed.work.live_skills[0].status,
        AgentSkillInstallStatus::Installed
    );
}
