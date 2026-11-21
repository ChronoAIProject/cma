//! One confirmed native upload message is the smallest public effect target.
use super::*;

fn upload(text: Option<&str>, count: usize) -> OutboundMessage {
    serde_json::from_value(json!({
        "text": text,
        "native_attachments": (0..count).map(|index| json!({
            "kind":"file", "filename":format!("part-{index}.txt"),
            "source":{"type":"base64","data":"AQIDBA=="}
        })).collect::<Vec<_>>()
    }))
    .unwrap()
}

#[tokio::test]
async fn confirmed_uploads_issue_exact_effects_only_scope_without_text_edit_gate() {
    for (text, count) in [(None, 1), (Some("files"), 4)] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = setup(&f);
        sender.relay = sender
            .relay
            .with_native_discord_effects(true)
            .with_native_discord_media_effects(true);
        let result = send(&sender, &channel, &event, &upload(text, count)).await;
        assert_eq!(result.delivery.kind(), "confirmed");
        let reference = result
            .message_ref
            .unwrap()
            .open(&sender.keys)
            .unwrap()
            .payload;
        assert_eq!(reference.platform_message_id, "800");
        assert_eq!(reference.route_id, "discord:77:99");
        let destination = reference.discord_message.as_ref().unwrap();
        assert!(destination.effects_only);
        let window = destination.effects.as_ref().unwrap();
        assert_eq!(window.expires_at - window.issued_at, 86400);
        assert_eq!(destination.bot_id, "77");
        assert_eq!(destination.channel_id, "99");
        // Even enabling edit issuance cannot widen the retained upload scope.
        sender.relay = sender.relay.with_native_discord_edits(true);
        let refused = edit(&sender, &channel, &reference, &message("replacement"), true).await;
        assert_eq!(refused.delivery.kind(), "unsupported");
        let calls = f.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].path, "/api/v10/channels/99/messages");
        assert_eq!(calls[0].method, "POST");
        assert!(calls[0]
            .content_type
            .as_ref()
            .unwrap()
            .starts_with("multipart/form-data"));
        let body = String::from_utf8_lossy(&calls[0].body);
        for index in 0..count {
            assert!(body.contains(&format!("name=\"files[{index}]\"")));
        }
        assert!(!body.contains(&format!("name=\"files[{count}]\"")));
    }
}

#[tokio::test]
async fn upload_authority_requires_both_effect_gates_independently_of_edit_issuance() {
    for edits in [false, true] {
        for (effects, media) in [(false, false), (false, true), (true, false), (true, true)] {
            let f = Fixture::new().await;
            let (mut sender, channel, event) = setup(&f);
            sender.relay = sender
                .relay
                .with_native_discord_edits(edits)
                .with_native_discord_effects(effects)
                .with_native_discord_media_effects(media);
            let result = send(&sender, &channel, &event, &upload(None, 1)).await;
            assert_eq!(result.delivery.kind(), "confirmed");
            assert_eq!(result.message_ref.is_some(), effects && media);
        }
    }
}

#[tokio::test]
async fn multiple_native_messages_partial_and_unconfirmed_uploads_never_issue_authority() {
    let f = Fixture::new().await;
    let (mut sender, channel, event) = setup(&f);
    sender.relay = sender
        .relay
        .with_native_discord_effects(true)
        .with_native_discord_media_effects(true)
        .with_native_discord_edits(true);
    let multipart = upload(Some(&"x".repeat(2001)), 2);
    let sent = send(&sender, &channel, &event, &multipart).await;
    assert_eq!(sent.delivery.kind(), "confirmed");
    assert!(sent.message_ref.is_none());
    {
        let calls = f.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0]
            .content_type
            .as_ref()
            .unwrap()
            .starts_with("multipart/form-data"));
        assert_eq!(calls[1].content_type.as_deref(), Some("application/json"));
    }
    f.memory.responses.lock().unwrap().extend([
        (StatusCode::OK, response("first")),
        (StatusCode::BAD_GATEWAY, json!({})),
    ]);
    let partial = send(&sender, &channel, &event, &multipart).await;
    assert!(
        matches!(partial.delivery, DeliveryOutcome::Unknown { reason } if reason == "partial_delivery")
    );
    assert!(partial.message_ref.is_none());
    for body in [
        json!({}),
        json!({"id":"800","channel_id":"100","author":{"id":"77"}}),
        json!({"id":"800","channel_id":"99","author":{"id":"78"}}),
    ] {
        f.respond(StatusCode::OK, body);
        let uncertain = send(&sender, &channel, &event, &upload(None, 1)).await;
        assert_eq!(uncertain.delivery.kind(), "unknown");
        assert!(uncertain.message_ref.is_none());
    }
    *f.memory.broken.lock().unwrap() = true;
    let lost = send(&sender, &channel, &event, &upload(None, 1)).await;
    assert_eq!(lost.delivery.kind(), "unknown");
    assert!(lost.message_ref.is_none());
    assert_eq!(f.memory.calls.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn degraded_card_upload_mismatched_record_and_handoff_do_not_issue_effects() {
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
    sender.relay = sender
        .relay
        .with_native_discord_effects(true)
        .with_native_discord_media_effects(true);
    let mut media = upload(None, 1);
    media.blocks =
        serde_json::from_value(json!([{"type":"card","title":"card","fields":[]}])).unwrap();
    let degraded = send(&sender, &channel, &event, &media).await;
    assert_eq!(degraded.delivery.kind(), "confirmed");
    assert!(degraded.message_ref.is_none());
    event.record_id = "other-record".parse().unwrap();
    assert!(sender
        .discord_message_ref(&channel, &event, "800", &upload(None, 1))
        .is_none());
    event.record_id = channel.pointer.record_id.clone();
    event.lineage = Some(cmaeg_core::seal::Lineage {
        root: "a".repeat(32),
        hop: 1,
        path: vec![cmaeg_core::ids::ConsumerId::parse("default").unwrap()],
        reply: cmaeg_core::channel::ReplyChoice::Origin,
    });
    let handoff = send(&sender, &channel, &event, &upload(None, 1)).await;
    assert_eq!(handoff.delivery.kind(), "confirmed");
    assert!(handoff.message_ref.is_none());
}
