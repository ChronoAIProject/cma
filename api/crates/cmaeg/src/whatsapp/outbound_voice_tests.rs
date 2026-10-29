use super::tests::{answer, attachment, confirmed, Graph};
use super::*;
use crate::{net::exchange::TransportError, whatsapp::tests::credentials};
use serde_json::json;

async fn prepare(item: &NativeAttachment) -> Result<MediaUpload, Error> {
    let file = crate::media::outbound::prepare(std::slice::from_ref(item))
        .await
        .unwrap()
        .pop()
        .unwrap();
    Client::prepare_media(item, file).await
}

#[tokio::test]
async fn voice_is_explicit_and_audio_retains_its_exact_existing_wire_payload() {
    for voice in [false, true] {
        let graph = Graph::new(vec![
            answer(json!({"id":"45678"})),
            confirmed("wamid.voice"),
        ]);
        let api = Client::production().unwrap().with_transport(graph.clone());
        let mut item = attachment("audio/ogg");
        if voice {
            item.kind = NativeMediaKind::Voice;
        }
        let uploaded = api
            .upload_media(&credentials(), "34567", prepare(&item).await.unwrap())
            .await
            .unwrap();
        api.send_media(
            &credentials(),
            "34567",
            "447700900123",
            "wamid.original",
            uploaded,
        )
        .await
        .unwrap();
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let bytes = include_bytes!("../../tests/fixtures/media/voice.ogg");
        assert!(calls[0].2.windows(bytes.len()).any(|part| part == bytes));
        let body: Value = serde_json::from_slice(&calls[1].2).unwrap();
        assert_eq!(body["type"], "audio");
        assert_eq!(
            body["audio"],
            if voice {
                json!({"id":"45678","voice":true})
            } else {
                json!({"id":"45678"})
            }
        );
        assert_eq!(body["context"]["message_id"], "wamid.original");
        assert_eq!(body["to"], "447700900123");
    }
}

#[tokio::test]
async fn voice_requires_mono_opus_without_captions_and_never_retries_unknown_upload() {
    for mime in ["audio/aac", "audio/amr", "audio/mpeg", "audio/mp4"] {
        let mut item = attachment(mime);
        item.kind = NativeMediaKind::Voice;
        assert!(prepare(&item).await.is_err());
    }
    let mut item = attachment("audio/ogg");
    item.kind = NativeMediaKind::Voice;
    item.caption = Some("unsupported".into());
    assert!(prepare(&item).await.is_err());
    item.caption = None;
    let graph = Graph::new(vec![Err(TransportError::Unavailable)]);
    let api = Client::production().unwrap().with_transport(graph.clone());
    assert!(api
        .upload_media(&credentials(), "34567", prepare(&item).await.unwrap())
        .await
        .is_err());
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}
