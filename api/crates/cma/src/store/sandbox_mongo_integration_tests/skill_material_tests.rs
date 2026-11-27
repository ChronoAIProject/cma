use super::*;
use layer2_core::profiles::{SkillRef, SkillSource};
use layer2_core::sandbox::{AgentCleanupDebt, ResolvedAgentSkill};
use layer2_core::SkillId;

use super::super::agents::{agent_document, read_agent, read_agent_owned};

#[test]
fn owned_agent_decode_preserves_record_and_projection_validation() {
    let agent = agent_for_worker("agt_owned_decode", NOW);
    let document = agent_document(&agent).unwrap();
    assert_eq!(read_agent_owned(document.clone()).unwrap(), agent);
    for (field, replacement) in [
        ("document_schema_version", Bson::Int64(99)),
        ("record", Bson::Null),
        ("owner", Bson::String("different-owner".into())),
        ("last_seq", Bson::Int64(99)),
    ] {
        let mut invalid = document.clone();
        invalid.insert(field, replacement);
        assert_eq!(
            read_agent(&invalid).unwrap_err().to_string(),
            read_agent_owned(invalid).unwrap_err().to_string()
        );
    }
}

fn selected_material() -> Vec<ResolvedAgentSkill> {
    vec![ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        name: None,
        version: "7.0".into(),
        archive_sha256: "a".repeat(64),
    }]
}

fn select_reference(agent: &mut Agent) {
    agent.profile.content.skills = vec![SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        version: Some("@stable".into()),
    }];
}

#[test]
fn bson_skill_material_roundtrip_preserves_absence_empty_and_literal_selection() {
    let mut agent = agent_for_worker("agt_material_bson", NOW);
    let document = agent_document(&agent).unwrap();
    assert!(!document
        .get_document("record")
        .unwrap()
        .get_document("work")
        .unwrap()
        .contains_key("resolved_skills"));
    assert_eq!(read_agent(&document).unwrap(), agent);
    agent.work.resolved_skills = Some(vec![]);
    assert_eq!(read_agent(&agent_document(&agent).unwrap()).unwrap(), agent);
    select_reference(&mut agent);
    let profile = agent.profile.clone();
    agent.work.resolved_skills = Some(selected_material());
    let document = agent_document(&agent).unwrap();
    let stored = read_agent(&document).unwrap();
    assert_eq!(stored, agent);
    assert_eq!(stored.profile, profile);
    let material = document
        .get_document("record")
        .unwrap()
        .get_document("work")
        .unwrap()
        .get_array("resolved_skills")
        .unwrap()[0]
        .as_document()
        .unwrap();
    assert_eq!(material.len(), 4);
    assert_eq!(material.get_str("version").unwrap(), "7.0");
    assert!(material.get("source_url").is_none());
}

