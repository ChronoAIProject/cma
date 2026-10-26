use super::*;

#[tokio::test]
async fn outbound_native_media_and_whatsapp_templates_preserve_typed_contract() {
    let f = Fixture::new(Platform::Whatsapp).await;
    for (index,body) in [
        json!({"text":"media","native_attachments":[{"kind":"image","source":{"type":"base64","data":"aGVsbG8="}},{"kind":"file","source":{"type":"url","url":"https://files.example/report.pdf"},"filename":"report.pdf"},{"kind":"audio","source":{"type":"base64","data":"aGVsbG8="}},{"kind":"video","source":{"type":"base64","data":"aGVsbG8="}}]}),
        json!({"whatsapp_template":{"name":"shipment_update","language":{"code":"en_US"},"components":[]}}),
    ].into_iter().enumerate() {
        let request=PrepareOutboundMessage{source_id:SourceId::parse("relay_whatsapp").unwrap(),route_id:ROUTE.into(),message:serde_json::from_value(body).unwrap()};
        let p=outbound::prepare(&f.state,&owner(),&f.channel,&format!("rich-{index}"),request.clone()).await.unwrap();
        outbound::commit(&f.state,&owner(),&f.channel,&p.delivery_id,request.message).await.unwrap();
    }
    let sent = f.probe.bodies.lock().unwrap().clone();
    assert_eq!(
        sent[0]["message"]["attachments"].as_array().unwrap().len(),
        4
    );
    assert_eq!(
        sent[1]["message"]["metadata"]["template"]["name"],
        "shipment_update"
    );
    assert!(sent.iter().all(|v| v.get("recipient").is_none()));
    assert_eq!(
        *f.probe.timeouts.lock().unwrap(),
        vec![Duration::from_secs(60), Duration::from_secs(1)]
    );
    f.close().await;
}

#[tokio::test]
async fn outbound_media_requires_platform_receipt_and_never_resends_uncertain_delivery() {
    let f = Fixture::new(Platform::Lark).await;
    f.probe.omit_platform_receipt.store(true, Ordering::SeqCst);
    let request = PrepareOutboundMessage {
        source_id: SourceId::parse("relay_lark").unwrap(),
        route_id: ROUTE.into(),
        message: serde_json::from_value(json!({
            "native_attachments": [{
                "kind": "file",
                "source": {"type": "url", "url": "https://files.example/report.pdf"},
                "filename": "report.pdf"
            }]
        }))
        .unwrap(),
    };
    let prepared = outbound::prepare(&f.state, &owner(), &f.channel, "receipt", request.clone())
        .await
        .unwrap();
    for _ in 0..2 {
        let receipt = outbound::commit(
            &f.state,
            &owner(),
            &f.channel,
            &prepared.delivery_id,
            request.message.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(receipt.delivery).unwrap(),
            json!({"outcome": "unknown", "reason": "receipt_invalid"})
        );
    }
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn outbound_http_requires_auth_and_exact_body_and_exposes_no_store_status() {
    let f = Fixture::new(Platform::Telegram).await;
    let (base, stop, task) = serve_app(f.state.clone()).await;
    let url = format!("{base}/v1/channels/{}/outbound-messages", f.channel);
    let client = Client::new();
    assert_eq!(
        client
            .post(&url)
            .json(&request())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = client
        .post(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "http")
        .json(&request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let prepared: outbound::OutboundReceipt = response.json().await.unwrap();
    let send = format!("{url}/{}/send", prepared.delivery_id);
    assert_eq!(
        client
            .post(&send)
            .bearer_auth("token-alice")
            .json(&json!({"text":"x","metadata":{"recipient":"other"}}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .post(&send)
            .bearer_auth("token-alice")
            .json(&message())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn outbound_gate_preserves_reads_and_cancel_and_refuses_new_preparations() {
    let f = Fixture::new(Platform::Telegram).await;
    let prepared = f.prepare("gate").await;
    let mut off = f.restart();
    off.outbound_messages_writes_enabled = false;
    assert_eq!(
        outbound::prepare(&off, &owner(), &f.channel, "new", request())
            .await
            .err()
            .unwrap()
            .status,
        403
    );
    assert!(
        outbound::read(&off, &owner(), &f.channel, &prepared.delivery_id)
            .await
            .is_ok()
    );
    assert!(
        outbound::commit(&off, &owner(), &f.channel, &prepared.delivery_id, message())
            .await
            .is_err()
    );
    assert_eq!(
        outbound::cancel(&off, &owner(), &f.channel, &prepared.delivery_id)
            .await
            .unwrap()
            .status,
        OutboundStatus::Cancelled
    );
    f.close().await;
}
