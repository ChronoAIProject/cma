use super::*;
use layer2_core::sandbox::store::SandboxMutationGuard;
use layer2_core::sandbox::{AgentSkillInstallStatus, AgentSkillInstallation};

fn initial(agent: &mut Agent, profile: bool) -> ResolvedAgentSkill {
    let material = selected_material().remove(0);
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    if profile {
        select_reference(agent);
        agent.work.resolved_skills = Some(vec![material.clone()]);
    } else {
        let mut item = AgentSkillInstallation::new(SkillRef {
            source: material.source,
            id: material.id.clone(),
            version: Some(material.version.clone()),
        });
        item.material = Some(material.clone());
        item.status = AgentSkillInstallStatus::Installed;
        agent.work.live_skills.push(item);
    }
    material
}
#[test]
fn bson_upgrade_keeps_original_profile_material_and_roundtrips_previous_identity() {
    for profile in [false, true] {
        let mut agent = agent_for_worker("agt_upgrade_bson", NOW);
        let previous = initial(&mut agent, profile);
        let mut item = AgentSkillInstallation::new(SkillRef {
            source: SkillSource::Ornn,
            id: previous.id.clone(),
            version: Some("8.0".into()),
        });
        item.previous = Some(previous);
        agent.work.live_skills.push(item);
        agent.work.schedule(AgentWorkStep::InstallSkill, NOW.into());
        let document = agent_document(&agent).unwrap();
        assert_eq!(read_agent(&document).unwrap(), agent);
        let history = document
            .get_document("record")
            .unwrap()
            .get_document("work")
            .unwrap()
            .get_array("live_skills")
            .unwrap();
        assert_eq!(
            history
                .last()
                .unwrap()
                .as_document()
                .unwrap()
                .get_document("previous")
                .unwrap()
                .get_str("version")
                .unwrap(),
            "7.0"
        );
        if !profile {
            assert!(!history[0].as_document().unwrap().contains_key("previous"));
        }
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_skill_upgrade_admission_and_material_are_cas_fenced() {
    let (client, name) = database("skill_upgrade").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for profile in [false, true] {
        let mut ready = ready_conversation_fixture(
            &store,
            if profile {
                "profile-upgrade"
            } else {
                "live-upgrade"
            },
        )
        .await;
        let previous = initial(&mut ready, profile);
        replace_agent_fixture(&store, &ready).await;
        let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
        let mut next = ready.clone();
        let mut item = AgentSkillInstallation::new(SkillRef {
            source: SkillSource::Ornn,
            id: previous.id.clone(),
            version: Some("8.0".into()),
        });
        item.previous = Some(previous);
        next.work.live_skills.push(item);
        next.work
            .schedule(AgentWorkStep::InstallSkill, MINUTE.into());
        next.revision += 1;
        next.updated_at = MINUTE.into();
        next.conversation.last_seq += 1;
        let admission = AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(&parent)),
            expected_agent_revision: ready.revision,
            expected_last_seq: ready.conversation.last_seq,
            runtime_generation: ready.conversation.runtime_generation.unwrap(),
            agent: next,
            appended_at: MINUTE.into(),
            agent_logs: vec![AgentLogKind::SkillInstallationChanged {
                skill_id: "research".into(),
                status: AgentSkillInstallStatus::Pending,
            }],
        };
        for bad_previous in [true, false] {
            let mut forged = admission.clone();
            if bad_previous {
                forged
                    .agent
                    .work
                    .live_skills
                    .last_mut()
                    .unwrap()
                    .previous
                    .as_mut()
                    .unwrap()
                    .archive_sha256 = "c".repeat(64);
            } else {
                forged.expected_agent_revision += 1;
            }
            assert_eq!(
                store.commit_agent_conversation(forged).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), ready);
        }
        store
            .commit_agent_conversation(admission.clone())
            .await
            .unwrap();
        assert_eq!(
            store.commit_agent_conversation(admission).await,
            Err(SandboxStoreError::Conflict)
        );
        let stale = store
            .claim_agent_work(&ready.id, "worker-a", MINUTE.into(), TWO_MINUTES.into())
            .await
            .unwrap()
            .unwrap();
        let current = store
            .claim_agent_work(
                &ready.id,
                "worker-b",
                TWO_MINUTES.into(),
                "2026-09-05T00:03:00Z".into(),
            )
            .await
            .unwrap()
            .unwrap();
        let pin = |current: &Agent, holder: &str| {
            let mut next = current.clone();
            let mut material = selected_material().remove(0);
            material.version = "8.0".into();
            material.archive_sha256 = "b".repeat(64);
            next.work.live_skills.last_mut().unwrap().material = Some(material);
            next.revision += 1;
            next.updated_at = TWO_MINUTES.into();
            AgentWorkMutation {
                expected_agent_revision: current.revision,
                holder: holder.into(),
                fence: current.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: None,
                appended_at: TWO_MINUTES.into(),
                sandbox_event: None,
                agent_logs: vec![],
            }
        };
        assert_eq!(
            store.commit_agent_work(pin(&stale, "worker-a")).await,
            Err(SandboxStoreError::Conflict)
        );
        let pinned = pin(&current, "worker-b");
        let mut forged = pinned.clone();
        forged
            .agent
            .work
            .live_skills
            .last_mut()
            .unwrap()
            .previous
            .as_mut()
            .unwrap()
            .version = "6.0".into();
        assert_eq!(
            store.commit_agent_work(forged).await,
            Err(SandboxStoreError::Conflict)
        );
        store.commit_agent_work(pinned.clone()).await.unwrap();
        assert_eq!(
            store.get_agent(&ready.id).await.unwrap().unwrap(),
            pinned.agent
        );
    }
    client.database(&name).drop().await.unwrap();
}
