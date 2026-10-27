use super::support::App;
use cmaeg::delivery::intents::{DeliveryCommand, DeliveryOutcome};
use cmaeg_core::{
    message::{OutboundMessage, RenderingOutcome},
    seal::MessageRefPayload,
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::oneshot;

fn choice() -> Value {
    json!({"text":"Choose owner","blocks":[{"type":"choice","id":"owner","prompt":"Personal or org?","multi":false,"options":[{"id":"personal","label":"Personal"},{"id":"org","label":"Organization"}]}]})
}

#[tokio::test]
async fn lark_replies_are_plain_text_without_reply_markup_even_with_interactions() {
    let app = App::on("lark", true).await;
    let receipt = app.reply(choice()).await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("platform-1".into())
        }
    );
    assert_eq!(receipt.rendering.outcome, RenderingOutcome::Degraded);
    let reference = receipt
        .message_ref
        .unwrap()
        .open(&app.fixture.state.master_keys)
        .unwrap()
        .payload;
    assert_eq!(reference.interaction_record_id, None);
    {
        let records = app.fake.requests.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].body["reply"]["text"],
            "Choose owner\n\nPersonal or org?\n1) Personal\n2) Organization\nReply with a number."
        );
        assert_eq!(records[0].body["reply"]["metadata"], Value::Null);
    }
    // Message updates are Telegram interactive cards only.
    let (sender, received) = oneshot::channel();
    app.commands
        .send(DeliveryCommand::Update {
            trace: tracing::Span::current(),
            channel: app.turn.channel.clone(),
            message_ref: MessageRefPayload {
                discord_message: None,
                native_lark_effect: None,
                whatsapp_reaction: None,
                lark_effect: None,
                telegram_effect: None,
                interaction_record_id: Some("record-1".into()),
                ..reference
            },
            message: serde_json::from_value::<OutboundMessage>(choice()).unwrap(),
            receipt: sender,
        })
        .await
        .unwrap();
    let update = tokio::time::timeout(Duration::from_secs(20), received)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(update.delivery, DeliveryOutcome::Unsupported);
    assert_eq!(app.fake.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn telegram_with_interactions_still_sends_native_controls() {
    let app = App::on("telegram", true).await;
    // The unreachable interaction record store proves the interactive path ran.
    let receipt = app.reply(choice()).await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Rejected {
            reason: "interaction_state_unavailable".into()
        }
    );
    assert!(app.fake.requests.lock().unwrap().is_empty());
}
