use super::*;
use layer2_core::sandbox::store::AGENT_LOG_CAPACITY_FAILURE;
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind, FirstMessageDelivery};
use mongodb::bson::{doc, serialize_to_bson, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_settles_capacity_under_its_new_claim() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    let mut full = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let revision = full.revision;
    full.conversation.first_message = Some(
        FirstMessageRecord::new(&full.creation_key, "retained input", rfc3339_at(at())).unwrap(),
    );
    full.revision += 1;
    fixture
        .store
        .save_agent(revision, full.clone())
        .await
        .unwrap();
    let entries: Vec<_> = (full.conversation.last_seq + 1..=10_000)
        .map(|seq| {
            let entry = AgentLogEntry {
                agent_id: full.id.clone(),
                seq,
                appended_at: full.updated_at.clone(),
                kind: AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                },
            };
            doc! {"_id":format!("{}:{seq:020}", full.id), "document_schema_version":1_i64,
            "agent_id":full.id.as_str(), "seq":seq as i64, "record":serialize_to_bson(&entry).unwrap()}
        })
        .collect();
    let database = fixture.client.database(&fixture.name);
    database
        .collection::<Document>("agent_log")
        .insert_many(entries)
        .await
        .unwrap();
    let result = database
        .collection::<Document>("agents")
        .update_one(
            doc! {"_id":full.id.as_str(), "revision":full.revision as i64},
            doc! {"$set":{"last_seq":10_000_i64, "record.conversation.last_seq":10_000_i64}},
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);

    finish(&fixture, &fixture.context).await;
    let failed = fixture.store.get_agent(&full.id).await.unwrap().unwrap();
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        AGENT_LOG_CAPACITY_FAILURE
    );
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert!(failed.work.lease.is_none());
    assert_eq!(failed.work.next_fence, fixture.agent.work.next_fence + 1);
    assert_eq!(failed.work.receipts, full.work.receipts);
    assert_eq!(failed.work.cleanup_debt, full.work.cleanup_debt);
    assert_eq!(
        failed.conversation.conversation_id,
        full.conversation.conversation_id
    );
    assert_eq!(
        failed.conversation.runtime_generation,
        full.conversation.runtime_generation
    );
    assert_eq!(failed.conversation.last_seq, 10_001);
    let first = failed.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(
        first.text,
        full.conversation.first_message.as_ref().unwrap().text
    );
    assert_eq!(first.attempt, 0);
    assert_eq!(fixture.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .target_is_attached(&target(&fixture)));
    let reopened = MongoSandboxStore::open(&fixture.client, &fixture.name)
        .await
        .unwrap();
    assert_eq!(reopened.get_agent(&full.id).await.unwrap(), Some(failed));
    fixture.cleanup().await;
}