#[test]
fn bson_skill_material_rejects_corruption_and_injected_locator_fields() {
    let mut agent = agent_for_worker("agt_material_corrupt", NOW);
    select_reference(&mut agent);
    agent.work.resolved_skills = Some(selected_material());
    for case in 0..6 {
        let mut document = agent_document(&agent).unwrap();
        let work = document
            .get_document_mut("record")
            .unwrap()
            .get_document_mut("work")
            .unwrap();
        if case == 0 {
            work.insert("resolved_skills", Vec::<Bson>::new());
        } else {
            let item = work.get_array_mut("resolved_skills").unwrap()[0]
                .as_document_mut()
                .unwrap();
            match case {
                1 => {
                    item.insert("id", "different");
                }
                2 => {
                    item.insert("version", "");
                }
                3 => {
                    item.insert("archive_sha256", "A".repeat(64));
                }
                4 => {
                    item.insert("source_url", "https://platform.example/archive");
                }
                _ => {
                    item.insert("bearer", "forbidden-authority-field");
                }
            }
        }
        assert!(matches!(
            read_agent(&document),
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
    agent.work.resolved_skills.as_mut().unwrap()[0].id = SkillId::new("different");
    assert!(matches!(
        agent_document(&agent),
        Err(SandboxStoreError::Corrupt(_))
    ));
}

fn intent(current: &Agent, holder: &str, selected: Vec<ResolvedAgentSkill>) -> AgentWorkMutation {
    let mut next = current.clone();
    next.work.resolved_skills = Some(selected);
    next.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::MaterializeHome,
        operation_id: format!("agent:{}:home:create:v1", current.id),
        request_sha256: "c".repeat(64),
    });
    next.updated_at = MINUTE.into();
    next.revision += 1;
    AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: holder.into(),
        fence: current.work.lease.as_ref().unwrap().fence,
        agent: next,
        worktree: None,
        appended_at: MINUTE.into(),
        sandbox_event: None,
        agent_logs: vec![],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_skill_material_is_atomic_fenced_and_immutable_for_ordinary_saves() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_skill_material_{}", std::process::id());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for empty in [false, true] {
        let suffix = if empty { "empty" } else { "selected" };
        let (_, worktree, mut stale) = claimed_agent_work_fixture(&store, suffix).await;
        if !empty {
            select_reference(&mut stale);
        }
        stale.work.step = AgentWorkStep::MaterializeHome;
        let workspace = store
            .get_workspace(&stale.workspace_id)
            .await
            .unwrap()
            .unwrap();
        stale.work.receipts.push(AgentWorkReceipt {
            step: AgentWorkStep::AddWorktree,
            operation_id: format!("worktree:{}:{}:add:v1", workspace.name, worktree.name),
            request_sha256: "b".repeat(64),
            completed_at: NOW.into(),
        });
        replace_agent_fixture(&store, &stale).await;
        let material = if empty { vec![] } else { selected_material() };
        let mut ordinary = stale.clone();
        ordinary.revision += 1;
        ordinary.work.resolved_skills = Some(material.clone());
        assert_eq!(
            store.save_agent(stale.revision, ordinary).await,
            Err(SandboxStoreError::Conflict)
        );
        let current = store
            .claim_agent_work(&stale.id, "worker-b", MINUTE.into(), TWO_MINUTES.into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .commit_agent_work(intent(&stale, "worker", material.clone()))
                .await,
            Err(SandboxStoreError::Conflict)
        );
        let accepted = intent(&current, "worker-b", material);
        for missing_material in [false, true] {
            let mut half = accepted.clone();
            if missing_material {
                half.agent.work.resolved_skills = None;
            } else {
                half.agent.work.cleanup_debt.clear();
            }
            assert_eq!(
                store.commit_agent_work(half).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(
                store.get_agent(&stale.id).await.unwrap(),
                Some(current.clone())
            );
        }
        store.commit_agent_work(accepted.clone()).await.unwrap();
        let stored = store.get_agent(&stale.id).await.unwrap().unwrap();
        assert_eq!(stored, accepted.agent);
        assert_eq!(stored.profile, stale.profile);
        for case in 0..4 {
            if empty && case == 1 {
                continue;
            }
            let mut changed = stored.clone();
            changed.revision += 1;
            match case {
                0 => changed.work.resolved_skills = None,
                1 => changed.work.resolved_skills.as_mut().unwrap()[0].version = "9.0".into(),
                2 => changed.owner = SubjectId::new("different-owner"),
                _ => changed.profile.digest = "e".repeat(64),
            }
            assert_eq!(
                store.save_agent(stored.revision, changed).await,
                Err(SandboxStoreError::Conflict)
            );
        }
        let mut unchanged = stored.clone();
        unchanged.revision += 1;
        store
            .save_agent(stored.revision, unchanged.clone())
            .await
            .unwrap();
        assert_eq!(store.get_agent(&stale.id).await.unwrap(), Some(unchanged));
    }
    client.database(&name).drop().await.unwrap();
}

#[path = "skill_upgrade_tests.rs"]
mod upgrade_tests;
