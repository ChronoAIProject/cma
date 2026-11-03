use super::*;
use crate::profiles::{SkillRef, SkillSource};
fn reference() -> SkillRef {
    SkillRef {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        version: Some("1.0".into()),
    }
}

#[tokio::test]
async fn live_skill_admission_is_durable_idempotent_and_blocks_conversation_input() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let pending = service
        .install_skill(&owner(), &ready.id, reference())
        .await
        .unwrap();
    assert_eq!(pending.work.step, AgentWorkStep::InstallSkill);
    assert_eq!(pending.work.live_skills.len(), 1);
    assert_eq!(
        pending.work.live_skills[0].status,
        AgentSkillInstallStatus::Pending
    );
    assert!(pending.work.live_skills[0].material.is_none());
    assert_eq!(pending.profile, ready.profile);
    assert_eq!(pending.work.resolved_skills, ready.work.resolved_skills);
    assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), pending);
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, reference())
            .await
            .unwrap(),
        pending
    );
    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "next".into())
            .await,
        Err(AgentOperationError::StatusConflict { .. })
    ));
    assert!(inputs.lock().unwrap().is_empty());
    let mut changed = reference();
    changed.version = Some("2.0".into());
    assert!(matches!(
        service.install_skill(&owner(), &ready.id, changed).await,
        Err(AgentOperationError::Conflict)
    ));
}

#[tokio::test]
async fn installed_skill_version_change_is_reported_as_unsupported() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut ready = test_agent(AgentStatus::Ready, None);
    ready.work.resolved_skills = Some(vec![ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        name: Some("Research".into()),
        version: "1.0".into(),
        archive_sha256: "a".repeat(64),
    }]);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );

    let mut requested = reference();
    requested.version = Some("1.1".into());
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, requested)
            .await,
        Err(AgentOperationError::SkillVersionUpgradeUnsupported {
            id: "research".into(),
            installed: "1.0".into(),
            requested: "1.1".into(),
        })
    );
    assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), ready);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn profile_skill_version_change_is_reported_as_unsupported() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut ready = test_agent(AgentStatus::Ready, None);
    ready.profile.content.skills = vec![reference()];
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );

    let mut requested = reference();
    requested.version = Some("1.1".into());
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, requested)
            .await,
        Err(AgentOperationError::SkillVersionUpgradeUnsupported {
            id: "research".into(),
            installed: "1.0".into(),
            requested: "1.1".into(),
        })
    );
    assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), ready);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn live_skill_admission_rejects_other_owners_busy_agents_and_unsafe_paths() {
    for case in 0..6 {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut ready = test_agent(AgentStatus::Ready, None);
        seed_active_parent(&store).await;
        if case == 2 {
            ready.status = AgentStatus::Working;
            ready.conversation.active_turn = Some(7);
        }
        store.create_agent(ready.clone()).await.unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let service = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let caller = match case {
            0 => unauthorized_owner(),
            1 => other_owner(),
            _ => owner(),
        };
        let mut skill = reference();
        if case == 3 {
            skill.id = crate::SkillId::new("../escape");
        }
        if case == 4 {
            skill.id = crate::SkillId::new("nyxid");
        }
        if case == 5 {
            skill.version = Some("@stable".into());
        }
        assert!(service
            .install_skill(&caller, &ready.id, skill)
            .await
            .is_err());
        assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), ready);
        assert!(inputs.lock().unwrap().is_empty());
    }
}

#[test]
fn live_skill_persistence_rejects_changed_version_and_forged_admission_confirmation() {
    let ready = test_agent(AgentStatus::Ready, None);
    let mut next = ready.clone();
    next.work
        .live_skills
        .push(AgentSkillInstallation::new(reference()));
    next.work
        .schedule(AgentWorkStep::InstallSkill, next.updated_at.clone());
    assert!(agent_live_skills::is_install_admission(&ready, &next));
    next.work.live_skills[0].material = Some(ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        name: None,
        version: "2.0".into(),
        archive_sha256: "a".repeat(64),
    });
    assert!(
        agent_live_skills::validate_installations(&next.work, &next.profile.content.skills)
            .is_err()
    );
    next.work.live_skills[0].material.as_mut().unwrap().version = "1.0".into();
    assert!(
        agent_live_skills::validate_installations(&next.work, &next.profile.content.skills).is_ok()
    );
    assert!(!agent_live_skills::is_install_admission(&ready, &next));
    next.work.live_skills[0].status = AgentSkillInstallStatus::Installed;
    next.work
        .schedule(AgentWorkStep::None, next.updated_at.clone());
    assert!(!agent_live_skills::is_install_admission(&ready, &next));
}
