//! Signed HTTP requests against disposable majority storage, without Agent work.
use super::*;
use crate::bridge::ingress::{event_cases, CallbackFixture, CallbackIngress, PinnedCallbackKeys};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

const CALLBACK_ROOT: &str = "https://cma.example/api/v1/bridge-callbacks";

async fn request(
    store: &Arc<MongoBridgeStore>,
    database: &str,
    protocol: &CallbackFixture,
) -> (StatusCode, Value) {
    let inbox = MongoCallbackInbox::open(store, database, "disposable-callback", &encryption_key())
        .await
        .unwrap();
    let ingress = CallbackIngress::new(
        store.clone(),
        Arc::new(Authority),
        Arc::new(inbox),
        PinnedCallbackKeys::fixture(),
        CallbackTokenPolicy {
            issuer: "https://nyx.example".into(),
            relay_audience: "https://nyx.example".into(),
            callback_ttl_seconds: 300,
            relay_ttl_seconds: 300,
            reply_ttl_seconds: 1800,
            clock_skew_seconds: 60,
        },
        CALLBACK_ROOT.into(),
    )
    .unwrap();
    let app = crate::bridge::ingress::http::router(Some(Arc::new(ingress)));
    let mut request = Request::post(format!(
        "/api/v1/bridge-callbacks/{}",
        protocol.bridge.id.as_str()
    ))
    .body(Body::from(protocol.bytes()))
    .unwrap();
    *request.headers_mut() = protocol.headers();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap(),
    )
}

async fn no_effects(client: &Client, database: &str, inbox: &MongoCallbackInbox) {
    let db = client.database(database);
    assert_eq!(
        db.collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {"admission":"accepted"})
            .await
            .unwrap(),
        0
    );
    assert!(inbox.pending_channel_inputs(None).await.unwrap().is_empty());
    let dispositions = db.collection::<Document>("bridge_callback_dispositions");
    assert_eq!(
        dispositions
            .count_documents(doc! {"record.receipt.kind":"control_refused"})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        dispositions
            .count_documents(doc! {"kind":"provider"})
            .await
            .unwrap(),
        0
    );
    for name in [
        "bridge_callback_receipts",
        "bridge_callback_replies",
        "bridge_callback_platform_messages",
        "agent_commands",
        "agent_log",
        "command_authorization_audits",
        "bridge_outbox_items",
        "bridge_outbox_invocations",
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "unexpected effect in {name}"
        );
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_unsupported_event_mongo_matrix_has_no_claimable_message_or_control_effects() {
    for platform in event_cases::platforms() {
        let mut protocol = CallbackFixture::new(platform.clone());
        let id = BridgeService::creation_id(&caller(), "create").unwrap();
        protocol.config_mut().connector.callback_url = format!("{CALLBACK_ROOT}/{}", id.as_str());
        protocol.config_mut().connector.route_ids = vec!["route-1".into()];
        protocol.body["conversation"]["id"] = json!("route-1");
        let (client, database, store, inbox, id) =
            setup_with_config(protocol.config().clone()).await;
        protocol.bridge = store.bridge(&id).await.unwrap().unwrap();
        let db = client.database(&database);
        let mut denied = 0;
        for (index, case) in event_cases::cases(&platform).into_iter().enumerate() {
            let mut payload = protocol.clone();
            payload.body["message_id"] = json!(format!("unsupported-{index}"));
            payload.body["correlation_id"] = json!(format!("unsupported-correlation-{index}"));
            payload.body["thread_id"] = json!("interaction:app:matrix-private-context");
            event_cases::text(
                &mut payload.body,
                &platform,
                "CMA-CONTROL/1 cancel ccr_source",
            );
            case.apply(&mut payload.body, &platform);
            let (status, response) = request(&store, &database, &payload).await;
            if let Some(reason) = case.expected(&platform) {
                assert_eq!(
                    status,
                    StatusCode::ACCEPTED,
                    "{platform:?} {case:?}: {response}"
                );
                assert_eq!(response["status"], "inbox_persisted");
                assert_eq!(
                    response["receipt"]["admission"],
                    json!({"partition_denied":{"reason":reason}})
                );
                assert!(response["receipt"].get("execution_hold").is_none());
                assert!(inbox
                    .channel_input(response["receipt"]["inbox_id"].as_str().unwrap())
                    .await
                    .unwrap()
                    .is_none());
                let replay = request(&store, &database, &payload).await;
                assert_eq!(replay, (status, response), "{platform:?} {case:?}");
                denied += 1;
            } else {
                assert_eq!(
                    status,
                    StatusCode::BAD_REQUEST,
                    "{platform:?} {case:?}: {response}"
                );
                assert_eq!(response["code"], "callback_payload_invalid");
            }
            no_effects(&client, &database, &inbox).await;
            for collection in ["bridge_callback_inbox", "bridge_callback_dispositions"] {
                assert_eq!(
                    db.collection::<Document>(collection)
                        .count_documents(doc! {})
                        .await
                        .unwrap(),
                    denied,
                    "{platform:?} {case:?}: {collection}"
                );
            }
        }
        // Fresh legitimate text still admits once and aliases to its original receipt.
        let (status, accepted) = request(&store, &database, &protocol).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{platform:?}: {accepted}");
        assert_eq!(accepted["receipt"]["admission"], "accepted");
        protocol.body["message_id"] = json!("fresh-signed-alias");
        protocol.body["correlation_id"] = json!("fresh-signed-correlation");
        assert_eq!(
            request(&store, &database, &protocol).await,
            (status, accepted)
        );
        assert_eq!(inbox.pending_channel_inputs(None).await.unwrap().len(), 1);
        assert_eq!(
            db.collection::<Document>("bridge_callback_inbox")
                .count_documents(doc! {"admission":"accepted"})
                .await
                .unwrap(),
            1
        );
        db.drop().await.unwrap();
    }
}
