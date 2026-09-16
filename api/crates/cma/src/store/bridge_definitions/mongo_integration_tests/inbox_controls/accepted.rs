//! Legacy retained-v2 receipts without the complete original control intent.
use super::*;
use layer2_core::bridge::ingress::ChannelControlAcceptedReceipt;

fn retained(write: &InboxWrite) -> (CallbackAcceptance, Document, Document) {
    let source = write.record();
    let proof = write.platform_redelivery().unwrap();
    let receipt = CallbackAcceptance::ControlAccepted(ChannelControlAcceptedReceipt {
        version: 2,
        callback_id: source.id.clone(),
        bridge_id: source.pin.bridge_id.clone(),
        message_id: source.message_id.clone(),
        intent_id: format!("cci_{}", "c".repeat(64)),
        persisted_at: source.accepted_at,
    });
    let identity = json!({"inbox_id":source.id,"owner":source.owner,"pin":source.pin,
        "bot_id":source.bot_id,"key_id":source.key_id,"message_id":source.message_id,
        "correlation_id":source.correlation_id,"issuer":source.attestation.issuer,
        "body_sha256":source.attestation.body_sha256});
    let record = json!({"version":2,"source":identity,"original":identity,
        "receipt":receipt,"provider":{"key_sha256":proof.key_sha256(),
        "semantic_sha256":proof.semantic_sha256()}});
    let canonical = doc! {"_id":format!("callback:{}",source.id),
    "document_schema_version":DOCUMENT_SCHEMA_VERSION,"kind":"callback",
    "record_sha256":layer2_core::profiles::content_digest(&record).unwrap(),
    "record":record_bson(&record).unwrap()};
    let mut provider = canonical.clone();
    provider.insert("_id", format!("provider:{}", proof.key_sha256()));
    provider.insert("kind", "provider");
    (receipt, canonical, provider)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn legacy_control_receipt_recovers_but_cannot_admit_alias_without_original_intent() {
    let (client, database, store, inbox, id) = setup().await;
    let text = "CMA-CONTROL/1 cancel ccr_source";
    let (body, token, _) = callback("accepted-original", text);
    let write = proven_write(&store, &id, &body, &token).await;
    let (expected, canonical, provider) = retained(&write);
    let decisions = client
        .database(&database)
        .collection::<Document>("bridge_callback_dispositions");
    decisions
        .insert_many([canonical.clone(), provider.clone()])
        .await
        .unwrap();
    assert_eq!(
        inbox.recover_receipt(&write).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(inbox.accept(write).await.unwrap(), expected);
    let (alias, alias_token, _) = callback("accepted-alias", text);
    let write = proven_write(&store, &id, &alias, &alias_token).await;
    // A matching provider record cannot certify an uncommitted alias.
    assert_eq!(inbox.recover_receipt(&write).await.unwrap(), None);
    // A legacy receipt remains readable, but does not establish the complete
    // retained authority/history required to admit a new callback alias.
    assert_eq!(inbox.accept(write).await, Err(CallbackError::Unavailable));
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let write = proven_write(&store, &id, &body, &token).await;
    assert_eq!(
        reopened.recover_receipt(&write).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(reopened.accept(write).await.unwrap(), expected);
    let write = proven_write(&store, &id, &alias, &alias_token).await;
    assert_eq!(reopened.recover_receipt(&write).await.unwrap(), None);
    assert_eq!(
        reopened.accept(write).await,
        Err(CallbackError::Unavailable)
    );
    let (changed, changed_token, _) = callback("accepted-alias", "ordinary input");
    assert_eq!(
        reopened
            .accept(proven_write(&store, &id, &changed, &changed_token).await)
            .await,
        Err(CallbackError::ReplayConflict)
    );
    assert_eq!(
        decisions
            .find_one(doc! {"_id":canonical.get_str("_id").unwrap()})
            .await
            .unwrap(),
        Some(canonical)
    );
    assert_eq!(
        decisions
            .find_one(doc! {"_id":provider.get_str("_id").unwrap()})
            .await
            .unwrap(),
        Some(provider)
    );
    assert_eq!(
        decisions
            .count_documents(doc! {"kind":"callback"})
            .await
            .unwrap(),
        1
    );
    assert!(expected.into_message().is_none());
    no_message_effects(&client, &database, &reopened).await;
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn accepted_intent_http_recovery_reports_only_durable_acceptance() {
    use crate::bridge::ingress::{CallbackIngress, PinnedCallbackKeys};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let id = BridgeService::creation_id(&caller(), "create").unwrap();
    let callback_root = "https://cma.example/api/v1/bridge-callbacks";
    let mut config = config();
    config.connector.callback_url = format!("{callback_root}/{}", id.as_str());
    let (client, database, store, inbox, id) = setup_with_config(config).await;
    let (body, token, _) = callback("http-accepted", "CMA-CONTROL/1 cancel ccr_source");
    let write = proven_write(&store, &id, &body, &token).await;
    let (expected, canonical, provider) = retained(&write);
    client
        .database(&database)
        .collection::<Document>("bridge_callback_dispositions")
        .insert_many([canonical, provider])
        .await
        .unwrap();
    let ingress = CallbackIngress::new(
        store.clone(),
        Arc::new(Authority),
        Arc::new(inbox.clone()),
        PinnedCallbackKeys::fixture(),
        CallbackTokenPolicy {
            issuer: "https://nyx.example".into(),
            relay_audience: "https://nyx.example".into(),
            callback_ttl_seconds: 300,
            relay_ttl_seconds: 300,
            reply_ttl_seconds: 1800,
            clock_skew_seconds: 60,
        },
        callback_root.into(),
    )
    .unwrap();
    let app = crate::bridge::ingress::http::router(Some(Arc::new(ingress)));
    for _ in 0..2 {
        let request = Request::post(format!("/api/v1/bridge-callbacks/{}", id.as_str()))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", &token)
            .header("x-nyxid-message-id", "http-accepted")
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-timestamp", layer2_core::now_rfc3339())
            .body(Body::from(body.clone()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let CallbackAcceptance::ControlAccepted(receipt) = &expected else {
            unreachable!()
        };
        assert_eq!(
            response,
            json!({"status":"control_accepted","receipt":receipt})
        );
        for key in ["command_id", "response_id", "completed", "cancelled"] {
            assert!(response["receipt"].get(key).is_none());
        }
    }
    no_message_effects(&client, &database, &inbox).await;
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn malformed_cross_version_disposition_cannot_be_reinterpreted_as_fresh_work() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("invalid-version", "CMA-CONTROL/1 cancel ccr_source");
    let write = proven_write(&store, &id, &body, &token).await;
    let (_, mut canonical, _) = retained(&write);
    canonical
        .get_document_mut("record")
        .unwrap()
        .insert("version", 1);
    let changed: Value =
        mongodb::bson::deserialize_from_bson(canonical.get("record").unwrap().clone()).unwrap();
    canonical.insert(
        "record_sha256",
        layer2_core::profiles::content_digest(&changed).unwrap(),
    );
    let decisions = client
        .database(&database)
        .collection::<Document>("bridge_callback_dispositions");
    decisions.insert_one(canonical.clone()).await.unwrap();
    assert_eq!(
        inbox.recover_receipt(&write).await,
        Err(CallbackError::Unavailable)
    );
    assert_eq!(inbox.accept(write).await, Err(CallbackError::Unavailable));
    assert_eq!(decisions.find_one(doc! {}).await.unwrap(), Some(canonical));
    no_message_effects(&client, &database, &inbox).await;
    client.database(&database).drop().await.unwrap();
}
