use super::{
    native_whatsapp_tests::{media_setup, send_media},
    *,
};
use crate::whatsapp::outbound::{
    document,
    tests::{answer, attachment, confirmed},
};
use cmaeg_core::message::NativeMediaSource;

#[tokio::test]
async fn native_whatsapp_documents_preserve_exact_uploaded_content_and_target() {
    for (kind, bytes) in document::tests::fixtures() {
        let (sender, channel, event, graph, _server) = media_setup(vec![
            answer(json!({"id":"45678"})),
            confirmed("wamid.document"),
        ])
        .await;
        let mut message = message(String::new());
        message.text = None;
        let mut item = attachment(kind.mime());
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(&bytes),
        };
        item.mime_type = None;
        message.native_attachments = vec![item];
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{kind:?}"
        );
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].2.windows(bytes.len()).any(|part| part == bytes));
        let body: serde_json::Value = serde_json::from_slice(&calls[1].2).unwrap();
        assert_eq!(body["type"], "document");
        assert_eq!(
            body["document"],
            json!({"id":"45678","caption":"A caption",
            "filename":format!("attachment-0.{}",kind.extensions()[0])})
        );
        assert_eq!(body["to"], "447700900123");
        assert_eq!(body["context"]["message_id"], "wamid.original");
    }
}

#[tokio::test]
async fn native_whatsapp_documents_validate_the_entire_batch_before_text_and_upload() {
    for defect in 0..5 {
        let (sender, channel, event, graph, _server) = media_setup(vec![]).await;
        let mut message = message("Must not be sent".into());
        let mut item = attachment(document::Document::WordXml.mime());
        match defect {
            0 => item.filename = Some("wrong.xlsx".into()),
            1 => item.mime_type = Some(document::Document::ExcelXml.mime().into()),
            2 => {
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(b"PK\x03\x04fake"),
                }
            }
            3 => {
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(b"plain text"),
                }
            }
            _ => {
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode([vec![b'a'; 65], vec![0xff]].concat()),
                }
            }
        }
        message.native_attachments = vec![attachment("image/png"), item];
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}
