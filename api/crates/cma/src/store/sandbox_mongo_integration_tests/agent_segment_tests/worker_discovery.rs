//! Actual gated adoption must not hide an Agent from restart discovery.
use super::retained_assessment::{fixture, ready_source};
use super::*;

async fn finish_legacy_connection(store: &MongoSandboxStore, agent: Agent) -> Agent {
    assert_eq!(agent.work.step, AgentWorkStep::PublishConnection);
    let mut claimed = store
        .claim_agent_work(&agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let expected_agent_revision = claimed.revision;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    claimed
        .work
        .advance("worker", fence, AgentWorkStep::None, NOW.into())
        .unwrap();
    claimed.updated_at = NOW.into();
    claimed.revision = expected_agent_revision + 1;
    claimed.conversation.last_seq += 1;
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision,
            holder: "worker".into(),
            fence,
            agent: claimed,
            worktree: None,
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection,
            }],
        })
        .await
        .unwrap();
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn live_discovery_pages_legacy_and_adopted_agents_without_rewriting_them() {
    let (client, name, store, adopted) = fixture("discovery_adopted").await;
    let legacy =
        finish_legacy_connection(&store, ready_source(&store, "discovery_legacy").await).await;
    store
        .agents
        .update_one(
            doc! {"_id": adopted.id.as_str()},
            doc! {"$set": {"agent_command_admission_fence": 17_i64}},
        )
        .await
        .unwrap();
    let original = store
        .agents
        .find_one(doc! {"_id": adopted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .agent_command_admission_generation(&adopted.id)
            .await
            .unwrap(),
        Some(17)
    );
    assert_eq!(
        store
            .agent_command_admission_generation(&legacy.id)
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        store
            .agent_command_admission_generation(&AgentId("absent-discovery-agent".into()))
            .await
            .unwrap(),
        None
    );
    let legacy_original = store
        .agents
        .find_one(doc! {"_id": legacy.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let page = store
            .live_agent_connections(&PageRequest::new(1, cursor).unwrap())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let candidate = &page.items[0];
        // Storage format must not grant recovery while ordinary connection
        // publication is still pending. A completed legacy connection is idle.
        if candidate.agent_id == adopted.id {
            assert!(!candidate.recovery_eligible);
            assert_eq!(candidate.work_step, AgentWorkStep::PublishConnection);
        } else {
            assert_eq!(candidate.agent_id, legacy.id);
            assert!(candidate.recovery_eligible);
            assert_eq!(candidate.work_step, AgentWorkStep::None);
        }
        assert_eq!(candidate.runtime_generation, 1);
        seen.push(candidate.agent_id.clone());
        assert!(seen.len() <= 2, "discovery must advance its cursor");
        let Some(next) = page.next else { break };
        cursor = Some(next);
    }
    let mut expected = vec![adopted.id.clone(), legacy.id.clone()];
    expected.sort();
    assert_eq!(seen, expected);
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": adopted.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": legacy.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        legacy_original
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn discovery_and_generation_hint_refuse_a_malformed_segmented_envelope() {
    let (client, name, store, agent) = fixture("discovery_corrupt").await;
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"agent_log_incarnation": "invalid"}},
        )
        .await
        .unwrap();
    let original = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        store.agent_command_admission_generation(&agent.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        store
            .live_agent_connections(&PageRequest::new(1, None).unwrap())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        original
    );
    client.database(&name).drop().await.unwrap();
}
