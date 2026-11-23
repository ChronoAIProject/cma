use super::*;
use crate::store::agent_log_integrity::TransactionLogError;
use segments::{
    capture::{capture, capture_transaction, points, points_transaction, CaptureMode},
    transaction_scope::TransactionScope,
};

fn domain_refusal<T>(result: &Result<T, TransactionLogError>) -> bool {
    matches!(
        result,
        Err(TransactionLogError::Domain(
            SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict
        ))
    )
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn empty_tail_and_empty_history_still_prove_malformed_key_absence() {
    for closed in [0, 3] {
        let (client, name) = database("seq_empty").await;
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = segmented_with_width(&store, closed, 2, 0).await;
        certify(&store, &agent, &entries).await;
        let mut session = client.start_session().snapshot(true).await.unwrap();
        let proof = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
            .await
            .unwrap();
        assert!(proof.active_leaf_digests().bytes.is_empty());
        assert_eq!(proof.latest_entry(), entries.last());
        drop(session);
        for seq in [None, Some(Bson::Null), Some("bad".into())] {
            let mut row = doc! {"_id":"bad-empty","agent_id":agent.id.as_str()};
            if let Some(seq) = seq {
                row.insert("seq", seq);
            }
            store
                .agent_log
                .insert_one(row.clone())
                .bypass_document_validation(true)
                .await
                .unwrap();
            let mut session = client.start_session().snapshot(true).await.unwrap();
            assert!(domain_refusal(
                &capture(&store, &agent, &mut session, CaptureMode::Snapshot).await
            ));
            assert!(refused(
                &store
                    .agent_log_range(
                        &agent.id,
                        agent.conversation.last_seq,
                        agent.conversation.last_seq,
                        1
                    )
                    .await
            ));
            assert!(refused(
                &store
                    .agent_log_entry(&agent.id, agent.conversation.last_seq + 1)
                    .await
            ));
            assert_eq!(
                store
                    .agent_log
                    .find_one(doc! {"_id":"bad-empty"})
                    .await
                    .unwrap(),
                Some(row)
            );
            drop(session);
            store
                .agent_log
                .delete_one(doc! {"_id":"bad-empty"})
                .await
                .unwrap();
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn malformed_probes_share_old_snapshots_and_observe_new_snapshots() {
    let (client, name) = database("seq_snapshot").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    certify(&store, &agent, &entries).await;
    for seq in [
        None,
        Some(Bson::Null),
        Some("bad".into()),
        Some(6_i32.into()),
    ] {
        let mut session = client.start_session().snapshot(true).await.unwrap();
        let captured = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
            .await
            .unwrap();
        let filter = doc! {"agent_id":agent.id.as_str(),"seq":6_i64};
        let original = store
            .agent_log
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        let id = doc! {"_id":original.get("_id").unwrap()};
        let mut changed = original.clone();
        changed.remove("seq");
        if let Some(seq) = seq {
            changed.insert("seq", seq);
        }
        store
            .agent_log
            .replace_one(id.clone(), changed.clone())
            .bypass_document_validation(true)
            .await
            .unwrap();
        assert_eq!(
            points(
                &store,
                &captured,
                &[1, 6, 8],
                &mut session,
                CaptureMode::Snapshot
            )
            .await
            .unwrap(),
            vec![entries[0].clone(), entries[5].clone(), entries[7].clone()]
        );
        let mut seen = Vec::new();
        visit_range(
            &store,
            captured.active_head(),
            LogRange {
                after: 0,
                through: 2,
                limit: 2,
            },
            &mut session,
            true,
            |entry| {
                seen.push(entry);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(seen, entries[..2]);
        assert!(refused(&store.agent_log_range(&agent.id, 0, 2, 2).await));
        let mut fresh = client.start_session().snapshot(true).await.unwrap();
        assert!(domain_refusal(
            &capture(&store, &agent, &mut fresh, CaptureMode::Snapshot).await
        ));
        assert_eq!(
            store.agent_log.find_one(id.clone()).await.unwrap(),
            Some(changed)
        );
        drop((session, fresh));
        store.agent_log.replace_one(id, original).await.unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn retained_heads_and_captured_points_recheck_transaction_local_source_writes() {
    let (client, name) = database("seq_txn").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    certify(&store, &agent, &entries).await;
    for seq in [Bson::Null, "bad".into(), 6_i32.into()] {
        let mut session = client.start_session().await.unwrap();
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        let captured = capture_transaction(&store, &agent, &mut scope)
            .await
            .unwrap();
        store
            .agent_log
            .update_one(
                doc! {"agent_id":agent.id.as_str(),"seq":6_i64},
                doc! {"$set":{"seq":seq}},
            )
            .bypass_document_validation(true)
            .session(scope.session())
            .await
            .unwrap();
        assert!(domain_refusal(
            &points_transaction(&store, &captured, &[1], &mut scope).await
        ));
        let mut visits = 0;
        assert!(domain_refusal(
            &visit_range(
                &store,
                captured.active_head(),
                LogRange {
                    after: 0,
                    through: 2,
                    limit: 2
                },
                scope.session(),
                false,
                |_| {
                    visits += 1;
                    Ok(())
                }
            )
            .await
        ));
        assert_eq!(visits, 0);
        assert!(domain_refusal(
            &capture_transaction(&store, &agent, &mut scope).await
        ));
        scope.release().abort_transaction().await.unwrap();
        assert_eq!(
            store.agent_log_range(&agent.id, 0, 8, 8).await.unwrap(),
            entries
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn final_audit_refuses_malformed_keys_without_rewriting_source_or_receipts() {
    for (i, seq) in [
        None,
        Some(Bson::Null),
        Some("bad".into()),
        Some(6_i32.into()),
        Some(6_f64.into()),
    ]
    .into_iter()
    .enumerate()
    {
        let (client, name) = database("seq_audit").await;
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = segmented(&store, 3).await;
        for _ in 0..3 {
            assert_eq!(
                segments::audit::run_unit(&client, &db, agent.id.as_str(), "malformed", false)
                    .await
                    .unwrap(),
                "running"
            );
        }
        let original = log_document(&entries[5]).unwrap();
        let id = doc! {"_id":original.get("_id").unwrap()};
        let mut changed = original;
        changed.remove("seq");
        if let Some(seq) = seq {
            changed.insert("seq", seq);
        }
        store
            .agent_log
            .replace_one(id, changed)
            .bypass_document_validation(true)
            .await
            .unwrap();
        let raw = store.agent_log.clone_with_type::<RawDocumentBuf>();
        let before: Vec<_> = raw
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let receipts: Vec<Document> = collection(&store, PUBLICATIONS)
            .find(doc! {})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(
            segments::audit::run_unit(&client, &db, agent.id.as_str(), "malformed", false)
                .await
                .unwrap(),
            "corrupt"
        );
        assert!(
            segments::audit::install(&client, &db, agent.id.as_str(), "malformed")
                .await
                .is_err()
        );
        let after: Vec<_> = raw
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(
            before.iter().map(|r| r.as_bytes()).collect::<Vec<_>>(),
            after.iter().map(|r| r.as_bytes()).collect::<Vec<_>>()
        );
        assert_eq!(
            collection(&store, PUBLICATIONS)
                .find(doc! {})
                .await
                .unwrap()
                .try_collect::<Vec<Document>>()
                .await
                .unwrap(),
            receipts
        );
        eprintln!("malformed_seq_final_audit shape={i}: corrupt, source and receipts unchanged");
        db.drop().await.unwrap();
    }
}
