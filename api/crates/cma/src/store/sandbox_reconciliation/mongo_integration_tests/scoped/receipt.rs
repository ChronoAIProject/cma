use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn original_receipt_proves_commit_and_refuses_abort_foreign_or_changed_publication() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = adopted(&store, "sbx_receipt").await;
    let original = row(&store, &agent).await;
    let request = loss(&sandbox);
    let mut session = client.start_session().await.unwrap();
    let RecoveryApplication::Persist(aborted) = apply_attempt(&store, &request, None, &mut session)
        .await
        .unwrap()
    else {
        panic!("expected publication");
    };
    assert_eq!(aborted.segmented.len(), 1);
    session.abort_transaction().await.unwrap();
    let aborted_time = session.operation_time().unwrap();
    assert!(!reconciliation::reconcile(&store, &aborted, aborted_time)
        .await
        .unwrap());
    unchanged(&store, &sandbox, &agent, &original).await;

    let RecoveryApplication::Persist(receipt) = apply_attempt(&store, &request, None, &mut session)
        .await
        .unwrap()
    else {
        panic!("expected publication");
    };
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let committed_time = session.operation_time().unwrap();
    // Reopen the store as a restarted process would; no original session is needed.
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(
        reconciliation::reconcile(&restarted, &receipt, committed_time)
            .await
            .unwrap()
    );
    for field in ["sandbox", "log", "operation", "publication"] {
        let mut foreign = receipt.clone();
        match field {
            "sandbox" => foreign.sandbox.name.push_str("-foreign"),
            "log" => foreign.logs[0].appended_at = BEFORE.into(),
            "operation" => foreign.segmented[0].operation.key.push_str("-foreign"),
            // The original full receipt must match even when current Agent and
            // log rows are identical and the publication is otherwise well-formed.
            "publication" => foreign.segmented[0].publication.head_revision += 1,
            _ => unreachable!(),
        }
        assert!(
            !matches!(
                reconciliation::reconcile(&store, &foreign, committed_time).await,
                Ok(true)
            ),
            "accepted {field}"
        );
    }
    // Absence at a later coherent snapshot cannot prove success. The original
    // commit snapshot still proves the exact publication, including its digest.
    let mut deletion = client.start_session().await.unwrap();
    collection(&store, segments::PUBLICATIONS)
        .delete_one(doc! {"_id": &receipt.segmented[0].publication.id})
        .session(&mut deletion)
        .await
        .unwrap();
    let deleted_time = deletion.operation_time().unwrap();
    assert!(!matches!(
        reconciliation::reconcile(&store, &receipt, deleted_time).await,
        Ok(true)
    ));
    assert!(reconciliation::reconcile(&store, &receipt, committed_time)
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}
