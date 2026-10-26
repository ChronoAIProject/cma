use super::*;

fn template() -> OutboundMessage {
    serde_json::from_value(json!({"whatsapp_template":{
        "name":"order_update","language":{"code":"en_US"},
        "components":[{"type":"body","parameters":[{"type":"text","text":"Order 42"}]}]
    }}))
    .unwrap()
}
fn whatsapp(event: &mut EventRefPayload) {
    let ReplyAddress::NyxidRelay { platform, .. } = &mut event.reply else {
        unreachable!()
    };
    *platform = Platform::Whatsapp;
}

#[tokio::test(start_paused = true)]
async fn template_uses_only_verified_event_destination_and_existing_commit_path() {
    let (mut sender, transport, channel, mut event) = setup();
    whatsapp(&mut event);
    sender.relay = sender.relay.with_whatsapp_templates(true);
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, effects) = sender.reply(&ctx, &event, &template()).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(effects.first_send_done);
    {
        let ReplyAddress::NyxidRelay { message_id, .. } = &event.reply else {
            unreachable!()
        };
        let sent = transport.1.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.path(), "/api/v1/channel-relay/reply");
        assert_eq!(
            sent[0].1,
            json!({"message_id":message_id,"reply":{"text":null,"metadata":{"template":template().whatsapp_template}}})
        );
    }
    assert_eq!(sender.consumed.lock().unwrap().len(), 1);
    assert_eq!(
        gate.calls(),
        ["enter_route", "attempt", "commit", "progress"]
    );
    let reference = receipt
        .message_ref
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    assert_eq!(reference.route_id, "route-1");
    assert_eq!(reference.event_id, event.event_id);
    assert!(reference.relay_message.is_none());
    let (edited, _) = sender.update(&ctx, &reference, &template()).await;
    assert_eq!(edited.delivery, DeliveryOutcome::Unsupported);
    assert_eq!(transport.0.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelled_template_never_consumes_the_reply_token_or_dispatches() {
    let (mut sender, transport, channel, mut event) = setup();
    whatsapp(&mut event);
    sender.relay = sender.relay.with_whatsapp_templates(true);
    let gate = RecordingGate::new(false);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, effects) = sender.reply(&ctx, &event, &template()).await;
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

#[tokio::test(start_paused = true)]
async fn disabled_wrong_sink_invalid_or_threaded_templates_never_claim_credentials() {
    for case in 0..6 {
        let (mut sender, transport, channel, mut event) = setup();
        whatsapp(&mut event);
        sender.relay = sender.relay.with_whatsapp_templates(case != 0);
        let mut message = template();
        match case {
            1 => {
                if let ReplyAddress::NyxidRelay { platform, .. } = &mut event.reply {
                    *platform = Platform::Discord;
                }
            }
            2 => message.thread_reply = true,
            3 => message.text = Some("ignored".into()),
            4 => message.whatsapp_template.as_mut().unwrap().name = "bad-name".into(),
            5 => event.reply = ReplyAddress::None,
            _ => {}
        }
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, effects) = sender.reply(&ctx, &event, &message).await;
        assert_eq!(receipt.delivery, DeliveryOutcome::Unsupported, "{case}");
        assert!(!effects.first_send_done);
        assert!(gate.calls().is_empty());
        assert!(transport.0.lock().unwrap().is_empty());
        assert!(sender.consumed.lock().unwrap().is_empty());
    }
}

struct Uncertain(StatusCode, Mutex<usize>, Vec<u8>);
impl RelayTransport for Uncertain {
    fn post(
        &self,
        _: Url,
        _: HeaderMap,
        _: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        *self.1.lock().unwrap() += 1;
        Box::pin(async { Ok(HttpReply::new(self.0, self.2.clone())) })
    }
}
#[tokio::test(start_paused = true)]
async fn unconfirmed_template_delivery_is_never_automatically_retried() {
    for (status, body) in [
        (StatusCode::TOO_MANY_REQUESTS, ""),
        (StatusCode::BAD_GATEWAY, ""),
        (StatusCode::OK, ""),
        (StatusCode::OK, r#"{"message_id":"out-1"}"#),
    ] {
        let (mut sender, _, channel, mut event) = setup();
        whatsapp(&mut event);
        let transport = Arc::new(Uncertain(status, Mutex::new(0), body.as_bytes().to_vec()));
        sender.relay = sender
            .relay
            .with_whatsapp_templates(true)
            .with_transport(transport.clone());
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, effects) = sender.reply(&ctx, &event, &template()).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(effects.first_send_done);
        assert_eq!(*transport.1.lock().unwrap(), 1);
    }
}
