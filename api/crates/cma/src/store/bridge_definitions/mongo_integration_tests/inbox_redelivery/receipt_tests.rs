use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn concurrent_platform_redeliveries_keep_one_inbox_rate_and_reply_route() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("first-provider-delivery", "original command");
    let first_write = proven_write(&store, &id, &body, &token).await;
    let first_id = first_write.record().id.clone();
    let (later_body, later_token, _) = callback("fresh-nyx-redelivery", "original command");
    let later_write = proven_write(&store, &id, &later_body, &later_token).await;
    let later_id = later_write.record().id.clone();
    let (first, later) = tokio::join!(inbox.accept(first_write), inbox.accept(later_write));
    let accepted = first.unwrap();
    assert_eq!(later.unwrap(), accepted);
    assert!([&first_id, &later_id].contains(&&accepted.inbox_id));
    let db = client.database(&database);
    for name in [
        "bridge_callback_inbox",
        "bridge_callback_receipts",
        "bridge_callback_replies",
    ] {
        let rows = db.collection::<Document>(name);
        assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1, "{name}");
        assert!(rows
            .find_one(doc! {"_id":&accepted.inbox_id})
            .await
            .unwrap()
            .is_some());
    }
    let rate = db
        .collection::<Document>("bridge_callback_rates")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rate.get_i64("count").unwrap(), 1);
    let ledger = db.collection::<Document>("bridge_callback_platform_messages");
    assert_eq!(
        ledger
            .count_documents(doc! {"kind":"first_message"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        ledger.count_documents(doc! {"kind":"alias"}).await.unwrap(),
        1
    );
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    for (body, token) in [(&body, &token), (&later_body, &later_token)] {
        assert_eq!(
            reopened
                .accept(proven_write(&store, &id, body, token).await)
                .await
                .unwrap(),
            accepted
        );
    }
    let retained = reopened
        .channel_input(&accepted.inbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.message_id, accepted.message_id);
    assert_eq!(retained.envelope.text.as_deref(), Some("original command"));
    assert_eq!(retained.envelope.chat_id, "-100123");
    let stored = serde_json::to_string(
        &ledger
            .find_one(doc! {"kind":"first_message"})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    for secret in [
        "original command",
        "protected-interaction-secret",
        "reply_token",
        "thread_context",
    ] {
        assert!(!stored.contains(secret));
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn exact_alias_is_required_for_read_only_reconciliation_and_content_conflicts_remain() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("original", "original text");
    let original = inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    let (alias_body, alias_token, _) = callback("alias", "original text");
    let pending_alias = proven_write(&store, &id, &alias_body, &alias_token).await;
    let alias_id = pending_alias.record().id.clone();
    assert!(inbox
        .recover_receipt(&pending_alias)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        inbox.reconcile_commit(&pending_alias).await,
        Err(CallbackError::Unavailable)
    );
    assert_eq!(inbox.accept(pending_alias).await.unwrap(), original);
    let fresh = proven_write(&store, &id, &alias_body, &alias_token).await;
    assert_eq!(inbox.reconcile_commit(&fresh).await.unwrap(), original);
    for message in ["original", "alias", "fresh-conflicting-alias"] {
        let (changed, changed_token, _) = callback(message, "changed command");
        assert_eq!(
            inbox
                .accept(proven_write(&store, &id, &changed, &changed_token).await)
                .await,
            Err(CallbackError::ReplayConflict),
            "{message}"
        );
    }
    let db = client.database(&database);
    let ledger = db.collection::<Document>("bridge_callback_platform_messages");
    ledger
        .delete_one(doc! {"_id":format!("alias:{alias_id}")})
        .await
        .unwrap();
    assert_eq!(
        inbox.reconcile_commit(&fresh).await,
        Err(CallbackError::Unavailable)
    );
    assert_eq!(ledger.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        db.collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn expired_original_payload_accepts_alias_without_restoring_command_or_reply_custody() {
    let (client, database, store, inbox, id) = setup().await;
    let mut short = config();
    short.limits.inbox_retention_seconds = 1;
    short.limits.reply_secret_retention_seconds = 1;
    service(store.clone())
        .mutate(
            &caller(),
            None,
            change(
                &id,
                2,
                "short-redelivery-retention",
                BridgeChange::Update {
                    config: Box::new(short),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
    let (body, token, _) = callback("original-before-expiry", "original text");
    let original = inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let db = client.database(&database);
    for name in ["bridge_callback_inbox", "bridge_callback_replies"] {
        db.collection::<Document>(name)
            .delete_many(doc! {"expires_at":{"$lte":mongodb::bson::DateTime::now()}})
            .await
            .unwrap();
    }
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let before = receipts
        .find_one(doc! {"_id":&original.inbox_id})
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        AcceptedCallbackReceipt::read(&before).unwrap().disposition,
        AcceptanceDisposition::Expired { .. }
    ));
    let (later, later_token, _) = callback("alias-after-expiry", "original text");
    assert_eq!(
        reopened
            .accept(proven_write(&store, &id, &later, &later_token).await)
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        receipts
            .find_one(doc! {"_id":&original.inbox_id})
            .await
            .unwrap(),
        Some(before)
    );
    assert!(reopened
        .pending_channel_inputs(None)
        .await
        .unwrap()
        .is_empty());
    for name in ["bridge_callback_inbox", "bridge_callback_replies"] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn legacy_receipts_do_not_speculate_provider_identity_and_missing_primary_refuses_alias() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("legacy-no-provider-ledger", "legacy text");
    let legacy = inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let db = client.database(&database);
    let ledger = db.collection::<Document>("bridge_callback_platform_messages");
    assert_eq!(ledger.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        reopened
            .accept(proven_write(&store, &id, &body, &token).await)
            .await
            .unwrap(),
        legacy
    );
    assert_eq!(ledger.count_documents(doc! {}).await.unwrap(), 0);
    // This explicit legacy limitation is why mixed old/new writers are not a rollout claim.
    let (fresh, fresh_token, _) = callback("new-writer-first", "legacy text");
    let new = reopened
        .accept(proven_write(&store, &id, &fresh, &fresh_token).await)
        .await
        .unwrap();
    assert_ne!(new.inbox_id, legacy.inbox_id);
    let (alias, alias_token, _) = callback("new-writer-alias", "legacy text");
    assert_eq!(
        reopened
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .unwrap(),
        new
    );
    ledger
        .delete_many(doc! {"kind":"first_message"})
        .await
        .unwrap();
    assert_eq!(
        reopened
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await,
        Err(CallbackError::Unavailable)
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    db.drop().await.unwrap();
}
