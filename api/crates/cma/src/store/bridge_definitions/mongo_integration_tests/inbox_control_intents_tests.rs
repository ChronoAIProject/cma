use super::*;
use layer2_core::bridge::channel_control::ChannelControlWrite;

const TEXT: &str =
    "CMA-CONTROL/1 steer ccr_0123456789abcdef0123456789abcdef\nPreserve partial output.";

async fn control(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
) -> ChannelControlWrite {
    ChannelControlWrite::from_verified(redelivery::proven_write(store, id, body, token).await)
        .unwrap()
}

async fn count(client: &Client, database: &str, name: &str) -> u64 {
    client
        .database(database)
        .collection::<Document>(name)
        .count_documents(doc! {})
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_refused_control_intent_aliases_recover_one_permanent_refusal_without_work() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, secret) = callback("intent-original", TEXT);
    let (alias, alias_token, _) = callback("intent-alias", TEXT);
    let a = control(&store, &id, &body, &token).await;
    let b = control(&store, &id, &alias, &alias_token).await;
    let (a, b) = tokio::join!(
        inbox.retain_refused_control(a),
        inbox.retain_refused_control(b)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.acceptance, b.acceptance);
    assert!(a.intent == b.intent);
    let intent = a.intent.unwrap();
    assert_eq!(
        intent.receipt.code,
        ChannelControlRefusal::ChannelControlsDisabled
    );
    assert_eq!(
        count(&client, &database, "bridge_refused_control_intents").await,
        1
    );
    assert_eq!(
        count(&client, &database, "bridge_refused_control_aliases").await,
        2
    );
    assert_eq!(
        count(&client, &database, "bridge_callback_dispositions").await,
        3
    );
    let encoded = serde_json::to_string(&intent).unwrap();
    assert!(!encoded.contains(&secret));
    assert!(!encoded.contains("protected-interaction-secret"));
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    for (body, token) in [(&body, &token), (&alias, &alias_token)] {
        // The unmodified ingress still sees the exact original refusal.
        assert_eq!(
            reopened
                .accept(redelivery::proven_write(&store, &id, body, token).await)
                .await
                .unwrap(),
            a.acceptance
        );
        let replay = reopened
            .retain_refused_control(control(&store, &id, body, token).await)
            .await
            .unwrap();
        assert_eq!(replay.acceptance, a.acceptance);
        assert!(replay.intent.as_ref() == Some(&intent));
    }
    let (changed, changed_token, _) = callback(
        "intent-changed",
        &TEXT.replace("partial output", "a different task"),
    );
    assert!(matches!(
        reopened
            .retain_refused_control(control(&store, &id, &changed, &changed_token).await)
            .await,
        Err(CallbackError::ReplayConflict)
    ));
    for name in [
        "bridge_callback_inbox",
        "bridge_callback_receipts",
        "bridge_callback_replies",
        "agent_commands",
        "agent_log",
        "bridge_outbox_items",
    ] {
        assert_eq!(count(&client, &database, name).await, 0, "{name}");
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_refused_control_never_backfills_old_refusals_or_reinterprets_legacy_message() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("old-control", TEXT);
    let old = inbox
        .accept(redelivery::proven_write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    let result = inbox
        .retain_refused_control(control(&store, &id, &body, &token).await)
        .await
        .unwrap();
    assert_eq!(result.acceptance, old);
    assert!(result.intent.is_none());
    assert_eq!(
        count(&client, &database, "bridge_refused_control_intents").await,
        0
    );
    client.database(&database).drop().await.unwrap();

    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("legacy-message", TEXT);
    let pending = control(&store, &id, &body, &token).await;
    let source = pending.inbox().record();
    let receipt =
        crate::store::bridge_definitions::inbox::receipts::model::AcceptedCallbackReceipt::capture(
            source,
        )
        .unwrap()
        .document()
        .unwrap();
    client
        .database(&database)
        .collection::<Document>("bridge_callback_receipts")
        .insert_one(receipt)
        .await
        .unwrap();
    let expected = CallbackAcceptance::Message(source.receipt());
    let result = inbox.retain_refused_control(pending).await.unwrap();
    assert_eq!(result.acceptance, expected);
    assert!(result.intent.is_none());
    assert_eq!(
        count(&client, &database, "bridge_refused_control_intents").await,
        0
    );
    assert_eq!(
        count(&client, &database, "bridge_callback_dispositions").await,
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_refused_control_alias_failure_rolls_back_intent_disposition_and_rate() {
    let (client, database, store, inbox, id) = setup().await;
    client
        .database(&database)
        .run_command(doc! {"collMod":"bridge_refused_control_aliases",
        "validator":{"never_present":{"$exists":true}},"validationLevel":"strict"})
        .await
        .unwrap();
    let (body, token, _) = callback("failed-control", TEXT);
    assert!(matches!(
        inbox
            .retain_refused_control(control(&store, &id, &body, &token).await)
            .await,
        Err(CallbackError::Unavailable)
    ));
    for name in [
        "bridge_refused_control_intents",
        "bridge_refused_control_aliases",
        "bridge_callback_dispositions",
        "bridge_callback_rates",
        "agent_commands",
    ] {
        assert_eq!(count(&client, &database, name).await, 0, "{name}");
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn signed_refused_control_unknown_commit_preserves_exact_intent_and_original_time() {
    let (admin, database, _, _, id) = setup().await;
    let mut options = mongodb::options::ClientOptions::parse(
        std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap(),
    )
    .await
    .unwrap();
    let application = format!("control-intent-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(application.clone());
    let client = Client::with_options(options).unwrap();
    let store = open_store(&client, &database).await;
    let inbox =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let (body, token, _) = callback("uncertain-control", TEXT);
    let pending = ChannelControlWrite::from_verified(
        redelivery::proven_write_with_lease(&store, &id, &body, &token, Duration::from_secs(1))
            .await,
    )
    .unwrap();
    let original_time = pending.inbox().record().accepted_at;
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":"alwaysOn",
        "data":{"failCommands":["commitTransaction"],"appName":&application,"blockConnection":true,"blockTimeMS":300_i32,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"uncertain refused control"}}}).await.unwrap();
    let result = inbox.retain_refused_control(pending).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(CallbackError::Unavailable)));
    let replay = inbox
        .retain_refused_control(control(&store, &id, &body, &token).await)
        .await
        .unwrap();
    assert_eq!(replay.intent.unwrap().receipt.persisted_at, original_time);
    assert_eq!(
        count(&client, &database, "bridge_refused_control_intents").await,
        1
    );
    assert_eq!(
        count(&client, &database, "bridge_refused_control_aliases").await,
        1
    );
    assert_eq!(count(&client, &database, "agent_commands").await, 0);
    client.database(&database).drop().await.unwrap();
}
