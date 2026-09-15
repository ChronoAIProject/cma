use super::super::redelivery::proven_write_with_lease;
use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_disposition_failure_aborts_both_message_and_control_effects() {
    for text in ["ordinary message", "CMA-CONTROL/1 cancel ccr_source"] {
        let (client, database, store, inbox, id) = setup().await;
        let db = client.database(&database);
        db.run_command(doc! {"collMod":"bridge_callback_dispositions",
        "validator":{"kind":{"$ne":"provider"}},"validationLevel":"strict"})
            .await
            .unwrap();
        let (body, token, _) = callback("failed-shared-disposition", text);
        assert_eq!(
            inbox
                .accept(proven_write(&store, &id, &body, &token).await)
                .await,
            Err(CallbackError::Unavailable)
        );
        no_message_effects(&client, &database, &inbox).await;
        for name in ["bridge_callback_dispositions", "bridge_callback_rates"] {
            assert_eq!(
                db.collection::<Document>(name)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0,
                "{name}"
            );
        }
        assert_eq!(
            inbox
                .reconcile_commit(&proven_write(&store, &id, &body, &token).await)
                .await,
            Err(CallbackError::Unavailable)
        );
        db.run_command(doc! {"collMod":"bridge_callback_dispositions","validator":{},"validationLevel":"strict"}).await.unwrap();
        let accepted = inbox
            .accept(proven_write(&store, &id, &body, &token).await)
            .await
            .unwrap();
        assert_eq!(
            matches!(accepted, CallbackAcceptance::ControlRefused(_)),
            text.starts_with("CMA-CONTROL")
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn uncertain_control_commit_expires_proof_then_recovers_only_durable_refusal() {
    let (admin, database, _, _, id) = setup().await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!("cma-control-refusal-{}", uuid::Uuid::new_v4().simple());
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = open_store(&client, &database).await;
    let inbox =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let (body, token, _) = callback("uncertain-control", "CMA-CONTROL/1 cancel ccr_source");
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":"alwaysOn",
        "data":{"failCommands":["commitTransaction"],"appName":&app_name,
            "blockConnection":true,"blockTimeMS":300_i32,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"disposable unknown control commit"}}
    }).await.unwrap();
    let write = proven_write_with_lease(&store, &id, &body, &token, Duration::from_secs(1)).await;
    let callback_id = write.record().id.clone();
    let outcome = inbox.accept(write).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .expect("disable test failpoint before assertions");
    assert_eq!(outcome, Err(CallbackError::Unavailable));
    let decisions = crate::store::durability::majority_collection::<Document>(
        &client.database(&database),
        "bridge_callback_dispositions",
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if decisions
                .find_one(doc! {"_id":format!("callback:{callback_id}")})
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
    .expect("committed refusal becomes majority visible");
    let original = decisions
        .find_one(doc! {"_id":format!("callback:{callback_id}")})
        .await
        .unwrap()
        .unwrap();
    let accepted = inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    assert!(
        matches!(&accepted, CallbackAcceptance::ControlRefused(receipt) if receipt.callback_id == callback_id)
    );
    assert_eq!(
        decisions
            .find_one(doc! {"_id":format!("callback:{callback_id}")})
            .await
            .unwrap(),
        Some(original)
    );
    no_message_effects(&client, &database, &inbox).await;
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn control_alias_requires_exact_commit_and_rejects_changed_sender_or_content() {
    let (client, database, store, inbox, id) = setup().await;
    let text = "CMA-CONTROL/1 cancel ccr_source";
    let (body, token, _) = callback("original-refusal", text);
    let original = inbox
        .accept(proven_write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    let (alias, alias_token, _) = callback("new-alias", text);
    let pending = proven_write(&store, &id, &alias, &alias_token).await;
    assert_eq!(inbox.recover_receipt(&pending).await.unwrap(), None);
    assert_eq!(
        inbox.reconcile_commit(&pending).await,
        Err(CallbackError::Unavailable)
    );
    let db = client.database(&database);
    db.run_command(doc! {"collMod":"bridge_callback_dispositions", "validator":{"_id":{"$ne":format!("callback:{}",pending.record().id)}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(inbox.accept(pending).await, Err(CallbackError::Unavailable));
    assert_eq!(
        inbox
            .recover_receipt(&proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .unwrap(),
        None
    );
    db.run_command(
        doc! {"collMod":"bridge_callback_dispositions","validator":{},"validationLevel":"strict"},
    )
    .await
    .unwrap();
    assert_eq!(
        inbox
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .unwrap(),
        original
    );
    for (body, token, _) in [
        callback("changed-text", "CMA-CONTROL/1 steer ccr_source\nDifferent"),
        callback_for_sender("changed-sender", text, 98765),
    ] {
        assert_eq!(
            inbox
                .accept(proven_write(&store, &id, &body, &token).await)
                .await,
            Err(CallbackError::ReplayConflict)
        );
    }
    no_message_effects(&client, &database, &inbox).await;
    db.drop().await.unwrap();
}
