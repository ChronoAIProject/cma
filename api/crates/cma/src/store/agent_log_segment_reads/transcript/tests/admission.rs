use super::*;
use layer2_core::sandbox::{store::AgentAdmission, Worktree, WorktreeStatus};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_creation_replay_reads_original_creation_without_lifetime_scan() {
    for closed in [1, 10, 100, 1000] {
        let (_, db, store, agent, entries) = fixture(closed, false).await;
        let admission = AgentAdmission {
            binding: None,
            created_by: None,
            owner: agent.owner.clone(),
            workspace_id: agent.workspace_id.clone(),
            agent_id: agent.id.clone(),
            worktree_id: agent.worktree_id.clone(),
            profile: agent.profile.clone(),
            capabilities: agent.capabilities.clone(),
            title: None,
            first_message: None,
            origin: agent.origin.clone(),
            creation_key: "replayed-creation".into(),
            event_at: NOW.into(),
        };
        let worktree = Worktree {
            id: agent.worktree_id.clone(),
            workspace_id: agent.workspace_id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            owner: agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/ws/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(agent.id.clone()),
            revision: 1,
        };
        store
            .worktrees
            .insert_one(crate::store::worktrees::worktree_document(&worktree).unwrap())
            .await
            .unwrap();
        store.agent_reuse_bindings.insert_one(doc! {
            "_id":crate::store::agent_admission_records::binding_id(&admission).unwrap(),
            "document_schema_version":1_i64,"owner":agent.owner.as_str(),"creation_key":&admission.creation_key,
            "fingerprint":admission.fingerprint().unwrap(),"agent_id":agent.id.as_str(),"sandbox_id":agent.sandbox_id.as_str(),
        }).await.unwrap();
        let measured = crate::store::mongo_read_telemetry::isolated_test(async {
            let mut session = store.client.start_session().await.unwrap();
            crate::store::sandbox_mongo::start_transaction(&mut session)
                .await
                .unwrap();
            let receipt = match crate::store::agent_admission_records::replay_in_transaction(
                &store,
                &admission,
                &mut session,
            )
            .await
            {
                Ok(Some(receipt)) => receipt,
                _ => panic!("segmented creation replay failed"),
            };
            assert_eq!(receipt.result().creation().initial_log, entries[0]);
            session.abort_transaction().await.unwrap();
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert!(measured[2] <= 20, "bounded originals: {measured:?}");
        assert!(measured[3] < 32 * 1024, "bounded BSON: {measured:?}");
        store
            .agent_log
            .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":1_i64})
            .await
            .unwrap();
        let mut session = store.client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        assert!(
            crate::store::agent_admission_records::replay_in_transaction(
                &store,
                &admission,
                &mut session
            )
            .await
            .is_err()
        );
        session.abort_transaction().await.unwrap();
        db.drop().await.unwrap();
    }
}
