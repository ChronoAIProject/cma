use super::*;
use crate::profiles::{SkillRef, SkillSource};
use serde::{Deserialize, Serialize};

fn reference(version: &str) -> SkillRef {
    SkillRef {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        version: Some(version.into()),
    }
}
fn material(version: &str) -> ResolvedAgentSkill {
    ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        name: None,
        version: version.into(),
        archive_sha256: "a".repeat(64),
    }
}
fn installed() -> Agent {
    let mut agent = test_agent(AgentStatus::Ready, None);
    let mut item = AgentSkillInstallation::new(reference("1.0"));
    item.material = Some(material("1.0"));
    item.status = AgentSkillInstallStatus::Installed;
    agent.work.live_skills.push(item);
    agent
}
#[tokio::test]
async fn live_upgrade_is_idempotent_busy_refusal_is_accurate_and_retry_keeps_history() {
    let mut busy = installed();
    busy.status = AgentStatus::Working;
    busy.conversation.active_turn = Some(7);
    let store = Arc::new(InMemorySandboxStore::with_agent_fixture(busy.clone()));
    seed_active_parent(&store).await;
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    )
    .with_skill_upgrades_enabled(true);
    assert_eq!(
        service
            .install_skill(&owner(), &busy.id, reference("1.1"))
            .await,
        Err(AgentOperationError::StatusConflict {
            status: AgentStatus::Working
        })
    );
    let mut ready = busy.clone();
    ready.status = AgentStatus::Ready;
    ready.conversation.active_turn = None;
    ready.revision += 1;
    store
        .save_agent(busy.revision, ready.clone())
        .await
        .unwrap();
    let next = service
        .install_skill(&owner(), &ready.id, reference("1.1"))
        .await
        .unwrap();
    assert_eq!(next.work.live_skills.len(), 2);
    assert_eq!(next.work.live_skills[0], ready.work.live_skills[0]);
    assert_eq!(next.work.live_skills[1].previous, Some(material("1.0")));
    assert_eq!(
        agent_live_skills::effective(&next.work, &reference("1.0").id),
        Some(&material("1.0"))
    );
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, reference("1.1"))
            .await
            .unwrap(),
        next
    );
    assert_eq!(store.agent_log(&ready.id, 0, 100).await.unwrap().len(), 1);
}
#[test]
fn upgrade_admission_requires_exact_previous_evidence_and_retains_failed_attempts() {
    let ready = installed();
    let mut next = ready.clone();
    let mut item = AgentSkillInstallation::new(reference("1.1"));
    item.previous = Some(material("1.0"));
    next.work.live_skills.push(item);
    next.work
        .schedule(AgentWorkStep::InstallSkill, next.updated_at.clone());
    assert!(agent_live_skills::is_install_admission(&ready, &next));
    next.work.live_skills[1]
        .previous
        .as_mut()
        .unwrap()
        .archive_sha256 = "b".repeat(64);
    assert!(!agent_live_skills::is_install_admission(&ready, &next));
    next.work.live_skills[1].previous = Some(material("1.0"));
    next.work.live_skills[1].status = AgentSkillInstallStatus::Failed;
    next.work.live_skills[1].error = Some("destination_changed".into());
    next.work
        .schedule(AgentWorkStep::None, next.updated_at.clone());
    assert!(agent_live_skills::validate_installations(&next.work, &[]).is_ok());
    assert_eq!(
        agent_live_skills::effective(&next.work, &reference("1.0").id),
        Some(&material("1.0"))
    );
    let mut retry = next.clone();
    let mut item = AgentSkillInstallation::new(reference("1.2"));
    item.previous = Some(material("1.0"));
    retry.work.live_skills.push(item);
    retry
        .work
        .schedule(AgentWorkStep::InstallSkill, next.updated_at.clone());
    assert!(agent_live_skills::is_install_admission(&next, &retry));
}
#[test]
fn additive_records_remain_byte_compatible_and_upgrade_intent_raises_the_reader_floor() {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Predecessor {
        reference: SkillRef,
        status: AgentSkillInstallStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<ResolvedAgentSkill>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    }
    for item in [
        AgentSkillInstallation::new(reference("1.0")),
        installed().work.live_skills[0].clone(),
    ] {
        let bytes = serde_json::to_vec(&item).unwrap();
        let prior: Predecessor = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_vec(&prior).unwrap(), bytes);
        let mut upgraded = item;
        upgraded.reference = reference("1.1");
        upgraded.previous = Some(material("1.0"));
        assert!(
            serde_json::from_slice::<Predecessor>(&serde_json::to_vec(&upgraded).unwrap()).is_err()
        );
    }
}

