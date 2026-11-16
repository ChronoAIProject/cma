//! Prepared admission must publish under the supported ordinary-work envelopes.
use super::*;
use futures_util::FutureExt;
use std::{future::IntoFuture, panic::AssertUnwindSafe, time::Duration};

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica-set database"]
async fn prepared_replay_accepts_renewed_and_installation_parent_envelopes() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = std::env::var("CHRONO_TEST_STARTUP_ABORT_DATABASE")
        .unwrap_or_else(|_| format!("cma_startup_abort_{}", uuid::Uuid::new_v4().simple()));
    assert!(name.starts_with("cma_startup_abort_") && name.len() < 64);
    assert!(name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'));
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&name);
    assert!(
        database.list_collection_names().await.unwrap().is_empty(),
        "refuse preexisting data before taking cleanup ownership"
    );
    let result = tokio::time::timeout(
        Duration::from_secs(90),
        AssertUnwindSafe(exercise(&client, &name)).catch_unwind(),
    )
    .await;
    let cleanup =
        tokio::time::timeout(Duration::from_secs(10), database.drop().into_future()).await;
    assert!(
        matches!(cleanup, Ok(Ok(()))),
        "owned test database cleanup failed"
    );
    eprintln!("prepared_schema_database_cleanup=PASS");
    match result {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("prepared schema regression timed out"),
    }
}

async fn exercise(client: &mongodb::Client, name: &str) {
    let mut failed = Vec::new();
    for (label, sandbox_claim, workspace_claim, proof, expected) in [
        ("legacy", false, false, false, (1_i64, 1_i64)),
        ("renewed_sandbox", true, false, false, (3, 1)),
        ("renewed_workspace", false, true, false, (1, 3)),
        ("installation", false, false, true, (4, 1)),
        ("production_combination", true, true, true, (4, 3)),
    ] {
        let store = MongoSandboxStore::open(client, name).await.unwrap();
        let (agent, parent, workspace, tree) = agent_replay_tests::hierarchy_with_claims(
            &store,
            sandbox_claim,
            workspace_claim,
            proof,
        )
        .await;
        let parent_row = store
            .sandboxes
            .find_one(doc! {"_id":parent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let workspace_row = store
            .workspaces
            .find_one(doc! {"_id":workspace.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                parent_row.get_i64("document_schema_version").unwrap(),
                workspace_row.get_i64("document_schema_version").unwrap()
            ),
            expected
        );
        assert!(
            store
                .prepared_agent_replay_matches(&agent, &parent, &workspace, &tree)
                .await
                .unwrap(),
            "real claimed preparation must match"
        );

        // A benign parent revision change after the read cannot change eligibility.
        let mut newer = parent.clone();
        newer.revision += 1;
        newer.updated_at = LATER.into();
        store.save_sandbox(parent.revision, newer).await.unwrap();
        let pending = agent_replay_tests::intent(&agent, &parent, &workspace, &tree);
        let mut committed = false;
        let mut attempts = 0;
        for _ in 0..8 {
            attempts += 1;
            match store.commit_agent_work(pending.clone()).await {
                Ok(()) => {
                    committed = true;
                    break;
                }
                Err(SandboxStoreError::Conflict) => {
                    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
                }
                other => panic!("unexpected prepared publication result: {other:?}"),
            }
        }
        eprintln!("prepared_schema case={label} sandbox={} workspace={} eligible=true attempts={attempts} committed={committed}", expected.0, expected.1);
        if !committed {
            failed.push(label);
        } else {
            let done = agent_replay_tests::completion(&pending.agent);
            store.commit_agent_work(done.clone()).await.unwrap();
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap().unwrap(),
                done.agent
            );
            for (collection, id) in [
                (&store.sandboxes, parent.id.as_str()),
                (&store.workspaces, workspace.id.as_str()),
                (&store.worktrees, tree.id.as_str()),
                (&store.native_prepared, parent.id.as_str()),
            ] {
                let row = collection.find_one(doc! {"_id":id}).await.unwrap().unwrap();
                assert_eq!(row.get_i64("agent_prepared_replay_fence").unwrap(), 2);
            }
            assert!(
                matches!(
                    store.commit_agent_work(done).await,
                    Err(SandboxStoreError::Conflict)
                ),
                "duplicate/stale publication must refuse"
            );
        }
        // The next case owns a fresh database namespace with the same scoped name.
        client.database(name).drop().await.unwrap();
    }
    // Real pause and cleanup markers remain barriers even with supported schemas.
    for barrier in ["pause", "cleanup", "unknown_worktree_schema"] {
        let store = MongoSandboxStore::open(client, name).await.unwrap();
        let (agent, parent, workspace, tree) = agent_replay_tests::hierarchy(&store).await;
        let pending = agent_replay_tests::intent(&agent, &parent, &workspace, &tree);
        store.commit_agent_work(pending.clone()).await.unwrap();
        match barrier {
            "pause" => {
                let mut paused = parent.clone();
                paused.begin_pause(LATER.into(), None).unwrap();
                store.save_sandbox(parent.revision, paused).await.unwrap();
            }
            "cleanup" => {
                store
                    .workspaces
                    .update_one(
                        doc! {"_id":workspace.id.as_str()},
                        doc! {"$set":{"owned_workspace_cleanup_admission_v1":true}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                store
                    .worktrees
                    .update_one(
                        doc! {"_id":tree.id.as_str()},
                        doc! {"$set":{"document_schema_version":3_i64}},
                    )
                    .await
                    .unwrap();
            }
        }
        assert!(
            store
                .commit_agent_work(agent_replay_tests::completion(&pending.agent))
                .await
                .is_err(),
            "barrier {barrier} must refuse"
        );
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().unwrap(),
            pending.agent
        );
        eprintln!("prepared_schema barrier={barrier} refused=true");
        client.database(name).drop().await.unwrap();
    }
    assert!(
        failed.is_empty(),
        "prepared publication must accept supported parent envelopes: {failed:?}"
    );
}
