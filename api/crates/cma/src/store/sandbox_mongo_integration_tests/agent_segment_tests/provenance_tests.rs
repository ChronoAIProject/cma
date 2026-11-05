use super::*;
use layer2_core::sandbox::store::AgentProvenanceStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn provenance_verifies_original_creation_bson_and_refuses_missing_segment_state() {
    let (client, name) = database("segment_provenance").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_segments");
    let (worktree, _) = worktree_and_agent(&parent.id);
    let workspace = ready_workspace(&parent, worktree.workspace_id.as_str(), "project");
    store.create_sandbox(parent).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    for oversized in [false, true] {
        let filter = doc! {"_id":agent.id.as_str()};
        if oversized {
            // Force the original aggregation's sequential fallback without
            // changing any decoded Agent or source identity.
            store
                .agents
                .update_one(filter.clone(), doc! {"$set":{"padding":"x".repeat(70_000)}})
                .await
                .unwrap();
        }
        let actual = store.agent_provenance_snapshot(&agent.id).await.unwrap();
        assert_eq!(actual.first_log, Some(entries[0].clone()));
        let source_filter = doc! {"agent_id":agent.id.as_str(),"seq":1_i64};
        let original = store
            .agent_log
            .find_one(source_filter.clone())
            .await
            .unwrap()
            .unwrap();
        store
            .agent_log
            .update_one(
                source_filter.clone(),
                doc! {"$set":{"record.appended_at":"2026-09-06T00:00:00Z"}},
            )
            .await
            .unwrap();
        assert!(matches!(
            store.agent_provenance_snapshot(&agent.id).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        store
            .agent_log
            .delete_one(source_filter.clone())
            .await
            .unwrap();
        assert!(matches!(
            store.agent_provenance_snapshot(&agent.id).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        store.agent_log.insert_one(original).await.unwrap();
        let head = store
            .segment_logs
            .heads
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        store.segment_logs.heads.delete_one(filter).await.unwrap();
        assert!(matches!(
            store.agent_provenance_snapshot(&agent.id).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        store.segment_logs.heads.insert_one(head).await.unwrap();
    }
    client.database(&name).drop().await.unwrap();
}
