use super::*;

#[tokio::test]
async fn owned_send_helper_prepares_and_commits_exactly_once() {
    let server = WireServer::new().await;
    let client = client(&server);
    let body: PrepareOutboundMessage =
        serde_json::from_value(sample("PrepareOutboundMessage")).unwrap();
    let mut prepared = sample("OutboundReceipt");
    prepared["status"] = "prepared".into();
    prepared["delivery"] = serde_json::Value::Null;
    let id = prepared["delivery_id"].as_str().unwrap();
    let mut prepare = wire::json(
        "POST",
        "/v1/channels/owned/outbound-messages",
        Some(serde_json::to_value(&body).unwrap()),
        201,
        prepared.clone(),
        true,
    );
    prepare
        .headers
        .insert("idempotency-key".into(), "logical-one".into());
    server.expect(prepare);
    let mut committed = prepared.clone();
    committed["status"] = "committed".into();
    committed["delivery"] = json!({"outcome":"unknown","reason":"lost"});
    server.expect(wire::json(
        "POST",
        &format!("/v1/channels/owned/outbound-messages/{id}/send"),
        Some(serde_json::to_value(&body.message).unwrap()),
        200,
        committed.clone(),
        true,
    ));
    same(
        client
            .send_outbound_message("owned", &body, "logical-one")
            .await
            .unwrap(),
        committed,
    );
    server.assert_drained();
}

#[tokio::test]
async fn owned_send_helper_returns_receipt_reference_on_ambiguous_commit() {
    let server = WireServer::new().await;
    let client = client(&server);
    let body: PrepareOutboundMessage =
        serde_json::from_value(sample("PrepareOutboundMessage")).unwrap();
    let mut prepared = sample("OutboundReceipt");
    prepared["status"] = "prepared".into();
    prepared["delivery"] = serde_json::Value::Null;
    let id = prepared["delivery_id"].as_str().unwrap();
    let mut prepare = wire::json(
        "POST",
        "/v1/channels/owned/outbound-messages",
        Some(serde_json::to_value(&body).unwrap()),
        201,
        prepared.clone(),
        true,
    );
    prepare
        .headers
        .insert("idempotency-key".into(), "logical-one".into());
    server.expect(prepare);
    server.expect(wire::json(
        "POST",
        &format!("/v1/channels/owned/outbound-messages/{id}/send"),
        Some(serde_json::to_value(&body.message).unwrap()),
        503,
        json!({"detail":"receipt unavailable"}),
        true,
    ));
    let error = match client
        .send_outbound_message("owned", &body, "logical-one")
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("expected uncertain failure"),
    };
    assert_eq!(error.delivery_id.as_deref(), Some(id));
    server.assert_drained();
}
