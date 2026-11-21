use super::*;

fn enabled(f: &Fixture) -> (Sender, Arc<ResolvedChannel>, EventRefPayload) {
    let (mut sender, channel, mut event) = setup(f);
    sender.relay = sender
        .relay
        .with_telegram_effects(true)
        .with_telegram_media_effects(true);
    event.activity_sha256 = Some("a".repeat(64));
    event.telegram_admission = Some(cmaeg_core::seal::TelegramAdmissionProof {
        actor_id: "42".into(),
        actor_kind: cmaeg_core::activity::ActorKind::Human,
        conversation_kind: cmaeg_core::activity::ConversationKind::Group,
        mentioned: Some(true),
        replied_to_bot: Some(false),
    });
    (sender, channel, event)
}

#[tokio::test]
async fn single_confirmed_native_upload_issues_effects_only_with_exact_topic() {
    for kind in ["image", "file", "audio", "video"] {
        let f = Fixture::new().await;
        let (sender, channel, event) = enabled(&f);
        let media: OutboundMessage = serde_json::from_value(json!({"native_attachments":[{
            "kind":kind,"source":{"type":"base64","data":"AQIDBA=="},"caption":"caption"}]}))
        .unwrap();
        let sent = send(&sender, &channel, &event, &media, &RecordingGate::new(true)).await;
        assert_eq!(sent.delivery.kind(), "confirmed");
        let reference = sent
            .message_ref
            .unwrap()
            .open(&sender.keys)
            .unwrap()
            .payload;
        assert_eq!(reference.platform_message_id, "800");
        assert_eq!(reference.route_id, "telegram:77:-42");
        let authority = reference.telegram_effect.as_ref().unwrap();
        assert!(authority.effects_only);
        assert_eq!(authority.thread_id.as_deref(), Some("7"));
        assert_eq!(authority.expires_at - authority.issued_at, 86400);
        let gate = RecordingGate::new(true);
        let updated = sender
            .update(
                &SendContext {
                    trace: &tracing::Span::none(),
                    channel: &channel,
                    first_send_done: false,
                    gate: &gate,
                },
                &reference,
                &serde_json::from_value(json!({"text":"replacement"})).unwrap(),
            )
            .await
            .0;
        assert_eq!(updated.delivery.kind(), "unsupported");
        assert_eq!(f.calls().len(), 1);
        assert!(String::from_utf8_lossy(&f.calls()[0].body).contains("caption"));
    }
}

#[tokio::test]
async fn media_references_require_both_gates_and_native_admission_proof() {
    for case in ["global_off", "media_off", "proof_missing", "wrong_record"] {
        let f = Fixture::new().await;
        let (mut sender, channel, mut event) = enabled(&f);
        match case {
            "global_off" => sender.relay = sender.relay.with_telegram_effects(false),
            "media_off" => sender.relay = sender.relay.with_telegram_media_effects(false),
            "proof_missing" => event.telegram_admission = None,
            "wrong_record" => event.record_id = "other-record".parse().unwrap(),
            _ => unreachable!(),
        }
        let sent = send(
            &sender,
            &channel,
            &event,
            &message(),
            &RecordingGate::new(true),
        )
        .await;
        assert_eq!(sent.delivery.kind(), "confirmed");
        assert!(sent.message_ref.is_none(), "{case}");
    }
}

#[tokio::test]
async fn separate_text_uploads_and_partial_batches_never_gain_one_aggregate_reference() {
    for case in ["text", "files", "partial"] {
        let f = Fixture::new().await;
        let (sender, channel, event) = enabled(&f);
        let mut media = message();
        if case == "text" {
            media.text = Some("separate text".into());
        } else {
            media
                .native_attachments
                .push(media.native_attachments[0].clone());
        }
        if case == "partial" {
            f.answer(Answer::ok());
            f.answer(Answer::json(reqwest::StatusCode::BAD_GATEWAY, json!({})));
        }
        let sent = send(&sender, &channel, &event, &media, &RecordingGate::new(true)).await;
        assert_eq!(
            sent.delivery.kind(),
            if case == "partial" {
                "unknown"
            } else {
                "confirmed"
            }
        );
        assert!(sent.message_ref.is_none());
        assert_eq!(f.calls().len(), 2);
    }
}

#[tokio::test]
async fn uncertain_unbound_or_album_upload_receipts_never_grant_effects_or_retry() {
    for case in ["lost", "chat", "topic", "id", "album"] {
        let f = Fixture::new().await;
        let (sender, channel, event) = enabled(&f);
        let mut value =
            json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42},"message_thread_id":7}});
        match case {
            "chat" => value["result"]["chat"]["id"] = (-43).into(),
            "topic" => value["result"]["message_thread_id"] = 8.into(),
            "id" => value["result"]["message_id"] = 0.into(),
            "album" => value["result"]["media_group_id"] = "group".into(),
            _ => {}
        }
        let mut answer = Answer::json(reqwest::StatusCode::OK, value);
        answer.broken = case == "lost";
        f.answer(answer);
        let sent = send(
            &sender,
            &channel,
            &event,
            &message(),
            &RecordingGate::new(true),
        )
        .await;
        assert_eq!(sent.delivery.kind(), "unknown", "{case}");
        assert!(sent.message_ref.is_none());
        assert_eq!(f.calls().len(), 1);
    }
}
