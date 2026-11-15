use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_rejects_missing_gapped_and_misaligned_live_logs_without_mutation() {
    let (client, name, store) = database().await;
    for scenario in ["missing", "gap", "stale_head", "identity"] {
        let (sandbox, agent) = fixture(&store, &format!("sbx_{scenario}")).await;
        match scenario {
            "missing" => {
                store
                    .agent_log
                    .delete_one(doc! { "agent_id": agent.id.as_str() })
                    .await
                    .unwrap();
            }
            "gap" => {
                let entry = AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: 3,
                    appended_at: IDLE_AT.into(),
                    kind: AgentLogKind::Deleted,
                };
                store
                    .agent_log
                    .insert_one(log_document(&entry).unwrap())
                    .await
                    .unwrap();
            }
            "stale_head" => {
                store.agents.update_one(doc! { "_id": agent.id.as_str() },
                    doc! { "$set": { "last_seq": 2_i64, "record.conversation.last_seq": 2_i64 } })
                    .await.unwrap();
            }
            "identity" => {
                store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": agent.id.as_str() },
                        doc! { "$set": { "record.agent_id": "agt_someone_else" } },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let result = store.schedule_inactivity_pause(pause(&sandbox.id)).await;
        assert!(matches!(
            result,
            Err(SandboxStoreError::SequenceConflict | SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        assert!(store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("sandbox_maintenance_fence"));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_rejects_corrupt_event_heads_and_rolls_back_all_rows() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_event_gap").await;
    let event = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 2,
        appended_at: IDLE_AT.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&event).unwrap())
        .await
        .unwrap();
    store
        .sandboxes
        .update_one(
            doc! { "_id": sandbox.id.as_str() },
            doc! { "$set": { "last_seq": 2_i64 } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.schedule_inactivity_pause(pause(&sandbox.id)).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! { "sandbox_id": sandbox.id.as_str() })
            .await
            .unwrap(),
        1
    );
    let document = store
        .sandbox_events
        .find_one(doc! { "sandbox_id": sandbox.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::store::sandbox_events::read_event(&document).unwrap(),
        event
    );
    client.database(&name).drop().await.unwrap();
}
