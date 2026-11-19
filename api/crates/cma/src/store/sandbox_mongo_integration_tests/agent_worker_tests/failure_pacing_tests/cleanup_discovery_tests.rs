use super::ownership_retry_wake_tests::fixture;
use super::*;
use layer2_core::sandbox::{AgentStatus, AgentStopReason};
use std::collections::BTreeSet;

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn cleanup_holds_preserve_healthy_work_and_controls_behind_a_full_page() {
    let (client, name, store, template, _authority, _) = fixture("cleanup_discovery").await;
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let candidate = |id: String| {
        let mut agent = template.clone();
        agent.id = AgentId(id.clone());
        agent.creation_key = id.clone();
        agent.worktree_id = WorktreeId(format!("{id}-tree"));
        agent.work.lease = None;
        agent
    };
    let mut held = BTreeSet::new();
    let mut rows = Vec::new();
    let until = layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(120));
    for index in 0..200 {
        let agent = candidate(format!("000-held-{index:03}"));
        held.insert(agent.id.clone());
        let mut row = crate::store::agents::agent_document(&agent).unwrap();
        // Stale parent revisions make these due through the wakeup arm of the union.
        if index % 2 == 0 {
            row.insert(FIELD, doc! {"class": CLASS, "agent_revision": agent.revision as i64,
                "command_admission_generation": 0_i64, "attempts": agent.work.attempts, "repeats": 0,
                "sandbox_revision": 0_i64, "workspace_revision": 0_i64, "worktree_revision": 0_i64,
                "ready_at": &agent.work.ready_at, "until": &until});
        }
        rows.push(row);
    }
    let healthy = candidate("001-healthy".into());
    let mut stop = candidate("002-stop".into());
    stop.request_stop(AgentStopReason::Owner, now.clone())
        .unwrap();
    let mut delete = candidate("003-delete".into());
    delete.status = AgentStatus::Stopped;
    delete.request_delete(now.clone()).unwrap();
    held.extend([stop.id.clone(), delete.id.clone()]);
    for agent in [&healthy, &stop, &delete] {
        rows.push(crate::store::agents::agent_document(agent).unwrap());
    }
    store.agents.insert_many(rows).await.unwrap();
    assert_eq!(
        store
            .due_agent_work_with_cleanup_holds(&now, 1, &held)
            .await
            .unwrap(),
        vec![healthy.id.clone()]
    );
    assert_eq!(
        store
            .due_agent_work_with_cleanup_holds(&now, 3, &held)
            .await
            .unwrap(),
        vec![healthy.id, stop.id, delete.id]
    );
    assert_eq!(
        store.due_agent_work(&now, 1).await.unwrap(),
        vec![AgentId("000-held-000".into())]
    );
    client.database(&name).drop().await.unwrap();
}