#[test]
fn skill_slots_count_effective_identifiers_not_upgrade_history() {
    let mut ready = installed();
    for n in 1..32 {
        let mut skill = reference("1.0");
        skill.id = crate::SkillId::new(format!("skill-{n}"));
        ready.work.live_skills.push(AgentSkillInstallation {
            status: AgentSkillInstallStatus::Failed,
            error: Some("failed".into()),
            ..AgentSkillInstallation::new(skill)
        });
    }
    assert_eq!(agent_live_skills::capacity(&ready), (false, true));
    let mut next = ready.clone();
    let mut item = AgentSkillInstallation::new(reference("1.1"));
    item.previous = Some(material("1.0"));
    next.work.live_skills.push(item);
    next.work
        .schedule(AgentWorkStep::InstallSkill, next.updated_at.clone());
    assert!(agent_live_skills::is_install_admission(&ready, &next));
    let item = &mut ready.work.live_skills[0];
    item.status = AgentSkillInstallStatus::Failed;
    item.error = Some("failed".into());
    assert_eq!(agent_live_skills::capacity(&ready), (false, false));
}

#[tokio::test]
async fn closed_upgrade_gate_preserves_ordinary_installs_and_accepted_work() {
    let ready = installed();
    let store = Arc::new(InMemorySandboxStore::with_agent_fixture(ready.clone()));
    seed_active_parent(&store).await;
    let (sender, _) = recording_sender(Ok(()));
    let closed = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(!closed.skill_upgrades_enabled());
    assert_eq!(
        closed
            .install_skill(&owner(), &ready.id, reference("1.1"))
            .await,
        Err(AgentOperationError::SkillUpgradeDisabled)
    );
    assert_eq!(
        closed
            .install_skill(&owner(), &ready.id, reference("1.0"))
            .await
            .unwrap(),
        ready
    );
    let opened = closed.with_skill_upgrades_enabled(true);
    let pending = opened
        .install_skill(&owner(), &ready.id, reference("1.1"))
        .await
        .unwrap();
    assert_eq!(
        closed
            .install_skill(&owner(), &ready.id, reference("1.1"))
            .await
            .unwrap(),
        pending
    );
    assert_eq!(store.agent_log(&ready.id, 0, 100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn history_capacity_bounds_hot_metadata_and_refuses_new_work_without_truncating() {
    let mut ready = test_agent(AgentStatus::Ready, None);
    for n in 0..128 {
        let version = format!("{n:03}{}", "v".repeat(253));
        let mut item = AgentSkillInstallation::new(reference(&version));
        item.previous = agent_live_skills::effective(&ready.work, &item.reference.id).cloned();
        let mut pinned = material(&version);
        pinned.name = Some("n".repeat(256));
        item.material = Some(pinned);
        item.status = AgentSkillInstallStatus::Installed;
        ready.work.live_skills.push(item);
    }
    assert!(agent_live_skills::validate_installations(&ready.work, &[]).is_ok());
    assert!(serde_json::to_vec(&ready.work.live_skills).unwrap().len() < 512 * 1024);
    assert_eq!(agent_live_skills::capacity(&ready), (false, false));
    let store = Arc::new(InMemorySandboxStore::with_agent_fixture(ready.clone()));
    seed_active_parent(&store).await;
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    )
    .with_skill_upgrades_enabled(true);
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, reference("next"))
            .await,
        Err(AgentOperationError::SkillHistoryFull)
    );
    assert_eq!(
        service
            .install_skill(
                &owner(),
                &ready.id,
                ready.work.live_skills.last().unwrap().reference.clone()
            )
            .await
            .unwrap(),
        ready
    );
    assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), ready);
}
