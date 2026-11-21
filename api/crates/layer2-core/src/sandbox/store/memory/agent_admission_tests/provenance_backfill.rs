use super::*;
use crate::sandbox::store::{
    AgentProvenanceBackfillStore, AgentProvenanceStore, PageRequest, ProvenanceBackfillOutcome,
};
use crate::sandbox::AgentCreationOrigin;

#[tokio::test]
async fn provenance_backfill_captures_proved_origin_without_changing_retained_work() {
    let store = ready_store(3).await;
    let mut request = admission("unused");
    request.creation_key = format!("manual:{}", request.agent_id);
    request.created_by = Some(crate::resource_access::ResourceActor::Human {
        subject: request.owner.clone(),
    });
    let first = created(store.admit_agent(request).await.unwrap());
    store
        .lock()
        .agent_creation_provenance
        .remove(&first.agent.id);
    let before = store.lock().clone();
    assert_eq!(
        store
            .backfill_agent_provenance(&first.agent.id)
            .await
            .unwrap(),
        ProvenanceBackfillOutcome::Recorded
    );
    assert_eq!(
        store
            .backfill_agent_provenance(&first.agent.id)
            .await
            .unwrap(),
        ProvenanceBackfillOutcome::Retained
    );
    let facts = store
        .agent_provenance_snapshot(&first.agent.id)
        .await
        .unwrap()
        .creation
        .unwrap();
    assert_eq!(facts.creation_origin, AgentCreationOrigin::Manual);
    assert_eq!(facts.created_by, None);
    let state = store.lock();
    assert_eq!(state.agents, before.agents);
    assert_eq!(state.worktrees, before.worktrees);
    assert_eq!(state.workspaces, before.workspaces);
    assert_eq!(state.sandboxes, before.sandboxes);
    assert_eq!(state.agent_logs, before.agent_logs);
    assert_eq!(state.sandbox_events, before.sandbox_events);
    assert_eq!(state.resource_policies, before.resource_policies);
    assert_eq!(state.resource_shares, before.resource_shares);
}

#[tokio::test]
async fn provenance_backfill_preserves_unknown_and_refuses_inconsistent_evidence() {
    let store = ready_store(3).await;
    let first = created(store.admit_agent(admission("legacy-import")).await.unwrap());
    store
        .lock()
        .agent_creation_provenance
        .remove(&first.agent.id);
    assert_eq!(
        store
            .backfill_agent_provenance(&first.agent.id)
            .await
            .unwrap(),
        ProvenanceBackfillOutcome::Recorded
    );
    let facts = store
        .agent_provenance_snapshot(&first.agent.id)
        .await
        .unwrap()
        .creation
        .unwrap();
    assert_eq!(facts.creation_origin, AgentCreationOrigin::LegacyUnknown);
    let second = created(
        store
            .admit_agent(admission("legacy-corrupt"))
            .await
            .unwrap(),
    );
    {
        let mut state = store.lock();
        state.agent_creation_provenance.remove(&second.agent.id);
        state.agent_logs.get_mut(&second.agent.id).unwrap()[0].appended_at =
            "2026-09-06T00:00:00Z".into();
    }
    assert!(matches!(
        store.backfill_agent_provenance(&second.agent.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(!store
        .lock()
        .agent_creation_provenance
        .contains_key(&second.agent.id));
    assert_eq!(
        store
            .backfill_agent_provenance(&AgentId("missing".into()))
            .await
            .unwrap_err(),
        SandboxStoreError::NotFound
    );
    assert_eq!(
        store
            .agent_provenance_snapshot(&first.agent.id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        facts
    );
}

#[tokio::test]
async fn provenance_backfill_scan_is_bounded_and_can_cross_refused_rows() {
    let store = ready_store(3).await;
    let mut ids = Vec::new();
    for key in ["a", "b", "c"] {
        ids.push(
            created(store.admit_agent(admission(key)).await.unwrap())
                .agent
                .id,
        );
    }
    ids.sort();
    let first = store
        .scan_agent_provenance_backfill(&PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items, ids[..2]);
    assert_eq!(first.next.as_deref(), Some(ids[1].as_str()));
    let second = store
        .scan_agent_provenance_backfill(&PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items, ids[2..]);
    assert!(second.next.is_none());
}
