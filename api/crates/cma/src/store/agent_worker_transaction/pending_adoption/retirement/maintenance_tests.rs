use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, insert_agent, ready_agent,
};
use layer2_core::sandbox::{store::SandboxStore, AgentWorkState};
use mongodb::{bson::Bson, options::WriteConcern, Client};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn expired_pending_witness_remains_admitted_under_a_held_sandbox() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let independent = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_pending_expiry_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = active_sandbox("sbx_pending_expiry");
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut agent = ready_agent(&parent, "agt_pending_expiry");
    let ready = "2000-01-01T00:00:00Z";
    let expired = "2000-01-01T00:00:01Z";
    agent.work = AgentWorkState::new(AgentWorkStep::PublishConnection, ready.into());
    agent
        .work
        .claim("worker".into(), ready, expired.into())
        .unwrap();
    insert_agent(&store, &agent, &agent.updated_at).await;
    let fence = agent.work.lease.as_ref().unwrap().fence;
    let mutation = AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: "worker".into(),
        fence,
        agent: agent.clone(),
        worktree: None,
        appended_at: agent.updated_at.clone(),
        sandbox_event: None,
        agent_logs: vec![],
    };
    let operation = PublicationIdentity {
        key: format!("{PENDING_WORKER_PREFIX}{}:{fence}", agent.revision),
        fingerprint: publication::request_digest(&"retained request").unwrap(),
        appended_at: mutation.appended_at.clone(),
    };
    let lineage = publication::request_digest(&"retained lineage").unwrap();
    let marker = Witness::new(&agent, &mutation, lineage, operation)
        .unwrap_or_else(|_| panic!("construct valid retained witness"));
    let agent_key = doc! {"_id": agent.id.as_str()};
    store.agents.update_one(agent_key.clone(), doc! {
        "$set": {witness::FIELD: marker.document().unwrap_or_else(|_| panic!("encode retained witness"))}
    }).await.unwrap();
    let original = store
        .agents
        .find_one(agent_key.clone())
        .await
        .unwrap()
        .unwrap();
    let sandboxes = independent
        .database(&name)
        .collection::<Document>("sandboxes");
    let sandbox_key = doc! {"_id": parent.id.as_str()};

    for hold in [Bson::Null, Bson::Document(doc! {"phase":"requested"})] {
        let installed = sandboxes
            .update_one(
                sandbox_key.clone(),
                doc! {
                    "$set": {"ledger_maintenance": hold}
                },
            )
            .write_concern(WriteConcern::majority())
            .await
            .unwrap();
        assert_eq!(installed.modified_count, 1);
        let held = sandboxes.find_one(sandbox_key.clone()).await.unwrap();
        assert!(!retire_expired(&store, &original, &agent, expired)
            .await
            .unwrap());
        assert_eq!(
            store.agents.find_one(agent_key.clone()).await.unwrap(),
            Some(original.clone())
        );
        assert_eq!(sandboxes.find_one(sandbox_key.clone()).await.unwrap(), held);
    }
    sandboxes
        .update_one(sandbox_key, doc! {"$unset": {"ledger_maintenance": ""}})
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();
    assert!(retire_expired(&store, &original, &agent, expired)
        .await
        .unwrap());
    let row = store.agents.find_one(agent_key).await.unwrap().unwrap();
    let retired = Witness::read(&row)
        .unwrap_or_else(|_| panic!("read retained witness"))
        .unwrap();
    assert_eq!(retired.state, State::Retired);
    client.database(&name).drop().await.unwrap();
}
