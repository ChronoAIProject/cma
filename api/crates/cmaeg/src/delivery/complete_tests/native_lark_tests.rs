use super::native_whatsapp_tests::send_media;
use super::*;
use crate::{
    ingress::routing_store::test_server::Server,
    native_lark::{self, outbound::tests::fixture::*, receipts::Store, test_support as native},
};
use cmaeg_core::{activity::SourceKind, message::Block};

pub(super) async fn media_setup(
    replies: Vec<Result<HttpReply, TransportError>>,
) -> (
    Sender,
    Arc<ResolvedChannel>,
    EventRefPayload,
    Arc<Api>,
    Server,
) {
    let (mut sender, _, mut channel, mut event) = setup();
    let server = Server::start_realtime().await;
    let io = Api::new(replies);
    sender.relay = sender
        .relay
        .with_native_lark(
            native_lark::Client::production()
                .unwrap()
                .with_transport(io.clone()),
            Arc::new(Store::new(server.records.clone(), sender.keys.clone())),
        )
        .with_native_lark_outbound_media(true);
    let definition = &mut Arc::make_mut(&mut channel).definition;
    definition.sources = vec![native::source()];
    definition.secrets.native_lark = Some(native::credentials());
    crate::channels::definition::fixture::replace_default_credential(
        definition,
        Some("native-effects-test-agent"),
    );
    event.reply = ReplyAddress::NativeLark(reply());
    (sender, channel, event, io, server)
}
fn media_message() -> OutboundMessage {
    let mut m = message("Explanation".into());
    m.native_attachments = vec![attachment("application/pdf")];
    m
}
#[tokio::test]
async fn native_lark_effect_reference_issuance_requires_one_confirmed_retained_message() {
    for (enabled, media, multipart) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        let mut responses = vec![];
        if !media || multipart {
            responses.push(confirmed("om_text", false));
        }
        if media {
            responses.push(uploaded(false));
            responses.push(confirmed("om_pdf", false));
        }
        let (mut sender, channel, event, io, _server) = media_setup(responses).await;
        sender.relay = sender.relay.with_native_lark_effects(enabled);
        let mut outbound = message("reply".into());
        if media {
            outbound.native_attachments = vec![attachment("application/pdf")];
        }
        if media && !multipart {
            outbound.text = None;
        }
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &outbound,
            &RecordingGate::new(true),
        )
        .await;
        assert_eq!(receipt.delivery.kind(), "confirmed");
        if enabled && !multipart {
            let reference = receipt
                .message_ref
                .expect("single retained message authority");
            let payload = reference.open(&sender.keys).unwrap().payload;
            let authority = payload.native_lark_effect.unwrap();
            assert!(authority.confirmed());
            assert_eq!(authority.editable(), !media);
            assert_eq!(
                authority.reply.message_id,
                if media { "om_pdf" } else { "om_text" }
            );
            assert!(io
                .reads
                .lock()
                .unwrap()
                .iter()
                .any(|url| url.path().ends_with("/bot/v3/info")));
        } else {
            assert!(receipt.message_ref.is_none());
        }
    }
}
#[tokio::test]
async fn native_lark_media_confirms_each_message_and_checks_custody_and_gate_for_every_write() {
    let (sender, channel, event, io, _server) = media_setup(vec![
        confirmed("om_text", false),
        uploaded(false),
        confirmed("om_pdf", false),
    ])
    .await;
    let gate = RecordingGate::new(true);
    let receipt = send_media(&sender, &channel, &event, &media_message(), &gate).await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id:Some(ref id) } if id == "om_text")
    );
    assert!(receipt.message_ref.is_none());
    assert_eq!(io.writes.lock().unwrap().len(), 3);
    assert_eq!(io.reads.lock().unwrap().len(), 6);
    assert_eq!(
        gate.calls(),
        [
            "enter_route",
            "attempt",
            "commit",
            "progress",
            "attempt",
            "commit",
            "progress",
            "attempt",
            "commit",
            "progress"
        ]
    );
    for id in ["om_text", "om_pdf"] {
        let stored = sender
            .relay
            .native_lark_receipts
            .as_ref()
            .unwrap()
            .load(
                &event.channel_id,
                &SourceKind::NativeLark(native::identity()),
                id,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.original.event_id, event.event_id);
        assert_eq!(stored.consumer.as_str(), "default");
        assert_eq!(
            stored.provider,
            channel.consumer(None).unwrap().provider.clone()
        );
        assert_eq!(stored.partition_key, event.partition_key);
        assert_eq!(stored.conversation_id, event.conversation_id);
        assert_eq!(stored.reply.message_id, id);
    }
}
#[tokio::test]
async fn native_lark_preflight_and_independent_gate_refuse_without_platform_write() {
    for defect in [
        "disabled",
        "source",
        "credentials",
        "caption",
        "controls",
        "template",
        "thread_override",
        "commit",
        "second_invalid",
        "receipts",
        "consumer",
        "revision",
    ] {
        let (mut sender, mut channel, mut event, io, _server) = media_setup(vec![]).await;
        let mut m = media_message();
        match defect {
            "disabled" => sender.relay = sender.relay.with_native_lark_outbound_media(false),
            "source" => Arc::make_mut(&mut channel).definition.sources.clear(),
            "credentials" => Arc::make_mut(&mut channel).definition.secrets.native_lark = None,
            "caption" => m.native_attachments[0].caption = Some("unsupported".into()),
            "controls" => m.blocks.push(Block::Buttons {
                id: "choice".into(),
                options: vec![serde_json::from_value(json!({"id":"yes","label":"Yes"})).unwrap()],
            }),
            "template" => {
                m.whatsapp_template = Some(
                    serde_json::from_value(
                        json!({"name":"fixture","language":{"code":"en_US"},"components":[]}),
                    )
                    .unwrap(),
                )
            }
            "thread_override" => m.thread_reply = true,
            "second_invalid" => {
                let mut bad = attachment("image/png");
                bad.mime_type = Some("audio/ogg".into());
                m.native_attachments.push(bad);
            }
            "receipts" => sender.relay.native_lark_receipts = None,
            "consumer" => {
                event.consumer =
                    Some(cmaeg_core::ids::ConsumerId::parse("csm_0000000000ff").unwrap())
            }
            "revision" => {
                Arc::make_mut(&mut channel)
                    .definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .encrypt_key = Secret::new("rotated-fixture-encryption-key");
            }
            _ => {}
        }
        let gate = RecordingGate::new(defect != "commit");
        let receipt = send_media(&sender, &channel, &event, &m, &gate).await;
        assert!(
            matches!(
                receipt.delivery,
                DeliveryOutcome::Rejected { .. } | DeliveryOutcome::Unsupported
            ),
            "{defect}"
        );
        assert!(io.writes.lock().unwrap().is_empty(), "{defect}");
        if defect != "commit" {
            assert!(io.reads.lock().unwrap().is_empty(), "{defect}");
        }
    }
}
#[tokio::test]
async fn native_lark_wrong_bot_prevents_upload_even_with_matching_app_credentials() {
    let (mut sender, channel, event, _, _server) = media_setup(vec![]).await;
    let mut io = Api::new(vec![]);
    Arc::get_mut(&mut io).unwrap().bot = "ou_foreign_bot".into();
    sender.relay.native_lark = native_lark::Client::production()
        .unwrap()
        .with_transport(io.clone());
    let receipt = send_media(
        &sender,
        &channel,
        &event,
        &media_message(),
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(io.reads.lock().unwrap().len(), 2);
    assert!(io.writes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn native_lark_media_only_has_no_placeholder_and_ambiguity_stops_batch() {
    for at_upload in [true, false] {
        let mut responses = vec![];
        if !at_upload {
            responses.push(uploaded(false));
        }
        responses.push(Err(TransportError::Unavailable));
        let (sender, channel, event, io, _server) = media_setup(responses).await;
        let mut m = media_message();
        m.text = None;
        m.native_attachments.push(attachment("image/png"));
        let receipt = send_media(&sender, &channel, &event, &m, &RecordingGate::new(true)).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        let calls = io.writes.lock().unwrap();
        assert_eq!(calls.len(), if at_upload { 1 } else { 2 });
        assert_eq!(calls[0].0.path(), "/open-apis/im/v1/files");
    }
}
#[tokio::test]
async fn native_lark_partial_delivery_and_receipt_loss_do_not_send_remaining_media() {
    for receipt_loss in [true, false] {
        let responses = if receipt_loss {
            vec![uploaded(false), confirmed("om_pdf", false)]
        } else {
            vec![confirmed("om_text", false), answer(json!({"code":123}))]
        };
        let (mut sender, channel, event, io, _server) = media_setup(responses).await;
        let mut m = media_message();
        if receipt_loss {
            m.text = None;
            sender.relay.native_lark_receipts = Some(Arc::new(Store::new(
                crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap(),
                sender.keys.clone(),
            )));
        }
        m.native_attachments.push(attachment("image/png"));
        let receipt = send_media(&sender, &channel, &event, &m, &RecordingGate::new(true)).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert_eq!(io.writes.lock().unwrap().len(), 2);
    }
}
struct RevokeAfterUpload(Arc<Api>);
impl crate::delivery::gate::SendGate for RevokeAfterUpload {
    fn enter_route<'a>(
        &'a self,
        _: &'a str,
    ) -> BoxFuture<'a, Result<crate::delivery::gate::RouteTicket, crate::delivery::gate::GateRefusal>>
    {
        Box::pin(async { Ok(crate::delivery::gate::RouteTicket::empty()) })
    }
    fn commit(&self) -> bool {
        true
    }
    fn attempt(
        &self,
    ) -> BoxFuture<
        '_,
        Result<crate::delivery::gate::AttemptPermit, crate::delivery::gate::GateRefusal>,
    > {
        Box::pin(async {
            if self.0.writes.lock().unwrap().is_empty() {
                Ok(crate::delivery::gate::AttemptPermit::empty())
            } else {
                Err(crate::delivery::gate::GateRefusal {
                    reason: "authority_revoked",
                    retry_after: None,
                })
            }
        })
    }
    fn progress(&self) {}
}
#[tokio::test]
async fn native_lark_checks_authority_again_between_upload_and_recipient_send() {
    let (sender, channel, event, io, _server) = media_setup(vec![uploaded(false)]).await;
    let mut m = media_message();
    m.text = None;
    let receipt = send_media(
        &sender,
        &channel,
        &event,
        &m,
        &RevokeAfterUpload(io.clone()),
    )
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(io.writes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn ordinary_native_lark_answer_retains_confirmation_past_its_transport_deadline() {
    use crate::delivery::gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate, Ungated};
    use std::sync::atomic::Ordering;

    struct NearlyExpired;
    impl SendGate for NearlyExpired {
        fn enter_route<'a>(
            &'a self,
            route: &'a str,
        ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
            Ungated.enter_route(route)
        }
        fn commit(&self) -> bool {
            true
        }
        fn attempt(&self) -> BoxFuture<'_, Result<AttemptPermit, GateRefusal>> {
            Box::pin(async {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(59)).await;
                tokio::time::resume();
                Ok(AttemptPermit::empty())
            })
        }
        fn progress(&self) {}
    }

    let (sender, channel, event, io, server) =
        media_setup(vec![confirmed("om_delayed_receipt", false)]).await;
    server.memory.delay_ms.store(1500, Ordering::SeqCst);
    let receipt = send_media(
        &sender,
        &channel,
        &event,
        &message("ordinary answer".into()),
        &NearlyExpired,
    )
    .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { platform_message_id: Some(ref id) } if id == "om_delayed_receipt"
    ));
    assert_eq!(io.writes.lock().unwrap().len(), 1);
    assert_eq!(server.memory.writes.load(Ordering::SeqCst), 1);
}
