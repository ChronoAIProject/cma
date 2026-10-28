use super::{native_lark_tests::media_setup, native_whatsapp_tests::send_media, *};
use crate::native_lark::{outbound::tests::fixture::*, test_support as native};
use cmaeg_core::activity::SourceKind;

#[tokio::test]
async fn native_lark_office_receipts_preserve_selected_named_consumer_and_provider() {
    use crate::{
        ingress::routing_plan::fixture as routed,
        interactions::native_lark::fixture::Fixture,
        native_lark::{self, receipts::Store},
    };
    let f = Fixture::new(true, false, cmaeg_core::channel::LarkRegion::Feishu).await;
    let (mut sender, _, _, _) = setup();
    sender.keys = Arc::new(routed::keys());
    let io = Api::new(vec![uploaded(false), confirmed("om_document", false)]);
    sender.relay = sender
        .relay
        .with_native_lark(
            native_lark::Client::production()
                .unwrap()
                .with_transport(io.clone()),
            Arc::new(Store::new(f.server.records.clone(), sender.keys.clone())),
        )
        .with_native_lark_outbound_media(true);
    let mut m = message(String::new());
    m.text = None;
    m.native_attachments = vec![attachment(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )];
    let receipt = send_media(&sender, &f.channel, &f.event, &m, &RecordingGate::new(true)).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let stored = sender
        .relay
        .native_lark_receipts
        .as_ref()
        .unwrap()
        .load(
            &f.event.channel_id,
            &SourceKind::NativeLark(native::test_support::identity()),
            "om_document",
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(stored.consumer), f.event.consumer);
    assert_eq!(
        stored.provider,
        f.channel
            .consumer(f.event.consumer.as_ref())
            .unwrap()
            .provider
            .clone()
    );
    assert_eq!(stored.original.event_id, f.event.event_id);
    assert_eq!(io.writes.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_lark_documents_keep_original_receipts_and_custody_at_every_write() {
    for mime in crate::native_lark::outbound::MIME_TYPES {
        if !mime.starts_with("application/") {
            continue;
        }
        let (sender, channel, event, io, _server) =
            media_setup(vec![uploaded(false), confirmed("om_document", false)]).await;
        let mut m = message(String::new());
        m.text = None;
        m.native_attachments = vec![attachment(mime)];
        let gate = RecordingGate::new(true);
        let receipt = send_media(&sender, &channel, &event, &m, &gate).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{mime}"
        );
        assert_eq!(io.writes.lock().unwrap().len(), 2);
        assert_eq!(io.reads.lock().unwrap().len(), 4);
        assert_eq!(
            gate.calls(),
            [
                "enter_route",
                "attempt",
                "commit",
                "progress",
                "attempt",
                "commit",
                "progress"
            ]
        );
        let stored = sender
            .relay
            .native_lark_receipts
            .as_ref()
            .unwrap()
            .load(
                &event.channel_id,
                &SourceKind::NativeLark(native::identity()),
                "om_document",
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
        assert_eq!(stored.reply.chat_id, "oc_native_chat");
        assert_eq!(stored.reply.message_id, "om_document");
    }
}

#[tokio::test]
async fn native_lark_document_batches_refuse_before_any_text_or_upload_on_invalid_typed_file() {
    for mime in [
        "application/zip",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ] {
        let (sender, channel, event, io, _server) = media_setup(vec![]).await;
        let mut m = message("Must not be sent".into());
        let mut broken = attachment(mime);
        broken.source = cmaeg_core::message::NativeMediaSource::Base64 {
            data: STANDARD.encode(b"PK\x03\x04truncated"),
        };
        m.native_attachments = vec![attachment("application/pdf"), broken];
        let receipt = send_media(&sender, &channel, &event, &m, &RecordingGate::new(true)).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
        assert!(io.reads.lock().unwrap().is_empty());
        assert!(io.writes.lock().unwrap().is_empty());
    }
}
