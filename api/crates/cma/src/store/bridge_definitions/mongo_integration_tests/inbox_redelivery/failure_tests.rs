use super::*;

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn ambiguous_alias_commit_recovers_only_the_durable_alias_and_original_receipt() {
    let (admin, database, store, first_inbox, id) = setup().await;
    let (body, token, _) = callback("first-before-ambiguous-alias", "original command");
    let original = first_inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!(
        "cma-platform-alias-reconciliation-{}",
        uuid::Uuid::new_v4().simple()
    );
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let commits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ambiguous = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_commits = commits.clone();
    let observed_ambiguous = ambiguous.clone();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        use mongodb::event::command::CommandEvent;
        use std::sync::atomic::Ordering;
        match event {
            CommandEvent::Started(event) if event.command_name == "commitTransaction" => {
                observed_commits.fetch_add(1, Ordering::SeqCst);
            }
            CommandEvent::Succeeded(event)
                if event.command_name == "commitTransaction"
                    && event.reply.get_document("writeConcernError").is_ok() =>
            {
                observed_ambiguous.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = open_store(&client, &database).await;
    let inbox =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let (alias, alias_token, _) = callback("ambiguous-alias", "original command");
    let pending = proven_write(&store, &id, &alias, &alias_token).await;
    let alias_id = pending.record().id.clone();
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":"alwaysOn",
        "data":{"failCommands":["commitTransaction"],"appName":&app_name,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous provider alias commit"}}
    }).await.unwrap();
    commits.store(0, std::sync::atomic::Ordering::SeqCst);
    ambiguous.store(0, std::sync::atomic::Ordering::SeqCst);
    let outcome = inbox
        .accept(pending)
        .await
        .map(|value| value.into_message().unwrap());
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .expect("disable failpoint before assertions");
    let attempts = commits.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        attempts >= 5,
        "the bounded commit helper must exhaust its attempts"
    );
    assert_eq!(
        ambiguous.load(std::sync::atomic::Ordering::SeqCst),
        attempts
    );
    // Injected write-concern errors bypass the usual majority acknowledgement.
    // A separate immediate majority read can legitimately lag this commit.
    // No observed durable alias must stay unknown, never become a false receipt.
    match outcome {
        Ok(receipt) => assert_eq!(receipt, original),
        Err(error) => assert_eq!(error, CallbackError::Unavailable),
    }
    let db = client.database(&database);
    let ledger = crate::store::durability::majority_collection::<Document>(
        &db,
        "bridge_callback_platform_messages",
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if ledger
                .find_one(doc! {"_id":format!("alias:{alias_id}")})
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the original alias commit must become majority-visible");
    assert_eq!(
        inbox
            .recover_receipt(&proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.map(|value| value.into_message().unwrap())),
        Ok(Some(original.clone()))
    );
    for name in [
        "bridge_callback_inbox",
        "bridge_callback_receipts",
        "bridge_callback_replies",
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
    }
    assert_eq!(
        inbox
            .reconcile_commit(&proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap(),
        original
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn failed_alias_insert_does_not_acknowledge_or_increment_admission_effects() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("first-before-alias-failure", "original command");
    let original = inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    let db = client.database(&database);
    // This validator belongs solely to this disposable test database.
    db.run_command(doc! {"collMod":"bridge_callback_platform_messages", "validator":{"kind":{"$ne":"alias"}}, "validationLevel":"strict"}).await.unwrap();
    let (alias, alias_token, _) = callback("failed-alias", "original command");
    assert_eq!(
        inbox
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Unavailable)
    );
    assert_eq!(
        inbox
            .reconcile_commit(&proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Unavailable)
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_platform_messages")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_rates")
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap()
            .get_i64("count")
            .unwrap(),
        1
    );
    for name in [
        "bridge_callback_inbox",
        "bridge_callback_receipts",
        "bridge_callback_replies",
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
    }
    assert_eq!(
        inbox
            .accept(proven_write(&store, &id, &body, &token).await)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap(),
        original
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn aliases_preserve_command_disposition_and_reject_corrupt_primary_or_alias_linkage() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("first-bound-command", "original command");
    let original = inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    let db = client.database(&database);
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let document = receipts
        .find_one(doc! {"_id":&original.inbox_id})
        .await
        .unwrap()
        .unwrap();
    let mut accepted = AcceptedCallbackReceipt::read(&document).unwrap();
    // This durable disposition fixture models the command correlation retained
    // by the separately exercised MongoChannelCommandStore transaction.
    accepted.bind(&doc! {"command_id":"cmd_original","response_id":"response_original","allocation_id":"allocation_original","body_sha256":&accepted.body_sha256}).unwrap();
    let bound = accepted.document().unwrap();
    receipts
        .replace_one(doc! {"_id":&original.inbox_id}, bound.clone())
        .await
        .unwrap();
    let (alias, alias_token, _) = callback("later-bound-command-alias", "original command");
    assert_eq!(
        inbox
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap(),
        original
    );
    assert_eq!(
        receipts
            .find_one(doc! {"_id":&original.inbox_id})
            .await
            .unwrap(),
        Some(bound)
    );
    let ledger = db.collection::<Document>("bridge_callback_platform_messages");
    let first = ledger
        .find_one(doc! {"kind":"first_message"})
        .await
        .unwrap()
        .unwrap();
    ledger
        .update_one(
            doc! {"_id":first.get_str("_id").unwrap()},
            doc! {"$set":{"record.source.body_sha256":"b".repeat(64)}},
        )
        .await
        .unwrap();
    assert_eq!(
        inbox
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Unavailable)
    );
    ledger
        .replace_one(doc! {"_id":first.get_str("_id").unwrap()}, first.clone())
        .await
        .unwrap();
    ledger
        .update_one(
            doc! {"kind":"alias"},
            doc! {"$set":{"record.first_sha256":"c".repeat(64)}},
        )
        .await
        .unwrap();
    assert_eq!(
        inbox
            .reconcile_commit(&proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Unavailable)
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(receipts.count_documents(doc! {}).await.unwrap(), 1);
    db.drop().await.unwrap();
}
