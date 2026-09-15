use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn source_control_missing_head_history_or_receipt_never_reopens_default_consent() {
    let mut f = Fixture::new().await;
    let grant = grant(&mut f, None).await;
    let api = service(&f);
    let consent = request(&f, 0, SourceControlAction::AllowSenderCancelAndSteer);
    api.submit(&f.context, None, &grant, "enable", consent.clone())
        .await
        .unwrap();
    let db = f.client.database(&f.database);
    for names in [
        vec!["bridge_source_control_policies"],
        vec!["bridge_source_control_audits"],
        vec!["bridge_source_control_receipts"],
        vec![
            "bridge_source_control_policies",
            "bridge_source_control_audits",
        ],
    ] {
        let mut removed = vec![];
        for name in &names {
            let collection = db.collection::<Document>(name);
            let rows = collection
                .find(doc! {})
                .await
                .unwrap()
                .try_collect::<Vec<Document>>()
                .await
                .unwrap();
            collection.delete_many(doc! {}).await.unwrap();
            removed.push((collection, rows));
        }
        assert_eq!(
            api.policy(&f.context, None, &grant).await,
            Err(BridgeInvokeError::Corrupt)
        );
        assert_eq!(
            api.submit(&f.context, None, &grant, "different", consent.clone())
                .await,
            Err(BridgeInvokeError::Corrupt)
        );
        for (collection, rows) in removed {
            collection.insert_many(rows).await.unwrap();
        }
    }
    assert!(
        api.policy(&f.context, None, &grant)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn source_control_receipt_and_audit_substitution_is_refused_after_reopening() {
    let mut f = Fixture::new().await;
    let grant = grant(&mut f, None).await;
    let api = service(&f);
    let consent = request(&f, 0, SourceControlAction::AllowSenderCancelAndSteer);
    let receipt = api
        .submit(&f.context, None, &grant, "enable", consent.clone())
        .await
        .unwrap();
    let collection = f
        .client
        .database(&f.database)
        .collection::<Document>("bridge_source_control_receipts");
    let original = collection
        .find_one(doc! {"_id":&receipt.id})
        .await
        .unwrap()
        .unwrap();
    collection
        .update_one(
            doc! {"_id":&receipt.id},
            doc! {"$set":{"record.request.expected_bridge_revision":(f.bridge.revision+1) as i64}},
        )
        .await
        .unwrap();
    // It is still a structurally valid receipt, but its request no longer matches
    // the audited consent. The request digest is part of the permanent proof.
    assert_eq!(
        api.policy(&f.context, None, &grant).await,
        Err(BridgeInvokeError::Corrupt)
    );
    assert!(api
        .submit(&f.context, None, &grant, "enable", consent)
        .await
        .is_err());
    collection
        .replace_one(doc! {"_id":&receipt.id}, original)
        .await
        .unwrap();
    f.close().await;
}
