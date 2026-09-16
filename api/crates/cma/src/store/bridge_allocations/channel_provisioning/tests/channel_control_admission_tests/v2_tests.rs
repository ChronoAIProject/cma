use super::*;
use futures_util::TryStreamExt;
use layer2_core::bridge::ingress::{CallbackAcceptance, CallbackInboxStore};

async fn rows(c: &ControlFixture, name: &str) -> u64 {
    c.f.client
        .database(&c.f.database)
        .collection::<Document>(name)
        .count_documents(doc! {})
        .await
        .unwrap()
}
pub(super) async fn rates(c: &ControlFixture) -> i64 {
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_callback_rates")
        .find(doc! {})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .iter()
        .map(|row| row.get_i64("count").unwrap())
        .sum()
}
pub(super) async fn order(c: &ControlFixture) -> Document {
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("agent_command_orders")
        .find_one(doc! {"_id":c.f.command.receipt.envelope.agent_id.as_str()})
        .await
        .unwrap()
        .unwrap()
}
async fn app_store(
    c: &ControlFixture,
) -> (
    crate::store::bridge_invokes::channel_controls::MongoChannelControlStore,
    String,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    let application = format!("accepted-control-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(application.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    let resources = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &c.f.database)
            .await
            .unwrap(),
    );
    let bridges = Arc::new(
        crate::store::MongoBridgeStore::open(
            &client,
            &c.f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    let callbacks = Arc::new(
        crate::store::MongoCallbackInbox::open(
            &bridges,
            &c.f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    let invokes = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(bridges, resources, &c.f.database)
            .await
            .unwrap(),
    );
    (
        crate::store::bridge_invokes::channel_controls::MongoChannelControlStore::open(
            invokes, callbacks,
        )
        .await
        .unwrap(),
        application,
    )
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_v2_concurrent_aliases_commit_one_acceptance_intent_order_and_rate() {
    let c = ControlFixture::new().await;
    let before_rate = rates(&c).await;
    let before_dispositions = rows(&c, "bridge_callback_dispositions").await;
    let mut requests = Vec::new();
    for index in 0..4 {
        requests.push(
            c.request(&format!("accepted-alias-{index}"), 210, false)
                .await,
        );
    }
    let results =
        futures_util::future::join_all(requests.into_iter().map(|request| c.store.admit(request)))
            .await;
    let mut results = results.into_iter().map(Result::unwrap);
    let first = results.next().unwrap();
    let admitted = first.admitted.unwrap();
    assert_eq!(
        first.acceptance,
        CallbackAcceptance::ControlAccepted(admitted.sidecar.intent.expected_acceptance().unwrap())
    );
    for value in results {
        assert_eq!(value.acceptance, first.acceptance);
        assert!(value.admitted.unwrap() == admitted);
    }
    assert_eq!(rates(&c).await, before_rate + 1);
    assert_eq!(
        rows(&c, "bridge_callback_dispositions").await,
        before_dispositions + 5
    );
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 1);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 4);
    let command_count =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_commands")
            .count_documents(doc! {"channel_control_admission":{"$exists":true}})
            .await
            .unwrap();
    assert_eq!(command_count, 1);
    let before_order = order(&c).await;
    c.f.finish().await;
    let write = c
        .write("accepted-after-finish", 210, 12345, true, false)
        .await;
    let result = c.store.recover_acceptance(&write).await.unwrap().unwrap();
    assert_eq!(result.acceptance, first.acceptance);
    assert!(result.admitted.unwrap() == admitted);
    assert_eq!(
        c.callbacks.recover_receipt(write.inbox()).await.unwrap(),
        Some(first.acceptance)
    );
    assert_eq!(rates(&c).await, before_rate + 1);
    assert_eq!(order(&c).await, before_order);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_v2_preserves_v1_refusal_and_legacy_message_without_control_work() {
    let c = ControlFixture::new().await;
    let refusal = c
        .callbacks
        .retain_refused_control(c.write("permanent-refusal", 211, 12345, true, false).await)
        .await
        .unwrap();
    let before_rate = rates(&c).await;
    let value = c
        .store
        .admit(c.request("permanent-refusal-alias", 211, false).await)
        .await
        .unwrap();
    assert_eq!(value.acceptance, refusal.acceptance);
    assert!(matches!(
        value.acceptance,
        CallbackAcceptance::ControlRefused(_)
    ));
    assert!(value.admitted.is_none());
    assert_eq!(rates(&c).await, before_rate);
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 0);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 0);

    let write = c
        .write("legacy-control-message", 212, 12345, true, false)
        .await;
    let source = write.inbox().record();
    let receipt = CallbackAcceptance::Message(source.receipt());
    // A retained Message from before the reserved-prefix guard remains history.
    // Its genuine signed callback is never reclassified into control work.
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_callback_inbox")
        .insert_one(doc! {"_id":&source.id,"document_schema_version":1_i32,
        "bridge_id":source.pin.bridge_id.as_str(),"admission":"accepted",
        "expires_at":mongodb::bson::DateTime::from_millis(source.expires_at*1000),
        "record":record_bson(source).unwrap()})
        .await
        .unwrap();
    let request = ChannelControlReservation::prepare(c.proof(write).await.unwrap()).unwrap();
    let value = c.store.admit(request).await.unwrap();
    assert_eq!(value.acceptance, receipt);
    assert!(value.admitted.is_none());
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 0);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 0);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_v2_provider_write_failure_rolls_back_every_admission_effect() {
    let c = ControlFixture::new().await;
    let before_rate = rates(&c).await;
    let before_order = order(&c).await;
    let before_dispositions = rows(&c, "bridge_callback_dispositions").await;
    c.f.client
        .database(&c.f.database)
        .run_command(doc! {
            "collMod":"bridge_callback_dispositions",
            "validator":{"$or":[{"kind":{"$ne":"provider"}},{"record.version":{"$ne":2_i32}}]},
            "validationLevel":"strict","validationAction":"error",
        })
        .await
        .unwrap();
    assert!(c
        .store
        .admit(c.request("atomic-provider-failure", 213, false).await)
        .await
        .is_err());
    assert_eq!(rates(&c).await, before_rate);
    assert_eq!(order(&c).await, before_order);
    assert_eq!(
        rows(&c, "bridge_callback_dispositions").await,
        before_dispositions
    );
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 0);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 0);
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_commands")
            .count_documents(doc! {"channel_control_admission":{"$exists":true}})
            .await
            .unwrap(),
        0
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_v2_requires_both_canonical_dispositions_before_retaining_alias() {
    let c = ControlFixture::new().await;
    let original = c
        .store
        .reserve(c.request("canonical-original", 214, false).await)
        .await
        .unwrap();
    let collection =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_dispositions");
    for (index, id) in [
        format!("callback:{}", original.sidecar.intent.callback.callback_id),
        format!(
            "provider:{}",
            original.sidecar.intent.callback.provider_key_sha256
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let row = collection
            .find_one(doc! {"_id":&id})
            .await
            .unwrap()
            .unwrap();
        collection.delete_one(doc! {"_id":&id}).await.unwrap();
        let write = c
            .write(
                &format!("missing-canonical-{index}"),
                214,
                12345,
                true,
                false,
            )
            .await;
        let callback = ChannelControlCallback::capture(&write).unwrap();
        assert!(c.store.recover_acceptance(&write).await.is_err());
        assert!(collection
            .find_one(doc! {"_id":format!("callback:{}",callback.callback_id)})
            .await
            .unwrap()
            .is_none());
        assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 1);
        collection.insert_one(row).await.unwrap();
    }
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn channel_control_v2_custody_transient_retries_the_complete_transaction() {
    let c = ControlFixture::new().await;
    let (store, application) = app_store(&c).await;
    let request = c.request("custody-transient", 215, false).await;
    let before_rate = rates(&c).await;
    c.f.client
        .database("admin")
        .run_command(
            doc! {"configureFailPoint":"failCommand","mode":{"times":1_i32},
            "data":{"failCommands":["find"],"appName":&application,
                "namespace":format!("{}.bridge_credentials",c.f.database),
                "errorCode":112_i32,"errorLabels":["TransientTransactionError"]}},
        )
        .await
        .unwrap();
    let result = store.admit(request).await;
    c.f.client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let result = result.unwrap();
    assert!(matches!(
        result.acceptance,
        CallbackAcceptance::ControlAccepted(_)
    ));
    assert!(result.admitted.is_some());
    assert_eq!(rates(&c).await, before_rate + 1);
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 1);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 1);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn channel_control_v2_delayed_alias_write_aborts_when_original_proof_expires() {
    let c = ControlFixture::new().await;
    let original = c
        .store
        .admit(c.request("deadline-original", 216, false).await)
        .await
        .unwrap();
    let original_receipt = original.acceptance;
    let (store, application) = app_store(&c).await;
    for admit in [true, false] {
        let write = c
            .write_with_lease(
                &format!("expired-alias-{admit}"),
                216,
                12345,
                true,
                false,
                Duration::from_secs(1),
            )
            .await;
        let callback = ChannelControlCallback::capture(&write).unwrap();
        let (request, recovery) = if admit {
            (
                Some(ChannelControlReservation::prepare(c.proof(write).await.unwrap()).unwrap()),
                None,
            )
        } else {
            (None, Some(write))
        };
        c.f.client
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":1_i32},
                "data":{"failCommands":["insert"],"appName":&application,
                    "namespace":format!("{}.bridge_channel_control_aliases",c.f.database),
                    "blockConnection":true,"blockTimeMS":1500_i32}},
            )
            .await
            .unwrap();
        let start = tokio::time::Instant::now();
        let result = if let Some(request) = request {
            store.admit(request).await.map(|_| ())
        } else {
            store
                .recover_acceptance(recovery.as_ref().unwrap())
                .await
                .map(|_| ())
        };
        c.f.client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert!(matches!(
            result,
            Err(ChannelControlError::AuthorityUnavailable)
        ));
        for (name, id) in [
            (
                "bridge_callback_dispositions",
                format!("callback:{}", callback.callback_id),
            ),
            ("bridge_channel_control_aliases", callback.callback_id),
        ] {
            assert!(c
                .f
                .client
                .database(&c.f.database)
                .collection::<Document>(name)
                .find_one(doc! {"_id":id})
                .await
                .unwrap()
                .is_none());
        }
    }
    let fresh = c
        .write("deadline-fresh-retry", 216, 12345, true, false)
        .await;
    assert_eq!(
        c.store
            .recover_acceptance(&fresh)
            .await
            .unwrap()
            .unwrap()
            .acceptance,
        original_receipt
    );
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 1);
    c.f.close().await;
}
