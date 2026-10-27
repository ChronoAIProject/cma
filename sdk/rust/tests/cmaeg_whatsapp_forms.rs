#[path = "support/wire.rs"]
mod wire;
use cma_sdk::cmaeg::{
    Cmaeg,
    types::{FormPreparationReceipt, OutboundMessage, ReplyOptions},
};
use serde_json::json;
#[test]
fn preparation_variants_preserve_not_sent_and_existing_delivery_receipts() {
    for state in [
        json!({"status":"pending","retry_after_ms":1000}),
        json!({"status":"ready","preparation_ref":"ready"}),
        json!({"status":"uncertain","reason":"create_unconfirmed"}),
        json!({"status":"revoked","reason":"consent"}),
        json!({"status":"declined","reason":"validation"}),
    ] {
        let mut value = json!({"preparation_id":"opaque","expires_at":1900000000,"customer_message":"not_sent"});
        value
            .as_object_mut()
            .unwrap()
            .extend(state.as_object().unwrap().clone());
        let decoded: FormPreparationReceipt = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
}
#[tokio::test]
async fn prepares_reads_and_explicitly_commits_with_exact_headers() {
    let server = wire::WireServer::new().await;
    let _ = (&server.client, &server.anonymous);
    let client = Cmaeg::builder(&server.base_url)
        .unwrap()
        .bearer_token("sdk-fixture")
        .unwrap()
        .build()
        .unwrap();
    let body = json!({"text":"Form prompt"});
    let message: OutboundMessage = serde_json::from_value(body.clone()).unwrap();
    let ready = json!({"preparation_id":"opaque","expires_at":1900000000,"customer_message":"not_sent","status":"ready","preparation_ref":"ready"});
    let mut post = wire::json(
        "POST",
        "/v1/events/event%2Fref/form-preparation",
        Some(body.clone()),
        200,
        ready.clone(),
        true,
    );
    post.headers
        .insert("idempotency-key".into(), "prepare-key".into());
    server.expect(post);
    assert_eq!(
        serde_json::to_value(
            client
                .prepare_form_reply("event/ref", &message, "prepare-key")
                .await
                .unwrap()
        )
        .unwrap(),
        ready
    );
    server.expect(wire::json(
        "GET",
        "/v1/events/event%2Fref/form-preparation",
        None,
        200,
        ready,
        true,
    ));
    client.read_form_preparation("event/ref").await.unwrap();
    let mut post = wire::json(
        "POST",
        "/v1/events/event%2Fref/replies",
        Some(body),
        200,
        json!({"delivery":{"outcome":"confirmed"},"rendering":{"outcome":"rendered"}}),
        true,
    );
    post.headers
        .insert("idempotency-key".into(), "commit-key".into());
    post.headers
        .insert("form-preparation-ref".into(), "ready".into());
    server.expect(post);
    client
        .reply_to_event_with_options(
            "event/ref",
            &message,
            ReplyOptions {
                idempotency_key: Some("commit-key"),
                form_preparation_ref: Some("ready"),
            },
        )
        .await
        .unwrap();
    server.assert_drained();
    assert_eq!(server.request_count(), 3);
}
