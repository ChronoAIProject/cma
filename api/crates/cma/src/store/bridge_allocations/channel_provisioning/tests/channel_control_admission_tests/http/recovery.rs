use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_reopens_v2_duplicate_and_provider_alias_before_flags_or_source() {
    let c = ControlFixture::with_callback_route().await;
    let original = body(&c, "http-original", 530, false);
    let (status, accepted) =
        post(app(&c, true, Some(Runtime::local(&c))).await, &c, &original).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    c.f.finish().await;
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_channel_control_references")
        .delete_one(doc! {"_id":&c.reference.reference})
        .await
        .unwrap();
    for value in [original, body(&c, "http-provider-alias", 530, false)] {
        let (status, recovered) = post(app(&c, false, None).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{recovered}");
        assert_eq!(recovered, accepted);
    }
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 1);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 2);
    assert_eq!(rows(&c, "agent_commands").await, 2);
    assert_eq!(rows(&c, "bridge_callback_inbox").await, 1);
    assert_eq!(rows(&c, "bridge_outbox_items").await, 1);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_disabled_refusal_is_permanent_across_selector_change_and_alias() {
    let c = ControlFixture::with_callback_route().await;
    let runtime = Runtime::local(&c);
    let original = body(&c, "http-disabled", 531, false);
    let (status, refused) = post(app(&c, false, Some(runtime.clone())).await, &c, &original).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{refused}");
    assert_eq!(refused["status"], "control_refused");
    assert_eq!(refused["receipt"]["code"], "channel_controls_disabled");
    for value in [original, body(&c, "http-disabled-alias", 531, false)] {
        let (status, recovered) =
            post(app(&c, true, Some(runtime.clone())).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{recovered}");
        assert_eq!(recovered, refused);
    }
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    no_message_effects(&c, 0).await;
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_legacy_message_history_wins_before_fresh_control_admission() {
    let c = ControlFixture::with_callback_route().await;
    let value = body(&c, "http-legacy-message", 532, false);
    let source = legacy_source(&c, &value).await;
    c.f.client.database(&c.f.database).collection::<Document>("bridge_callback_inbox")
        .insert_one(doc! {"_id":&source.id,"document_schema_version":1_i32,
        "bridge_id":source.pin.bridge_id.as_str(),"admission":"accepted",
        "expires_at":mongodb::bson::DateTime::from_millis(source.expires_at*1000),"record":record_bson(&source).unwrap()}).await.unwrap();
    for enabled in [false, true] {
        let (status, recovered) = post(app(&c, enabled, None).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{recovered}");
        assert_eq!(
            recovered,
            json!({"status":"inbox_persisted","receipt":source.receipt()})
        );
    }
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 0);
    assert_eq!(rows(&c, "agent_commands").await, 1);
    assert_eq!(rows(&c, "bridge_callback_inbox").await, 2);
    assert_eq!(rows(&c, "bridge_outbox_items").await, 1);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_reserved_approval_and_legacy_reference_shape_keep_original_refusals()
{
    let c = ControlFixture::with_callback_route().await;
    for (index, (text, code)) in [
        (
            "CMA-CONTROL/1 approve ccr_source",
            "approval_dashboard_only",
        ),
        (
            "CMA-CONTROL/1 cancel ccr_source",
            "channel_controls_disabled",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = body(
            &c,
            &format!("http-reserved-{index}"),
            533 + index as u64,
            false,
        );
        value["content"]["text"] = json!(text);
        value["raw_platform_data"]["message"]["text"] = json!(text);
        let (status, result) = post(app(&c, true, None).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{result}");
        assert_eq!(result["status"], "control_refused");
        assert_eq!(result["receipt"]["code"], code);
    }
    no_message_effects(&c, 0).await;
    c.f.close().await;
}
