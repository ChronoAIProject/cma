use crate::sandbox::store::{
    AgentAdmissionStore, AgentLogStore, AgentStore, AgentWorkMutation, AgentWorkerStore,
    SandboxEventStore, VersionedWorktree, WorktreeStore,
};
use crate::sandbox::{
    AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep, SandboxEventKind, WorktreeStatus,
};

use super::agent_admission_tests::{admission, created, ready_store, NOW};

const MINUTE: &str = "2026-09-05T00:01:00Z";
const TWO_MINUTES: &str = "2026-09-05T00:02:00Z";

mod capacity_tests;

#[tokio::test]
async fn immutable_skill_material_is_atomic_fenced_and_protected_from_ordinary_saves() {
    use crate::profiles::{SkillRef, SkillSource};
    use crate::sandbox::store::SandboxStoreError;
    use crate::sandbox::{AgentCleanupDebt, ResolvedAgentSkill};
    let store = ready_store(1).await;
    let mut input = admission("durable-skills");
    input.profile.content.skills = vec![SkillRef {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        version: None,
    }];
    let creation = created(store.admit_agent(input).await.unwrap());
    let material = vec![ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        name: Some("research".into()),
        version: "2.10".into(),
        archive_sha256: "a".repeat(64),
    }];
    let mut home = creation.agent.clone();
    home.work.step = AgentWorkStep::MaterializeHome;
    home.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::AddWorktree,
        operation_id: format!("worktree:project:{}:add:v1", creation.worktree.name),
        request_sha256: "b".repeat(64),
        completed_at: NOW.into(),
    });
    home.revision += 1;
    store
        .save_agent(creation.agent.revision, home.clone())
        .await
        .unwrap();
    let stale = store
        .claim_agent_work(&home.id, "worker-a", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let current = store
        .claim_agent_work(&home.id, "worker-b", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    let mutation = |claimed: &crate::sandbox::Agent, holder: &str| {
        let mut next = claimed.clone();
        next.work.resolved_skills = Some(material.clone());
        next.work.cleanup_debt.push(AgentCleanupDebt {
            step: AgentWorkStep::MaterializeHome,
            operation_id: format!("agent:{}:home:create:v1", home.id),
            request_sha256: "c".repeat(64),
        });
        next.updated_at = MINUTE.into();
        next.revision += 1;
        AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: holder.into(),
            fence: claimed.work.lease.as_ref().unwrap().fence,
            agent: next,
            worktree: None,
            appended_at: MINUTE.into(),
            sandbox_event: None,
            agent_logs: vec![],
        }
    };
    assert_eq!(
        store.commit_agent_work(mutation(&stale, "worker-a")).await,
        Err(SandboxStoreError::Conflict)
    );
    let accepted = mutation(&current, "worker-b");
    let mut half = accepted.clone();
    half.agent.work.resolved_skills = None;
    assert_eq!(
        store.commit_agent_work(half).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut half = accepted.clone();
    half.agent.work.cleanup_debt.clear();
    assert_eq!(
        store.commit_agent_work(half).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&home.id).await.unwrap(), Some(current));
    store.commit_agent_work(accepted.clone()).await.unwrap();
    let stored = store.get_agent(&home.id).await.unwrap().unwrap();
    assert_eq!(stored, accepted.agent);
    assert_eq!(stored.profile, creation.agent.profile);
    for mutation in 0..4 {
        let mut changed = stored.clone();
        changed.revision += 1;
        match mutation {
            0 => changed.work.resolved_skills = None,
            1 => changed.work.resolved_skills.as_mut().unwrap()[0].version = "9.0".into(),
            2 => changed.owner = crate::SubjectId::new("different-owner"),
            _ => changed.profile.content.skills[0].version = Some("9.0".into()),
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
    assert_eq!(store.get_agent(&home.id).await.unwrap(), Some(unchanged));
}

#[tokio::test]
async fn due_discovery_and_claim_are_bounded_and_fenced() {
    let store = ready_store(2).await;
    let first = created(store.admit_agent(admission("first-due")).await.unwrap());
    let second = created(store.admit_agent(admission("second-due")).await.unwrap());

    assert_eq!(store.due_agent_work(NOW, 1).await.unwrap().len(), 1);
    let claimed = store
        .claim_agent_work(&first.agent.id, "worker-a", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    assert!(claimed.work.owns("worker-a", 1));
    assert!(store
        .claim_agent_work(&first.agent.id, "worker-b", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.due_agent_work(NOW, 2).await.unwrap(),
        vec![second.agent.id]
    );

    let takeover = store
        .claim_agent_work(
            &first.agent.id,
            "worker-b",
            MINUTE.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(takeover.work.owns("worker-b", 2));
    assert_eq!(takeover.revision, claimed.revision + 1);
}

#[tokio::test]
async fn stale_holder_cannot_publish_after_lease_takeover() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("stale-holder")).await.unwrap());
    let stale = store
        .claim_agent_work(&creation.agent.id, "worker-a", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let current = store
        .claim_agent_work(
            &creation.agent.id,
            "worker-b",
            MINUTE.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();

    let stale_fence = stale.work.lease.as_ref().unwrap().fence;
    let mut next = stale.clone();
    next.work
        .advance(
            "worker-a",
            stale_fence,
            AgentWorkStep::MaterializeHome,
            MINUTE.into(),
        )
        .unwrap();
    next.revision += 1;
    let result = store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: stale.revision,
            holder: "worker-a".into(),
            fence: stale_fence,
            agent: next,
            worktree: None,
            appended_at: MINUTE.into(),
            sandbox_event: None,
            agent_logs: vec![],
        })
        .await;

    assert_eq!(
        result.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(
        store.get_agent(&creation.agent.id).await.unwrap().unwrap(),
        current
    );
}

#[tokio::test]
async fn ready_publication_updates_every_artifact_atomically() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("ready-publication"))
            .await
            .unwrap(),
    );
    let mut claimed = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    claimed.work.step = AgentWorkStep::PublishReady;
    claimed.conversation.conversation_id = Some("conversation-1".into());
    claimed.conversation.runtime_generation = Some(1);
    claimed.work.receipts = [
        (
            AgentWorkStep::AddWorktree,
            "worktree:project:ready-publication:add:v1".into(),
        ),
        (
            AgentWorkStep::MaterializeHome,
            format!("agent:{}:home:create:v1", claimed.id),
        ),
        (
            AgentWorkStep::OpenConversation,
            format!("agent:{}:open:v1", claimed.id),
        ),
    ]
    .into_iter()
    .map(|(step, operation_id)| AgentWorkReceipt {
        step,
        operation_id,
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    })
    .collect();
    store
        .lock()
        .agents
        .insert(claimed.id.clone(), claimed.clone());
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut agent = claimed.clone();
    agent
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    agent
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    agent.conversation.last_seq += 2;
    let mut worktree = creation.worktree.clone();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: agent.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: worktree.clone(),
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: agent.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: vec![
                AgentLogKind::ConversationOpened {
                    conversation_id: "conversation-1".into(),
                    runtime_generation: 1,
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                },
            ],
        })
        .await
        .unwrap();

    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    assert_eq!(
        store
            .agent_log(&creation.agent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        store
            .sandbox_events(&creation.agent.sandbox_id, 0, 10)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn publication_rejects_an_incorrect_agent_log_high_water_mark() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("incorrect-log-high-water"))
            .await
            .unwrap(),
    );
    let claimed = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("worker", fence, AgentWorkStep::MaterializeHome, NOW.into())
        .unwrap();
    next.revision += 1;

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence,
                agent: next,
                worktree: None,
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(crate::sandbox::store::SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&creation.agent.id).await.unwrap(),
        Some(claimed)
    );
    assert_eq!(
        store
            .agent_log(&creation.agent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn late_worktree_conflict_rolls_back_agent_log_and_event() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("rollback")).await.unwrap());
    let claimed = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("worker", fence, AgentWorkStep::MaterializeHome, NOW.into())
        .unwrap();
    next.revision += 1;
    let mut invalid_worktree = creation.worktree.clone();
    invalid_worktree.status = WorktreeStatus::Ready;
    invalid_worktree.revision += 2;

    let result = store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next,
            worktree: Some(VersionedWorktree {
                expected_revision: creation.worktree.revision,
                worktree: invalid_worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: creation.agent.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: vec![AgentLogKind::StatusChanged {
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }],
        })
        .await;

    assert_eq!(
        result.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(
        store.get_agent(&creation.agent.id).await.unwrap(),
        Some(claimed)
    );
    assert_eq!(
        store.get_worktree(&creation.worktree.id).await.unwrap(),
        Some(creation.worktree)
    );
    assert_eq!(
        store
            .agent_log(&creation.agent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .sandbox_events(&creation.agent.sandbox_id, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
}
