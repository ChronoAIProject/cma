use super::{gate::recording::RecordingGate, send::SendContext, telegram_media_tests::setup};
use crate::telegram::media_fixture::Fixture;
use cmaeg_core::seal::ReplyAddress;
use serde_json::json;

#[tokio::test]
async fn static_replies_fail_closed_without_gate_retained_proof_or_original_native_destination() {
    for case in ["off", "missing_retained", "non_native", "mixed"] {
        let f = Fixture::new().await;
        let (mut sender, channel, mut event) = setup(&f);
        sender.relay = sender
            .relay
            .with_native_telegram_structured_replies(case != "off");
        if case == "non_native" {
            event.reply = ReplyAddress::None;
        }
        let mut wire =
            json!({"structured_content":{"type":"location","latitude":1.3,"longitude":103.8}});
        if case == "mixed" {
            wire["text"] = "not exclusive".into();
        }
        let message = serde_json::from_value(wire).unwrap();
        let trace = tracing::Span::none();
        let gate = RecordingGate::new(true);
        let context = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let (result, _) = sender.reply(&context, &event, &message).await;
        assert_eq!(result.delivery.kind(), "rejected", "{case}");
        assert!(result.message_ref.is_none());
        assert!(f.calls().is_empty());
        assert!(gate.calls().is_empty());
    }
}
