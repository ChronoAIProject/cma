use super::*;

#[tokio::test(start_paused = true)]
async fn commonmark_validates_bytes_and_obeys_durable_barrier_before_token_claim() {
    for commit in [true, false] {
        let (mut sender, transport, channel, event) = setup();
        sender.relay = sender
            .relay
            .with_complete_replies(false)
            .with_commonmark_relay(true);
        let trace = tracing::Span::none();
        let gate = RecordingGate::new(commit);
        let ctx = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let (oversized, effects) = sender
            .reply(&ctx, &event, &message("*".repeat(REQUEST_MAX_BYTES)))
            .await;
        assert_eq!(
            oversized.delivery,
            DeliveryOutcome::Rejected {
                reason: "relay_request_too_large".into()
            }
        );
        assert!(!effects.first_send_done);
        assert!(gate.calls().is_empty());
        assert!(sender.consumed.lock().unwrap().is_empty());
        assert!(transport.0.lock().unwrap().is_empty());
        let text = "**原文** `C:\\qa\\menu_01.txt`".repeat(500);
        let (receipt, effects) = sender.reply(&ctx, &event, &message(text.clone())).await;
        if commit {
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert!(effects.first_send_done);
            assert_eq!(
                gate.calls(),
                ["enter_route", "attempt", "commit", "progress"]
            );
            assert_eq!(sender.consumed.lock().unwrap().len(), 1);
            let sent = transport.1.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].1["reply"]["text"], text);
        } else {
            assert_eq!(
                receipt.delivery,
                DeliveryOutcome::Rejected {
                    reason: "delivery_expired".into()
                }
            );
            assert!(!effects.first_send_done);
            assert_eq!(gate.calls(), ["enter_route", "attempt", "commit"]);
            assert!(sender.consumed.lock().unwrap().is_empty());
            assert!(transport.0.lock().unwrap().is_empty());
        }
    }
}
