use super::*;
use mongodb::{
    bson::{Bson, Document},
    options::WriteConcern,
    Client,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_hold_refuses_a_fresh_quiet_checkpoint() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_hold").await;
    let first = partial(&current, "durable first chunk");
    let next = partial(&current, "durable first chunk and another");
    store
        .commit_agent_message_update(current.clone(), None, first.clone(), None)
        .await
        .unwrap();

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let other = Client::with_uri_str(&uri).await.unwrap();
    let sandboxes = other.database(&name).collection::<Document>("sandboxes");
    let sandbox_key = doc! {"_id": current.sandbox_id.as_str()};
    let agent_key = doc! {"_id": current.id.as_str()};
    let agent = store.agents.find_one(agent_key.clone()).await.unwrap();
    let logs = store.agent_log(&current.id, 0, 100).await.unwrap();
    for hold in [Bson::Null, Bson::Document(doc! {"phase": "requested"})] {
        assert_eq!(
            sandboxes
                .update_one(
                    sandbox_key.clone(),
                    doc! {
                        "$set": {"ledger_maintenance": hold}
                    }
                )
                .write_concern(WriteConcern::majority())
                .await
                .unwrap()
                .modified_count,
            1
        );
        let held = sandboxes.find_one(sandbox_key.clone()).await.unwrap();
        assert_eq!(
            store
                .commit_agent_message_update(
                    current.clone(),
                    Some(first.clone()),
                    next.clone(),
                    None
                )
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.agents.find_one(agent_key.clone()).await.unwrap(),
            agent
        );
        assert_eq!(sandboxes.find_one(sandbox_key.clone()).await.unwrap(), held);
        assert_eq!(store.agent_log(&current.id, 0, 100).await.unwrap(), logs);
    }
    sandboxes
        .update_one(sandbox_key, doc! {"$unset": {"ledger_maintenance": ""}})
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_agent_message_update(current.clone(), Some(first), next.clone(), None)
            .await
            .unwrap(),
        next.outcome
    );
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(next)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_hold_refuses_output_log_publication() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_output_publication_hold").await;
    let mutation = update(&current, "interrupted Agent output");
    let output = boundary(&mutation);
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let other = Client::with_uri_str(&uri).await.unwrap();
    let sandboxes = other.database(&name).collection::<Document>("sandboxes");
    let sandbox_key = doc! {"_id": current.sandbox_id.as_str()};
    let agent_key = doc! {"_id": current.id.as_str()};
    let original_agent = store.agents.find_one(agent_key.clone()).await.unwrap();
    let original_logs = store.agent_log(&current.id, 0, 100).await.unwrap();
    let installed = sandboxes
        .update_one(
            sandbox_key.clone(),
            doc! {
                "$set": {"ledger_maintenance": {"phase": "requested"}}
            },
        )
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();
    assert_eq!(installed.modified_count, 1);
    let held = sandboxes.find_one(sandbox_key.clone()).await.unwrap();
    assert_eq!(
        store
            .commit_agent_message_update(
                current.clone(),
                None,
                output.clone(),
                Some(mutation.clone()),
            )
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.commit_agent_conversation(mutation.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.agents.find_one(agent_key).await.unwrap(),
        original_agent
    );
    assert_eq!(
        store.agent_log(&current.id, 0, 100).await.unwrap(),
        original_logs
    );
    assert_eq!(sandboxes.find_one(sandbox_key.clone()).await.unwrap(), held);
    sandboxes
        .update_one(sandbox_key, doc! {"$unset": {"ledger_maintenance": ""}})
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_agent_message_update(current.clone(), None, output.clone(), Some(mutation))
            .await
            .unwrap(),
        output.outcome
    );
    let after = store.agent_log(&current.id, 0, 100).await.unwrap();
    assert_eq!(
        &after[original_logs.len()..],
        output.outcome.appended.as_slice()
    );
    client.database(&name).drop().await.unwrap();
}
