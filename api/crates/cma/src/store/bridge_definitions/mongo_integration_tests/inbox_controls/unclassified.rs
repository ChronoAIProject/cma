use super::*;

async fn transport_only(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
) -> InboxWrite {
    let bridge = store.bridge(id).await.unwrap().unwrap();
    let proof = Authority
        .validate_connection(&bridge.owner, &bridge.current().unwrap().config)
        .await
        .unwrap();
    let binding = CallbackBinding::new(bridge, proof).unwrap();
    let keys = CallbackKeySet::from_pinned_jwks(
        "https://nyx.example".into(),
        JWKS,
        tokio::time::Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let verifier = CallbackVerifier::new(CallbackTokenPolicy {
        issuer: "https://nyx.example".into(),
        relay_audience: "https://nyx.example".into(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    })
    .unwrap();
    let payload: Value = serde_json::from_slice(body).unwrap();
    verifier
        .verify(
            binding,
            &keys,
            CallbackHeaders {
                callback_token: token,
                user_token: None,
                message_id: payload["message_id"].as_str().unwrap(),
                platform: "telegram",
                timestamp: &layer2_core::now_rfc3339(),
            },
            body,
        )
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn transport_only_write_cannot_bypass_guard_but_can_recover_exact_history() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("transport-only", "CMA-CONTROL/1 cancel ccr_source");
    let write = transport_only(&store, &id, &body, &token).await;
    assert!(write.record().capabilities.is_none());
    assert!(write.control_refusal().is_none());
    assert_eq!(inbox.accept(write).await, Err(CallbackError::Payload));
    no_message_effects(&client, &database, &inbox).await;
    let db = client.database(&database);
    assert_eq!(
        db.collection::<Document>("bridge_callback_dispositions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let classified = proven_write(&store, &id, &body, &token).await;
    let legacy_receipt = AcceptedCallbackReceipt::capture(classified.record()).unwrap();
    // Genuine historical v1 Message evidence is recovered before the new gate.
    db.collection::<Document>("bridge_callback_receipts")
        .insert_one(legacy_receipt.document().unwrap())
        .await
        .unwrap();
    assert_eq!(
        inbox
            .accept(transport_only(&store, &id, &body, &token).await)
            .await
            .unwrap(),
        CallbackAcceptance::Message(legacy_receipt.receipt)
    );
    let (control, control_token, _) =
        callback("classified-refusal", "CMA-CONTROL/1 cancel ccr_source");
    let refused = inbox
        .accept(proven_write(&store, &id, &control, &control_token).await)
        .await
        .unwrap();
    assert!(matches!(refused, CallbackAcceptance::ControlRefused(_)));
    assert_eq!(
        inbox
            .accept(transport_only(&store, &id, &control, &control_token).await)
            .await
            .unwrap(),
        refused
    );
    db.drop().await.unwrap();
}
