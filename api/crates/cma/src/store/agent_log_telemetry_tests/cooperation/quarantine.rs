use super::fixture::*;
use super::*;
use crate::store::{
    agent_log_integrity::{scan_complete_log_with_session, QUARANTINE_FIELD},
    agents::agent_document,
    sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent},
    sandbox_mongo::MongoSandboxStore,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn failed_scans_keep_quarantine_and_transaction_abort_semantics() {
    let client = client().await;
    let db = database(&client);
    let mut store = MongoSandboxStore::open(&client, db.name()).await.unwrap();
    // An unindexed fixture admits damage that the normal unique index prevents.
    // The production scanner and quarantine paths still run unchanged.
    store.agent_log = db.collection("damaged_log_fixture");
    let mut agent = ready_agent(
        &active_sandbox("sbx_scan_quarantine"),
        "agt_scan_quarantine",
    );
    agent.conversation.last_seq = 16;
    let writes = db.collection::<Document>("pending_writes");
    writes.insert_one(doc! {"_id": "committed"}).await.unwrap();
    for case in 0..9 {
        store.agent_log.delete_many(doc! {}).await.unwrap();
        let expected_head = if case == 6 {
            10_129
        } else if case == 7 {
            66
        } else {
            16
        };
        agent.conversation.last_seq = expected_head;
        let rows = entries(
            &agent.id,
            if case == 7 { 66 } else { 16 },
            if case == 7 { 1024 * 1024 } else { 32 },
        );
        insert(&store.agent_log, &rows).await;
        match case {
            0 => {
                store
                    .agent_log
                    .delete_one(doc! {"seq": 12_i64})
                    .await
                    .unwrap();
            }
            1 => {
                let mut duplicate = log_document(&rows[11]).unwrap();
                duplicate.insert("_id", "conflicting-duplicate-envelope");
                store.agent_log.insert_one(duplicate).await.unwrap();
            }
            2 => {
                store
                    .agent_log
                    .update_one(doc! {"seq": 12_i64}, doc! {"$set":{"seq": 0_i64}})
                    .await
                    .unwrap();
            }
            3 => {
                store
                    .agent_log
                    .update_one(
                        doc! {"seq": 12_i64},
                        doc! {"$set":{"record": "invalid record"}},
                    )
                    .await
                    .unwrap();
            }
            4 => {
                store
                    .agent_log
                    .update_one(
                        doc! {"seq": 12_i64},
                        doc! {"$set":{"document_schema_version": -1_i64}},
                    )
                    .await
                    .unwrap();
            }
            5 => {
                store
                    .agent_log
                    .update_one(doc! {"seq": 12_i64}, doc! {"$set":{"record.seq": 99_i64}})
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let mut outcomes = Vec::new();
        for enabled in [false, true] {
            store = store.with_scan_cooperation(enabled);
            store
                .agents
                .replace_one(
                    doc! {"_id": agent.id.as_str()},
                    agent_document(&agent).unwrap(),
                )
                .upsert(true)
                .await
                .unwrap();
            let mut session = client.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            writes
                .insert_one(doc! {"_id": "uncommitted"})
                .session(&mut session)
                .await
                .unwrap();
            let error = scan_complete_log_with_session(
                ScanCaller::Other,
                &store,
                &agent.id,
                expected_head,
                &mut session,
                |entry| {
                    if case == 8 && entry.seq == 12 {
                        Err(SandboxStoreError::Unavailable("visitor refusal".into()))
                    } else {
                        Ok(())
                    }
                },
            )
            .await
            .unwrap_err();
            match case {
                0 => assert!(matches!(
                    error,
                    TransactionLogError::Domain(SandboxStoreError::SequenceConflict)
                )),
                6 | 7 => assert!(matches!(
                    error,
                    TransactionLogError::Domain(SandboxStoreError::AgentLogCapacityExceeded)
                )),
                8 => assert!(matches!(
                    error,
                    TransactionLogError::Domain(SandboxStoreError::Unavailable(_))
                )),
                _ => assert!(matches!(
                    error,
                    TransactionLogError::Domain(SandboxStoreError::Corrupt(_))
                )),
            }
            let current = store
                .agents
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(current.contains_key(QUARANTINE_FIELD), case != 8);
            if case == 8 {
                // Visitor errors remain caller-owned; they never quarantine.
                session.abort_transaction().await.unwrap();
            } else {
                // An already aborted session refuses another abort locally.
                assert!(session.abort_transaction().await.is_err());
            }
            assert!(writes
                .find_one(doc! {"_id": "uncommitted"})
                .await
                .unwrap()
                .is_none());
            outcomes.push((format!("{error:?}"), current.get(QUARANTINE_FIELD).cloned()));
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
    db.drop().await.unwrap();
}
