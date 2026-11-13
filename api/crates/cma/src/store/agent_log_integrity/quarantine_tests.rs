use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use layer2_core::sandbox::store::AgentStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn quarantine_survives_a_revision_race_but_does_not_cross_incarnations() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_quarantine_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = ready_agent(&active_sandbox("sbx_quarantine"), "agt_quarantine");
    agent.conversation.last_seq = 0;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    store.create_agent(agent.clone()).await.unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    assert!(proof::quarantine_row(&store, &row).await.is_ok());
    let current = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(current.contains_key(QUARANTINE_FIELD));
    // Simulate a different creation incarnation under the same id. A delayed
    // observation is not allowed to invalidate that separate history.
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! {
                "$set": { "record.creation_key": "different-incarnation" },
                "$unset": { QUARANTINE_FIELD: "" }
            },
        )
        .await
        .unwrap();
    assert!(proof::quarantine_row(&store, &row).await.is_err());
    let replaced = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!replaced.contains_key(QUARANTINE_FIELD));
    client.database(&name).drop().await.unwrap();
}
