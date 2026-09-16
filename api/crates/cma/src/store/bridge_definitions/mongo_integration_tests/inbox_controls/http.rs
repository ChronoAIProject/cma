use super::*;
use crate::bridge::ingress::{CallbackIngress, PinnedCallbackKeys};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_http_handler_commits_control_refusal_and_replays_after_reopen() {
    let id = BridgeService::creation_id(&caller(), "create").unwrap();
    let callback_root = "https://cma.example/api/v1/bridge-callbacks";
    let mut config = config();
    config.connector.callback_url = format!("{callback_root}/{}", id.as_str());
    let (client, database, store, inbox, id) = setup_with_config(config).await;
    let (body, token, _) = callback("http-reserved", "CMA-CONTROL/1 approve ccr_source");
    let mut original = None;
    for _ in 0..2 {
        let inbox =
            MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
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
            callback_root.into(),
        )
        .unwrap();
        let app = crate::bridge::ingress::http::router(Some(Arc::new(ingress)));
        let request = Request::post(format!("/api/v1/bridge-callbacks/{}", id.as_str()))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", &token)
            .header("x-nyxid-message-id", "http-reserved")
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-timestamp", layer2_core::now_rfc3339())
            .body(Body::from(body.clone()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(response["status"], "control_refused");
        assert_eq!(response["receipt"]["code"], "approval_dashboard_only");
        if let Some(original) = &original {
            assert_eq!(&response, original);
        } else {
            original = Some(response);
        }
    }
    no_message_effects(&client, &database, &inbox).await;
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_callback_dispositions")
            .count_documents(doc! {"kind":"callback"})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}
