use super::*;
use cmaeg_core::message::NativeMediaKind;

#[tokio::test]
async fn presentation_issuance_gate_prevents_effects_and_never_grants_edit_or_reaction_references()
{
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
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
    let mut message = message();
    message.native_attachments[0].kind = NativeMediaKind::Voice;
    message.native_attachments[0].filename = None;
    message.native_attachments[0].source = cmaeg_core::message::NativeMediaSource::Base64 {
        data: STANDARD.encode(include_bytes!("../../tests/fixtures/media/voice.ogg")),
    };
    let refused = send(
        &sender,
        &channel,
        &event,
        &message,
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(refused.delivery, DeliveryOutcome::Unsupported));
    assert!(f.calls().is_empty());
    sender.relay = sender.relay.with_native_media_presentations(true);
    f.answer(Answer::json(
        reqwest::StatusCode::OK,
        json!({"ok":true,"result":{
            "message_id":800,"chat":{"id":-42},"message_thread_id":7,"voice":{"file_id":"voice"}
        }}),
    ));
    let sent = send(
        &sender,
        &channel,
        &event,
        &message,
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(sent.delivery, DeliveryOutcome::Confirmed { .. }));
    assert!(sent.message_ref.is_none() && sent.message_parts.is_empty());
    assert_eq!(f.calls().len(), 1);
    sender.relay = sender.relay.with_native_media_presentations(false);
    let refused = send(
        &sender,
        &channel,
        &event,
        &message,
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(refused.delivery, DeliveryOutcome::Unsupported));
    assert_eq!(f.calls().len(), 1);
}

#[tokio::test]
async fn a_later_invalid_presentation_prevents_text_and_all_earlier_batch_effects() {
    let f = Fixture::new().await;
    let (mut sender, channel, event) = setup(&f);
    sender.relay = sender.relay.with_native_media_presentations(true);
    let mut message = message();
    message.text = Some("must not be sent".into());
    let mut invalid = message.native_attachments[0].clone();
    invalid.kind = NativeMediaKind::Voice;
    message.native_attachments.push(invalid);
    let result = send(
        &sender,
        &channel,
        &event,
        &message,
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(result.delivery, DeliveryOutcome::Rejected { .. }));
    assert!(f.calls().is_empty());
}
