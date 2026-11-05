use super::*;
use crate::store::{agent_log_segments::publication, agent_worker_transaction::PublicationReceipt};

async fn receipt(
    store: &MongoSandboxStore,
    value: &PublicationReceipt,
    expected_seq: i64,
    ordinal: i64,
    revision: i64,
) -> publication::Publication {
    let operation = value.publication.as_ref().unwrap();
    let incarnation = digest::incarnation(&value.agent);
    let previous = if ordinal == 0 {
        None
    } else {
        Some(Seal::read(store.segment_logs.seals.find_one(doc! {"_id":Seal::identity(value.agent.id.as_str(), &incarnation, ordinal - 1)}).await.unwrap().unwrap()).unwrap().seal_digest)
    };
    let row = publication::Publication {
        id: publication::identity(value.agent.id.as_str(), &incarnation, &operation.key),
        format_version: 1,
        agent_id: value.agent.id.0.clone(),
        incarnation,
        publication_key: operation.key.clone(),
        fingerprint: operation.fingerprint.clone(),
        expected_revision: 0,
        expected_seq,
        first_seq: expected_seq + 1,
        last_seq: value.agent.conversation.last_seq as i64,
        result_revision: value.agent.revision as i64,
        head_revision: revision,
        active_ordinal: ordinal,
        previous_seal: previous,
        result_digest: publication::result_digest(&(
            &value.agent,
            &value.worktree,
            &value.logs,
            &value.event,
            &value.sign_in_sandbox,
        ))
        .unwrap(),
    };
    store
        .segment_logs
        .publications
        .insert_one(bson::serialize_to_document(&row).unwrap())
        .await
        .unwrap();
    row
}

async fn head_revision(store: &MongoSandboxStore, agent: &Agent, revision: i64) {
    store
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":{"agent_log_head_revision":revision}},
        )
        .await
        .unwrap();
    store
        .segment_logs
        .heads
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":{"revision":revision}},
        )
        .await
        .unwrap();
    store
        .segment_logs
        .projections
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":{"head_revision":revision}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn immutable_work_receipts_reconcile_after_later_appends_and_empty_metadata_updates() {
    let (client, name) = database("segment_publications").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 100).await;
    head_revision(&store, &agent, 3).await;
    let mut earlier = agent.clone();
    earlier.conversation.last_seq = 4;
    let committed = PublicationReceipt {
        publication: Some(publication::PublicationIdentity {
            key: "fixture-work".into(),
            fingerprint: digest::binary(vec![7; 32]),
        }),
        agent: earlier,
        logs: entries[2..4].to_vec(),
        worktree: None,
        event: None,
        sign_in_sandbox: None,
    };
    let mut record = receipt(&store, &committed, 2, 1, 2).await;
    assert!(
        crate::store::agent_worker_reconcile::reconcile_publication(&store, &committed)
            .await
            .unwrap()
    );
    let conversation = crate::store::agent_conversation_store::ConversationReceipt {
        publication: Some(publication::PublicationIdentity {
            key: "fixture-conversation".into(),
            fingerprint: digest::binary(vec![9; 32]),
        }),
        agent: committed.agent.clone(),
        logs: committed.logs.clone(),
        command_dispatch: None,
    };
    let operation = conversation.publication.as_ref().unwrap();
    record.id = publication::identity(&record.agent_id, &record.incarnation, &operation.key);
    record.publication_key = operation.key.clone();
    record.fingerprint = operation.fingerprint.clone();
    record.result_digest = publication::result_digest(&(
        &conversation.agent,
        &conversation.logs,
        &conversation.command_dispatch,
    ))
    .unwrap();
    store
        .segment_logs
        .publications
        .insert_one(bson::serialize_to_document(&record).unwrap())
        .await
        .unwrap();
    assert!(
        crate::store::agent_conversation_store::reconcile_conversation(&store, &conversation)
            .await
            .unwrap()
    );
    let direct = crate::store::agent_log::AppendReceipt {
        publication: Some(publication::PublicationIdentity {
            key: "fixture-direct".into(),
            fingerprint: digest::binary(vec![10; 32]),
        }),
        agent: committed.agent.clone(),
        entry: committed.logs.last().unwrap().clone(),
    };
    let operation = direct.publication.as_ref().unwrap();
    record.id = publication::identity(&record.agent_id, &record.incarnation, &operation.key);
    record.publication_key = operation.key.clone();
    record.fingerprint = operation.fingerprint.clone();
    record.expected_seq = 3;
    record.first_seq = 4;
    record.result_digest = publication::result_digest(&(&direct.agent, &direct.entry)).unwrap();
    store
        .segment_logs
        .publications
        .insert_one(bson::serialize_to_document(&record).unwrap())
        .await
        .unwrap();
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    assert_eq!(
        crate::store::agent_log::reconcile_append_snapshot(&store, &direct, &mut snapshot)
            .await
            .unwrap(),
        Some(direct.entry.clone())
    );
    let mut different = committed.clone();
    different.publication.as_mut().unwrap().fingerprint.bytes[0] ^= 1;
    assert!(matches!(
        crate::store::agent_worker_reconcile::reconcile_publication(&store, &different).await,
        Err(SandboxStoreError::Conflict)
    ));
    different.publication = None;
    assert!(matches!(
        crate::store::agent_worker_reconcile::reconcile_publication(&store, &different).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    different = committed.clone();
    different.logs[0].appended_at = "2026-10-01T00:00:00Z".into();
    assert!(
        crate::store::agent_worker_reconcile::reconcile_publication(&store, &different)
            .await
            .is_err()
    );
    different = committed.clone();
    different.publication.as_mut().unwrap().key = "missing".into();
    assert!(
        !crate::store::agent_worker_reconcile::reconcile_publication(&store, &different)
            .await
            .unwrap()
    );
    let metadata = PublicationReceipt {
        publication: Some(publication::PublicationIdentity {
            key: "fixture-metadata".into(),
            fingerprint: digest::binary(vec![8; 32]),
        }),
        agent: agent.clone(),
        logs: vec![],
        worktree: None,
        event: None,
        sign_in_sandbox: None,
    };
    receipt(
        &store,
        &metadata,
        agent.conversation.last_seq as i64,
        100,
        3,
    )
    .await;
    head_revision(&store, &agent, 4).await;
    store
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
        )
        .await
        .unwrap();
    assert!(
        crate::store::agent_worker_reconcile::reconcile_publication(&store, &metadata)
            .await
            .unwrap()
    );
    client.database(&name).drop().await.unwrap();
}
