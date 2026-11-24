use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use crate::store::sandbox_mongo::start_transaction;
use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use layer2_core::sandbox::store::{AgentStore, SandboxStore};

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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn preparation_quarantine_conflicts_with_hold_installed_after_snapshot() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let independent = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_quarantine_hold_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let other = MongoSandboxStore::open(&independent, &name).await.unwrap();
    let parent = active_sandbox("sbx_quarantine_hold");
    let agent = ready_agent(&parent, "agt_quarantine_hold");
    store.create_sandbox(parent.clone()).await.unwrap();
    crate::store::sandbox_maintenance::mongo_integration_tests::insert_agent(
        &store,
        &agent,
        "2026-01-01T00:00:00Z",
    )
    .await;

    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let agent_key = doc! {"_id": agent.id.as_str()};
    let parent_key = doc! {"_id": parent.id.as_str()};
    let observed = store
        .agents
        .find_one(agent_key.clone())
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let before = store
        .sandboxes
        .find_one(parent_key.clone())
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    assert!(!before.contains_key(LEDGER_MAINTENANCE_FIELD));
    other
        .sandboxes
        .update_one(
            parent_key.clone(),
            doc! {
                "$set": {LEDGER_MAINTENANCE_FIELD: Bson::Null}
            },
        )
        .await
        .unwrap();
    let held = other.sandboxes.find_one(parent_key.clone()).await.unwrap();
    assert!(
        proof::quarantine_preparation(&store, &observed, &mut session)
            .await
            .is_err()
    );
    let _ = session.abort_transaction().await;
    // A retained observation from before the hold also reaches the independent
    // ordinary audit path after its original snapshot has been released.
    proof::quarantine_row(&store, &observed).await.unwrap();
    assert_eq!(
        store.agents.find_one(agent_key.clone()).await.unwrap(),
        Some(observed.clone())
    );
    assert_eq!(
        store.sandboxes.find_one(parent_key.clone()).await.unwrap(),
        held
    );
    other
        .sandboxes
        .update_one(parent_key, doc! {"$unset": {LEDGER_MAINTENANCE_FIELD: ""}})
        .await
        .unwrap();
    proof::quarantine_row(&store, &observed).await.unwrap();
    assert!(store
        .agents
        .find_one(agent_key)
        .await
        .unwrap()
        .unwrap()
        .contains_key(QUARANTINE_FIELD));
    client.database(&name).drop().await.unwrap();
}
