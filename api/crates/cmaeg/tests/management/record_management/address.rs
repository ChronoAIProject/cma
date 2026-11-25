use super::*;

#[tokio::test]
async fn saved_creation_flags_and_endpoint_urls_match_existing_reads() {
    let fixture = Fixture::new().await;
    for advertise in [false, true] {
        let mut state = fixture.state();
        state.public_api_url = "https://gateway.example/event-gateway".into();
        state.advertise_public_api_url = advertise;
        let (base, stop, task) = serve_app(state).await;
        for kind in ["relay", "telegram", "native_lark", "whatsapp"] {
            let mut body = create_body();
            match kind {
                "telegram" => {
                    body["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
                    body["credentials"]["telegram_bot_token"] = "77:fixture-token".into();
                    body["credentials"]["telegram_webhook_secret"] = "fixture-webhook".into();
                }
                "native_lark" => {
                    body["sources"] = json!([{"type":"native_lark","separate_app":true,
                        "identity":{"region":"lark","app_id":"cli_fixture","tenant_key":"fixture_tenant","bot_open_id":"ou_fixture"},"admission":{"type":"all"}}]);
                    body["credentials"]["native_lark"] = json!({"app_secret":"fixture-native-secret","encrypt_key":"fixture-native-encryption","verification_token":"fixture-native-verification"});
                }
                "whatsapp" => {
                    body["sources"] = json!([{"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567","admission":{"type":"all"}}]);
                    body["credentials"]["whatsapp"] = json!({"app_secret":"fixture-whatsapp-app-secret","access_token":"fixture-whatsapp-access-token","verify_token":"fixture-whatsapp-verification"});
                }
                _ => {}
            }
            let mut record = definition_from(body);
            let id = saved_id(&fixture.cma.base_url, &record.record_id, 1);
            for ready in [false, true] {
                record.creation = Some(creation(id.clone(), ready));
                record.validate().unwrap();
                fixture.seed(&record);
                let body = read(get(&base, &record.record_id).await).await;
                assert_eq!(
                    body["record"]["creation"],
                    if ready { "complete" } else { "incomplete" }
                );
                assert_eq!(body["record"]["address"]["state"], "available");
                assert_eq!(body["record"]["address"]["channel_id"], id.as_str());
                let endpoints = &body["record"]["address"]["endpoints"];
                for (source, field) in [
                    ("telegram", "telegram_webhook_url"),
                    ("native_lark", "native_lark_webhook_url"),
                    ("whatsapp", "whatsapp_webhook_url"),
                ] {
                    assert_eq!(endpoints.get(field).is_some(), source == kind);
                }
                let expected = if advertise {
                    "https://gateway.example/event-gateway"
                } else {
                    "http://127.0.0.1:8090"
                };
                assert_eq!(
                    endpoints["custom_event_url"],
                    format!("{expected}/v1/channels/{id}/events")
                );
                for path in [
                    format!("/v1/channel-records/{}", record.record_id),
                    format!("/v1/channels/{id}"),
                ] {
                    let old = Client::new()
                        .get(format!("{base}{path}"))
                        .bearer_auth("token-alice")
                        .send()
                        .await
                        .unwrap();
                    headers(&old, 200);
                    let old: Value = old.json().await.unwrap();
                    assert_eq!(old["channel_id"], id.as_str());
                    assert_eq!(&old["endpoints"], endpoints);
                }
            }
        }
        stop.send(()).unwrap();
        task.await.unwrap();
    }
    fixture.close().await;
}

#[tokio::test]
async fn unverified_saved_ids_are_withheld_without_resealing_or_echoing() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    let mut record = definition();
    for bad_id in [
        ChannelId::new("cmaeg1.ch.undecipherable-private-id").unwrap(),
        saved_id(&fixture.cma.base_url, &new_id(), 1),
        saved_id("https://foreign.example", &record.record_id, 1),
        saved_id(&fixture.cma.base_url, &record.record_id, 2),
    ] {
        record.creation = Some(creation(bad_id.clone(), true));
        // The record uses readable key 1; the last saved ID uses unavailable key 2.
        fixture.seed(&record);
        let result = read(get(&base, &record.record_id).await).await;
        assert_eq!(result["record"]["state"], "valid");
        assert_eq!(result["record"]["address"], json!({"state":"unverifiable"}));
        assert!(!result.to_string().contains(bad_id.as_str()));
        assert!(!result.to_string().contains("channel_id"));
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn explicitly_retained_origin_and_key_preserve_the_original_address() {
    let fixture = Fixture::new().await;
    let mut record = definition();
    let id = saved_id("https://retained.example", &record.record_id, 1);
    record.creation = Some(creation(id.clone(), true));
    fixture.seed(&record);
    let mut state = fixture.state();
    state.master_keys = Arc::new(key_ring(2));
    state.records = Arc::new(
        RecordClient::new(&fixture.cma.base_url, "cma-service")
            .unwrap()
            .with_retained_origins(&["https://retained.example".into()])
            .unwrap(),
    );
    let (base, stop, task) = serve_app(state).await;
    let result = read(get(&base, &record.record_id).await).await;
    assert_eq!(result["record"]["address"]["state"], "available");
    assert_eq!(result["record"]["address"]["channel_id"], id.as_str());
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}
