use crate::sandbox::store::{
    AgentAdmissionStore, AgentLogStore, AgentStore, AgentWorkMutation, AgentWorkerStore,
    SandboxEventStore, VersionedWorktree, WorktreeStore,
};
use crate::sandbox::{AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, WorktreeStatus};

use super::agent_admission_tests::{admission, created, ready_store, NOW};

const MINUTE: &str = "2026-09-05T00:01:00Z";
const TWO_MINUTES: &str = "2026-09-05T00:02:00Z";

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
    let claimed = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut agent = claimed.clone();
    agent.conversation.conversation_id = Some("conversation-1".into());
    agent.conversation.runtime_generation = Some(1);
    agent
        .work
        .advance("worker", fence, AgentWorkStep::None, NOW.into())
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
