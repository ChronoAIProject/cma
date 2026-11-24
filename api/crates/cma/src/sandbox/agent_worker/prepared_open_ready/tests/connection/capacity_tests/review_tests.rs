use super::*;
use crate::store::adoption_probe::{self, Boundary};
use layer2_core::sandbox::store::{
    AgentLogPublicationIdentity, AgentWorkMutation, VersionedWorktree, WorktreeStore,
};
use layer2_core::sandbox::SandboxEventKind;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn publication_lease_expiry_allows_replacement_only_after_abort_and_gate_reopen() {
    let fixture = Fixture::with_adoption(None, true).await;
    let full = initialize_connection(&fixture).await;
    adoption_policy(&fixture.client, &fixture.name, full.id.as_str(), 0, true).await;
    let store = fixture.store.clone();
    let now = SystemTime::now();
    let expires = now + Duration::from_secs(60);
    let claimed = store
        .claim_agent_work(
            &full.id,
            "open-worker",
            rfc3339_at(now),
            rfc3339_at(expires),
        )
        .await
        .unwrap()
        .unwrap();
    let mut parents = fixture.context.publication_parents();
    parents.worktree_revision = store
        .get_worktree(&full.worktree_id)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let original = fixture
        .worker
        .progress_mutation(
            claimed.clone(),
            AgentWorkStep::DispatchFirstMessage,
            now,
            None,
            Vec::new(),
            None,
            None,
        )
        .unwrap();
    let barrier = adoption_probe::arm(&fixture.name, Boundary::BeforeCommit);
    let attempted = tokio::spawn({
        let store = store.clone();
        let proposal = original.clone();
        async move { store.commit_agent_connection_work(proposal, parents).await }
    });
    tokio::time::timeout(Duration::from_secs(58), barrier.reached.notified())
        .await
        .unwrap();
    let off = MongoSandboxStore::open(&fixture.client, &fixture.name)
        .await
        .unwrap();
    let on = off.clone().with_segment_adoption_enabled(true);
    assert!(replacement_claim(&on, &full).await.is_none());
    tokio::time::sleep(
        expires
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            + Duration::from_secs(1),
    )
    .await;
    barrier.resume.notify_one();
    assert!(attempted.await.unwrap().is_err());
    assert_eq!(current(&fixture).await, claimed);
    adoption_policy(&fixture.client, &fixture.name, full.id.as_str(), 1, false).await;
    assert!(replacement_claim(&off, &full).await.is_none());
    assert!(replacement_claim(&on, &full).await.is_none());
    adoption_policy(&fixture.client, &fixture.name, full.id.as_str(), 2, true).await;
    let replacement = replacement_claim(&on, &full).await.unwrap();
    assert!(replacement.work.lease.as_ref().unwrap().fence > original.fence);
    assert!(on
        .commit_agent_connection_work(original, parents)
        .await
        .is_err());
    // The registered worker's original operation and first input still bind
    // this storage proposal. Only its claim authority and timestamp change.
    let next = fixture
        .worker
        .progress_mutation(
            replacement,
            AgentWorkStep::DispatchFirstMessage,
            SystemTime::now(),
            None,
            Vec::new(),
            None,
            None,
        )
        .unwrap();
    on.commit_agent_connection_work(next, parents)
        .await
        .unwrap();
    check_publication(&fixture, &full, 1).await;
    assert_eq!(
        fixture
            .client
            .database(&fixture.name)
            .collection::<Document>("agent_log_publications")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    fixture.cleanup().await;
}

async fn replacement_claim(store: &MongoSandboxStore, agent: &Agent) -> Option<Agent> {
    let now = SystemTime::now();
    store
        .claim_agent_work(
            &agent.id,
            "open-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(300)),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn restarted_ready_replay_requires_original_sandbox_event_after_later_writes() {
    let fixture = Fixture::with_adoption(None, true).await;
    let request = layer3_adapter::AgentOpenRequest::new(
        format!("agent:{}:open:v1", fixture.agent.id),
        crate::sandbox::runtime_options(
            &fixture.agent,
            &fixture.context.sandbox,
            &fixture.context.worktree,
        ),
        None,
    );
    fixture
        .worker
        .retain_completed_open_after_conflict(
            &fixture.agent,
            &fixture.context.sandbox,
            &request,
            &fixture.attachment,
            at(),
        )
        .await
        .unwrap();
    let original = current(&fixture).await;
    fill_count_boundary(&fixture, &original).await;
    adoption_policy(
        &fixture.client,
        &fixture.name,
        original.id.as_str(),
        0,
        true,
    )
    .await;
    let claimed = replacement_claim(&fixture.store, &original).await.unwrap();
    let proposal = ready_proposal(&fixture, claimed.clone());
    let parents = fixture.context.publication_parents();
    fixture
        .store
        .commit_agent_connection_work(proposal.clone(), parents)
        .await
        .unwrap();
    check_publication(&fixture, &claimed, 2).await;
    adoption_policy(
        &fixture.client,
        &fixture.name,
        claimed.id.as_str(),
        1,
        false,
    )
    .await;
    let reopened = MongoSandboxStore::open(&fixture.client, &fixture.name)
        .await
        .unwrap();
    reopened
        .append_agent_log_keyed(
            &claimed.id,
            &AgentLogPublicationIdentity {
                key: "after-original-ready".into(),
                fingerprint: [2; 32],
                appended_at: rfc3339_at(SystemTime::now()),
            },
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
    reopened
        .commit_agent_connection_work(proposal.clone(), parents)
        .await
        .unwrap();
    let db = fixture.client.database(&fixture.name);
    let witness = db
        .collection::<Document>("agents")
        .find_one(doc! {"_id":claimed.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let seq = witness
        .get_document("pending_worker_log_adoption_v1")
        .unwrap()
        .get_i64("sandbox_event_seq")
        .unwrap();
    let events = db.collection::<Document>("sandbox_events");
    let filter = doc! {"sandbox_id":claimed.sandbox_id.as_str(), "seq":seq};
    let row = events.find_one(filter.clone()).await.unwrap().unwrap();
    assert_eq!(events.delete_one(filter).await.unwrap().deleted_count, 1);
    assert!(reopened
        .commit_agent_connection_work(proposal.clone(), parents)
        .await
        .is_err());
    let mut changed = row;
    changed
        .get_document_mut("record")
        .unwrap()
        .insert("appended_at", "2026-09-05T00:03:00Z");
    events.insert_one(changed).await.unwrap();
    assert!(reopened
        .commit_agent_connection_work(proposal, parents)
        .await
        .is_err());
    fixture.cleanup().await;
}

fn ready_proposal(fixture: &Fixture, claimed: Agent) -> AgentWorkMutation {
    let target = target(fixture);
    let now = SystemTime::now();
    let stamp = rfc3339_at(now);
    let mut worktree = fixture.context.worktree.clone();
    worktree.transition(WorktreeStatus::Ready).unwrap();
    let logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: target.conversation_id,
            runtime_generation: target.runtime_generation,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    let event = SandboxEventKind::AgentStatusChanged {
        agent_id: claimed.id.clone(),
        from: AgentStatus::Starting,
        to: AgentStatus::Ready,
    };
    let mut proposal = fixture
        .worker
        .progress_mutation(
            claimed,
            AgentWorkStep::PublishConnection,
            now,
            None,
            logs,
            Some(VersionedWorktree {
                expected_revision: fixture.context.worktree.revision,
                worktree,
            }),
            Some(event),
        )
        .unwrap();
    proposal.agent.revision -= 1;
    proposal
        .agent
        .transition(AgentStatus::Ready, stamp, None)
        .unwrap();
    proposal
}
