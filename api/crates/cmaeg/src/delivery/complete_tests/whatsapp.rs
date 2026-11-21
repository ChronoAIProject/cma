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

#[tokio::test(start_paused = true)]
async fn rich_template_preserves_media_currency_and_datetime_in_the_bound_reply() {
    let (mut sender, transport, channel, mut event) = setup();
    whatsapp(&mut event);
    sender.relay = sender.relay.with_whatsapp_templates(true);
    let message: OutboundMessage = serde_json::from_value(json!({"whatsapp_template":{
        "name":"invoice","language":{"code":"en_US"},"components":[
            {"type":"header","parameters":[{"type":"document","document":{"id":"123456789","filename":"invoice.pdf"}}]},
            {"type":"body","parameters":[{"type":"currency","currency":{"fallback_value":"USD 10","code":"USD","amount_1000":10000}},{"type":"date_time","date_time":{"fallback_value":"Tomorrow"}}]}
        ]
    }})).unwrap();
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender.reply(&ctx, &event, &message).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let sent = transport.1.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].1["reply"]["metadata"]["template"],
        serde_json::to_value(message.whatsapp_template).unwrap()
    );
}
#[tokio::test]
async fn private_template_links_are_rejected_before_any_commit() {
    let (_, _, _, mut event) = setup();
    whatsapp(&mut event);
    for link in [
        "https://127.0.0.1/a",
        "https://169.254.169.254/a",
        "https://files.internal/a",
        "https://localhost/a",
        "https://localhost./a",
        "https://files.internal./a",
        "https://[::1]/a",
    ] {
        let message: OutboundMessage = serde_json::from_value(json!({"whatsapp_template":{
            "name":"invoice","language":{"code":"en_US"},"components":[
                {"type":"header","parameters":[{"type":"image","image":{"link":link}}]}
            ]
        }}))
        .unwrap();
        assert!(RelayClient::prepare_whatsapp_template(
            &event.reply,
            message.whatsapp_template.as_ref().unwrap()
        )
        .is_err());
    }
}

struct MediaTransport {
    deadlines: Mutex<Vec<Duration>>,
    delay: Duration,
    receipt: Vec<u8>,
}
impl RelayTransport for MediaTransport {
    fn post(
        &self,
        _: Url,
        _: HeaderMap,
        _: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.deadlines.lock().unwrap().push(timeout);
        Box::pin(async {
            tokio::time::sleep(self.delay).await;
            Ok(HttpReply::new(StatusCode::OK, self.receipt.clone()))
        })
    }
}

fn media_template(kind: &str, reference: serde_json::Value) -> OutboundMessage {
    serde_json::from_value(json!({"whatsapp_template":{
        "name":"approved_media","language":{"code":"en_US"},"components":[
            {"type":"header","parameters":[{"type":kind,kind:reference}]}
        ]
    }}))
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn media_templates_allow_platform_processing_time_and_require_native_receipts() {
    for kind in ["image", "video", "document"] {
        for reference in [
            json!({"id":"123456789"}),
            json!({"link":"https://media.example/approved"}),
        ] {
            for native_receipt in [true, false] {
                let (mut sender, _, channel, mut event) = setup();
                whatsapp(&mut event);
                let transport = Arc::new(MediaTransport {
                    deadlines: Mutex::default(),
                    delay: Duration::from_secs(6),
                    receipt: serde_json::to_vec(&json!({
                        "message_id":"out-1",
                        "platform_message_id":native_receipt.then_some("platform-1")
                    }))
                    .unwrap(),
                });
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
                let message = media_template(kind, reference.clone());
                let (receipt, effects) = sender.reply(&ctx, &event, &message).await;
                if native_receipt {
                    assert!(matches!(
                        receipt.delivery,
                        DeliveryOutcome::Confirmed {
                            platform_message_id: Some(_)
                        }
                    ));
                } else {
                    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
                    assert!(receipt.message_ref.is_none());
                }
                assert!(effects.first_send_done);
                assert_eq!(
                    *transport.deadlines.lock().unwrap(),
                    [Duration::from_secs(60)]
                );
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn template_deadlines_bound_one_attempt_without_expanding_text_only_budget() {
    for message in [
        template(),
        media_template("image", json!({"id":"123456789"})),
    ] {
        let (mut sender, _, channel, mut event) = setup();
        whatsapp(&mut event);
        let expected = if message.whatsapp_template.as_ref().unwrap().has_media() {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(5)
        };
        let transport = Arc::new(MediaTransport {
            deadlines: Mutex::default(),
            delay: Duration::from_secs(61),
            receipt: br#"{"message_id":"out-1","platform_message_id":"platform-1"}"#.to_vec(),
        });
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
        let started = tokio::time::Instant::now();
        let (receipt, effects) = sender.reply(&ctx, &event, &message).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(effects.first_send_done);
        assert_eq!(started.elapsed(), expected);
        assert_eq!(*transport.deadlines.lock().unwrap(), [expected]);
    }
}
